use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

use crate::{QwenStreamDecoder, bundled_catalogue};

#[test]
fn provider_rejects_non_loopback_and_port_zero() {
    assert_eq!(
        OllamaProviderConfig::loopback(SocketAddr::from(([192, 168, 1, 2], 11_434))),
        Err(OllamaError::InvalidConfiguration)
    );
    assert_eq!(
        OllamaProviderConfig::loopback(SocketAddr::from(([127, 0, 0, 1], 0))),
        Err(OllamaError::InvalidConfiguration)
    );
}

#[test]
fn chat_streams_keep_a_bounded_thirty_minute_deadline() {
    let config = OllamaProviderConfig::default();
    assert_eq!(config.stream_timeout, Duration::from_mins(30));
    assert_eq!(CHAT_DEADLINE, Duration::from_mins(30));
}

#[test]
fn admitted_chat_request_explicitly_enables_ollama_prompt_caching() -> Result<(), Box<dyn Error>> {
    let verified = VerifiedOllamaModel::pinned_for_adapter(
        "qwen3.8:27b",
        "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
    );
    let request = OllamaChatRequest::new(
        &verified,
        vec![OllamaChatMessage::new("user", "hello")?],
        true,
    )
    .with_context(65_536, 8_192);
    let serialized = serde_json::to_value(request)?;

    assert_eq!(serialized["truncate"], false);
    assert_eq!(serialized["shift"], true);
    Ok(())
}

#[test]
fn semantic_version_comparison_is_strict_and_ordered() {
    assert!(version_at_least("0.32.12", "0.32.12"));
    assert!(version_at_least("v0.33.0", "0.32.12"));
    assert!(!version_at_least("0.32.11", "0.32.12"));
    assert!(!version_at_least("not-a-version", "0.32.12"));
}

#[test]
fn errors_never_include_server_content() {
    for error in [
        OllamaError::Unavailable,
        OllamaError::ServerRejected,
        OllamaError::InvalidResponse,
    ] {
        assert!(!error.to_string().contains("secret model output"));
    }
}

#[test]
fn provider_errors_map_to_closed_output_failure_classes() {
    let cases = [
        (OllamaError::DeadlineExceeded, "provider_timeout"),
        (OllamaError::Unavailable, "provider_unavailable"),
        (OllamaError::ServerRejected, "provider_rejected"),
        (OllamaError::InvalidResponse, "invalid_provider_response"),
        (OllamaError::Cancelled, "cancelled"),
    ];
    for (error, expected) in cases {
        assert_eq!(ModelOutputFailureKind::from(error).code(), expected);
    }
}

#[test]
fn verification_is_read_only_and_requires_the_exact_digest() -> Result<(), Box<dyn Error>> {
    let catalogue = bundled_catalogue()?;
    let model = &catalogue.models()[1];
    let distribution = model
        .ollama_distribution()
        .ok_or(OllamaError::InvalidConfiguration)?;
    let version = serde_json::json!({ "version": "0.32.12" }).to_string();
    let tags = serde_json::json!({
        "models": [{
            "name": distribution.tag(),
            "digest": format!("sha256:{}", distribution.manifest_sha256())
        }]
    })
    .to_string();
    let (address, requests, server) = mock_server(vec![version, tags]);
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;

    let verified = provider.verify_model(model)?;

    assert_eq!(verified.tag(), "qwen3.8:27b");
    assert_eq!(requests.recv()?, "GET /api/version");
    assert_eq!(requests.recv()?, "GET /api/tags");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}

#[test]
fn explicit_pull_reports_aggregate_layers_then_reverifies() -> Result<(), Box<dyn Error>> {
    let catalogue = bundled_catalogue()?;
    let model = &catalogue.models()[1];
    let distribution = model
        .ollama_distribution()
        .ok_or(OllamaError::InvalidConfiguration)?;
    let primary = model.artifact();
    let projector = model.companions()[0].artifact();
    let pull = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({
            "digest": format!("sha256:{}", primary.sha256()),
            "total": primary.installed_bytes(),
            "completed": primary.installed_bytes()
        }),
        serde_json::json!({
            "digest": format!("sha256:{}", projector.sha256()),
            "total": projector.installed_bytes(),
            "completed": projector.installed_bytes()
        }),
        serde_json::json!({ "status": "success" }),
    );
    let version = serde_json::json!({ "version": "0.32.12" }).to_string();
    let tags = serde_json::json!({
        "models": [{
            "name": distribution.tag(),
            "digest": distribution.manifest_sha256()
        }]
    })
    .to_string();
    let (address, requests, server) = mock_server(vec![pull, version, tags]);
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;
    let mut snapshots = Vec::new();

    provider.pull_and_verify(model, &DownloadCancellation::new(), |progress| {
        snapshots.push(progress);
    })?;

    let total = primary.installed_bytes() + projector.installed_bytes();
    assert_eq!(
        snapshots.first().map(|item| item.downloaded_bytes()),
        Some(0)
    );
    assert_eq!(
        snapshots.last().map(|item| item.downloaded_bytes()),
        Some(total)
    );
    assert!(
        snapshots
            .iter()
            .any(|item| item.phase() == DownloadPhase::Verifying)
    );
    assert!(
        snapshots
            .iter()
            .any(|item| item.phase() == DownloadPhase::Installing)
    );
    assert_eq!(requests.recv()?, "POST /api/pull");
    assert_eq!(requests.recv()?, "GET /api/version");
    assert_eq!(requests.recv()?, "GET /api/tags");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}

