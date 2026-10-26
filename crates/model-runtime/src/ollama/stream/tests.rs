use std::{
    error::Error,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::super::{MAX_STREAM_LINE_BYTES, OllamaError};
use crate::{
    DownloadCancellation, GenerationCompletion, GenerationEvent, ModelOutputFailureKind,
    OllamaChatRequest, OllamaProvider, OllamaProviderConfig, QwenStreamDecoder,
    VerifiedOllamaModel,
};

fn request() -> OllamaChatRequest {
    OllamaChatRequest::new(
        &VerifiedOllamaModel::pinned_for_adapter("test", "0"),
        vec![],
        false,
    )
}

fn consume_request(socket: &mut TcpStream) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    let length = String::from_utf8_lossy(&header)
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    socket.read_exact(&mut vec![0; length])
}

#[test]
fn answer_preview_arrives_before_the_server_can_finish() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let (preview_seen, wait_for_preview) = mpsc::channel();
    let server = thread::spawn(move || -> Result<(), Box<dyn Error + Send + Sync>> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        socket.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n",
        )?;
        // Each later record is gated by receipt of the previous visible delta.
        // A buffered client cannot pass by merely decoding everything at EOF.
        for text in ["Hello", " world"] {
            writeln!(
                socket,
                "{}",
                serde_json::json!({
                    "message": {"role": "assistant", "content": text}, "done": false
                })
            )?;
            socket.flush()?;
            wait_for_preview.recv_timeout(Duration::from_secs(3))?;
        }
        socket.write_all(b"{\"done\":true,\"done_reason\":\"stop\"}\n")?;
        Ok(())
    });
    let mut decoder = QwenStreamDecoder::new("stream-before-completion")?;
    let mut previews = String::new();
    let mut committed = String::new();
    provider.stream_chat_events(
        &request(),
        &DownloadCancellation::new(),
        &mut decoder,
        |event| match event {
            GenerationEvent::ContentPreviewDelta { text } => {
                previews.push_str(&text);
                let _ = preview_seen.send(());
            }
            GenerationEvent::FinalContentDelta { text } => committed.push_str(&text),
            _ => {}
        },
    )?;
    server
        .join()
        .map_err(|_| "stream server panicked")?
        .map_err(|error| error.to_string())?;
    assert_eq!(previews, "Hello world");
    assert_eq!(committed, previews);
    Ok(())
}

#[test]
fn cancellation_closes_delayed_headers_and_partial_records() -> Result<(), Box<dyn Error>> {
    for payload in [
        b"".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{\"message\":".as_slice(),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let provider =
            OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
        let (ready, waiting) = mpsc::channel();
        let server = thread::spawn(move || -> std::io::Result<()> {
            let (mut socket, _) = listener.accept()?;
            consume_request(&mut socket)?;
            socket.write_all(payload)?;
            ready.send(()).map_err(std::io::Error::other)?;
            assert_eq!(
                socket.read(&mut [0])?,
                0,
                "cancelled client must close the connection"
            );
            Ok(())
        });
        let cancellation = DownloadCancellation::new();
        let trigger = cancellation.clone();
        let canceller = thread::spawn(move || -> Result<Instant, mpsc::RecvTimeoutError> {
            waiting.recv_timeout(Duration::from_secs(3))?;
            // Give the request time to enter the pending header/body read.
            thread::sleep(Duration::from_millis(50));
            let at = Instant::now();
            trigger.cancel();
            Ok(at)
        });
        let mut decoder = QwenStreamDecoder::new("cancel")?;
        let mut events = Vec::new();
        let result = provider
            .stream_chat_events(&request(), &cancellation, &mut decoder, |e| events.push(e));
        assert!(matches!(result, Err(OllamaError::Cancelled)));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, GenerationEvent::Cancelled))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, GenerationEvent::Completed { .. }))
        );
        let at = canceller.join().map_err(|_| "cancel thread panicked")??;
        assert!(at.elapsed() < Duration::from_secs(1));
        server.join().map_err(|_| "server panicked")??;
    }
    Ok(())
}

#[test]
fn deadline_includes_the_wait_for_headers() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut config = OllamaProviderConfig::loopback(listener.local_addr()?)?;
    config.stream_timeout = Duration::from_millis(150);
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        assert_eq!(socket.read(&mut [0])?, 0);
        Ok(())
    });
    let provider = OllamaProvider::new(config)?;
    assert!(matches!(
        provider.stream_chat_bytes(&request(), &DownloadCancellation::new(), |_| true),
        Err(OllamaError::DeadlineExceeded)
    ));
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn prompt_render_can_be_cancelled_before_its_body_arrives() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let cancellation = DownloadCancellation::new();
    let trigger = cancellation.clone();
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")?;
        trigger.cancel();
        assert_eq!(socket.read(&mut [0])?, 0);
        Ok(())
    });
    assert!(matches!(
        provider.render_chat_prompt_with_cancellation(&request(), || cancellation.is_cancelled()),
        Err(OllamaError::Cancelled)
    ));
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn stopping_after_a_record_closes_a_stalled_response() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{}\n")?;
        assert_eq!(socket.read(&mut [0])?, 0);
        Ok(())
    });
    provider.stream_chat_bytes(&request(), &DownloadCancellation::new(), |_| false)?;
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn fragmented_records_and_unterminated_final_record_are_preserved() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")?;
        for fragment in [b"{".as_slice(), b"}\n{", b"}"] {
            socket.write_all(fragment)?;
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    });
    let mut records = Vec::new();
    provider.stream_chat_bytes(&request(), &DownloadCancellation::new(), |line| {
        records.push(line.to_vec());
        true
    })?;
    assert_eq!(records, [b"{}\n".to_vec(), b"{}".to_vec()]);
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn successful_http_eof_without_a_done_frame_is_an_incomplete_model_response()
-> Result<(), Box<dyn Error>> {
    for (body, expected) in [
        (
            b"{\"message\":{\"role\":\"assistant\",\"thinking\":\"partial\"},\"done\":false}\n"
                .as_slice(),
            GenerationEvent::Failed {
                failure: ModelOutputFailureKind::UnexpectedEnd,
            },
        ),
        (
            b"{\"done\":true,\"done_reason\":\"length\"}\n".as_slice(),
            GenerationEvent::Completed {
                reason: GenerationCompletion::Length,
            },
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let provider =
            OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
        let server = thread::spawn(move || -> std::io::Result<()> {
            let (mut socket, _) = listener.accept()?;
            consume_request(&mut socket)?;
            socket.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
            )?;
            socket.write_all(body)
        });
        let mut decoder = QwenStreamDecoder::new("missing-done")?;
        let mut events = Vec::new();
        provider.stream_chat_events(
            &request(),
            &DownloadCancellation::new(),
            &mut decoder,
            |event| events.push(event),
        )?;
        assert_eq!(events.last(), Some(&expected));
        server.join().map_err(|_| "server panicked")??;
    }
    Ok(())
}

#[test]
fn unterminated_oversized_record_is_rejected_before_eof() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        consume_request(&mut socket)?;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9000000\r\n\r\n")?;
        let _ = socket.write_all(&vec![b'x'; MAX_STREAM_LINE_BYTES + 1]);
        let _ = socket.read(&mut [0]);
        Ok(())
    });
    assert!(matches!(
        provider.stream_chat_bytes(&request(), &DownloadCancellation::new(), |_| true),
        Err(OllamaError::InvalidResponse)
    ));
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}