#[test]
fn pre_cancelled_streams_perform_no_network_io() -> Result<(), Box<dyn Error>> {
    let cancellation = DownloadCancellation::new();
    cancellation.cancel();
    let provider = OllamaProvider::new(OllamaProviderConfig::default())?;
    let verified = VerifiedOllamaModel {
        tuning_policy: QwenTuningPolicy::Existing,
        tag: "qwen3.8:27b".to_owned(),
        manifest_sha256: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643"
            .to_owned(),
        provider_version: crate::PINNED_OLLAMA_VERSION.to_owned(),
    };
    let request = OllamaChatRequest::new(
        &verified,
        vec![OllamaChatMessage::new("user", "hello")?],
        true,
    );
    assert_eq!(
        provider.stream_chat(&request, &cancellation, |_| {}),
        Err(OllamaError::Cancelled)
    );
    Ok(())
}

#[test]
fn raw_chat_records_drive_one_model_neutral_decoder_lifecycle() -> Result<(), Box<dyn Error>> {
    let body = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({
            "message": {"role": "assistant", "thinking": "check"},
            "done": false
        }),
        serde_json::json!({
            "message": {"role": "assistant", "content": "ready"},
            "done": false
        }),
        serde_json::json!({
            "message": {"role": "assistant", "content": ""},
            "done": true,
            "done_reason": "stop"
        }),
    );
    let (address, requests, server) = mock_server(vec![body]);
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;
    let verified = VerifiedOllamaModel::pinned_for_adapter(
        "qwen3.8:27b",
        "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
    );
    let request = OllamaChatRequest::new(
        &verified,
        vec![OllamaChatMessage::new("user", "hello")?],
        true,
    );
    let mut decoder = QwenStreamDecoder::new("ollama-bridge")?;
    let mut events = Vec::new();

    provider.stream_chat_events(
        &request,
        &DownloadCancellation::new(),
        &mut decoder,
        |event| events.push(event),
    )?;

    assert_eq!(
        events,
        vec![
            GenerationEvent::OutputStarted,
            GenerationEvent::ReasoningDelta {
                text: "check".to_owned()
            },
            GenerationEvent::ContentPreviewDelta {
                text: "ready".to_owned()
            },
            GenerationEvent::FinalContentDelta {
                text: "ready".to_owned()
            },
            GenerationEvent::Completed {
                reason: GenerationCompletion::Stop
            }
        ]
    );
    assert_eq!(requests.recv()?, "POST /api/chat");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}

#[test]
fn prompt_rendering_uses_the_cold_load_deadline() -> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({
        "_debug_info": {"rendered_template": "rendered prompt"}
    })
    .to_string();
    let (address, requests, server) = mock_server_with_delay(vec![body], Duration::from_millis(75));
    let mut config = OllamaProviderConfig::loopback(address)?;
    config.control_timeout = Duration::from_millis(10);
    config.stream_timeout = Duration::from_secs(1);
    let provider = OllamaProvider::new(config)?;
    let verified = VerifiedOllamaModel::pinned_for_adapter(
        "qwen3.8:27b",
        "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
    );
    let request = OllamaChatRequest::new(
        &verified,
        vec![OllamaChatMessage::new("user", "hello")?],
        true,
    );

    assert_eq!(provider.render_chat_prompt(&request)?, "rendered prompt");
    assert_eq!(requests.recv()?, "POST /api/chat");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}

#[allow(clippy::expect_used)]
fn mock_server(
    responses: Vec<String>,
) -> (SocketAddr, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    mock_server_with_delay(responses, Duration::ZERO)
}

#[allow(clippy::expect_used)]
fn mock_server_with_delay(
    responses: Vec<String>,
    response_delay: Duration,
) -> (SocketAddr, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock server");
    let address = listener.local_addr().expect("mock address");
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        for body in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).expect("read request");
            let first_line = String::from_utf8_lossy(&request[..length])
                .lines()
                .next()
                .expect("request line")
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            sender.send(first_line).expect("record request");
            thread::sleep(response_delay);
            write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write response");
        }
    });
    (address, receiver, server)
}
mod mlx;
