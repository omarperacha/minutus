use super::super::GenericError;
use super::{
    SourceFailure, SourceFailureReason as Reason, SourceFailureStage as Stage, compiler_cache,
    failure::{FailureClock, body_reason, http_reason},
    http, source_input,
};
use crate::PublicEvmAddress;
use serde_json::json;
use solidity_compilation::CompilationError;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

fn failure(error: GenericError) -> Result<SourceFailure, GenericError> {
    match error {
        GenericError::SourceQualificationFailed(failure) => Ok(failure),
        other => Err(other),
    }
}

#[test]
fn timings_distinguish_active_stage_from_whole_acquisition_without_inference_latency()
-> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .ok_or("clock underflow")?;
    let result =
        failure(FailureClock::start(started, Stage::SourceInput).fail(Reason::SourceSchema))?;
    assert!(result.elapsed_ms() >= 2000);
    assert!(result.stage_elapsed_ms() < result.elapsed_ms());
    assert_eq!(result.stage(), Stage::SourceInput);
    assert_eq!(result.reason(), Reason::SourceSchema);
    assert_eq!(result.http_status(), None);
    Ok(())
}

#[test]
fn compiler_resource_limit_and_cancellation_are_closed_causes_not_timeout_claims()
-> Result<(), GenericError> {
    for error in [
        CompilationError::Limit,
        CompilationError::Cancelled,
        CompilationError::WorkerUnavailable,
        CompilationError::CompilerMismatch,
        CompilationError::ArtifactMismatch,
        CompilationError::UnconfirmedTermination,
    ] {
        let error = FailureClock::start(Instant::now(), Stage::CompilerExecution)
            .fail(Reason::Compiler(error));
        let output = error.to_string();
        assert!(output.len() < 240);
        assert!(!output.contains("httpTimeout"));
        assert_eq!(failure(error)?.stage(), Stage::CompilerExecution);
    }
    assert_eq!(
        Reason::Compiler(CompilationError::Limit).code(),
        "compilerResourceLimit"
    );
    Ok(())
}

#[test]
fn source_json_schema_and_compiler_input_failures_exclude_untrusted_content()
-> Result<(), Box<dyn std::error::Error>> {
    let target = PublicEvmAddress::new("0x2222222222222222222222222222222222222222")?;
    let schema = json!({"chainId":"1","address":target.as_str(),
        "compilation":{"compilerVersion":"0.8.7+commit.e28d00a7","fullyQualifiedName":"Storage.sol:Storage"},
        "stdJsonInput":{"language":"Solidity","sources":{"Storage.sol":{"content":"secret credential https://user:pass@host/private"}},"settings":{}}});
    let mut invalid = schema.clone();
    invalid["address"] = json!("https://private-provider/credential");
    for (bytes, expected) in [
        (
            b"secret malformed JSON https://user:pass@host/private".to_vec(),
            Reason::JsonDecode,
        ),
        (serde_json::to_vec(&invalid)?, Reason::SourceSchema),
    ] {
        let result = source_input::parse(&bytes, 1, &target, Instant::now());
        let error = result.err().ok_or("unexpected input acceptance")?;
        assert_eq!(failure(error)?.reason(), expected);
        for secret in ["secret", "credential", "user:pass", "private-provider"] {
            assert!(!error.to_string().contains(secret));
        }
    }
    source_input::parse(&serde_json::to_vec(&schema)?, 1, &target, Instant::now())?;
    let mut unsupported = schema;
    unsupported["compilation"]["compilerVersion"] = json!("hostile compiler text");
    let result = source_input::parse(
        &serde_json::to_vec(&unsupported)?,
        1,
        &target,
        Instant::now(),
    );
    assert_eq!(
        failure(result.err().ok_or("unsupported compiler accepted")?)?.reason(),
        Reason::Compiler(CompilationError::UnsupportedCompiler)
    );
    Ok(())
}

#[test]
fn http_status_and_body_failures_keep_only_native_status_and_closed_cause()
-> Result<(), Box<dyn std::error::Error>> {
    for code in [404, 429, 503] {
        let response = format!(
            "HTTP/1.1 {code} Result\r\nContent-Length: 31\r\nConnection: close\r\n\r\nprovider secret https://credential"
        );
        let (url, server) = server(response.into_bytes(), Duration::ZERO)?;
        let result = http::get(
            &url,
            1024,
            &AtomicBool::new(false),
            Instant::now(),
            Stage::SourceHttp,
        );
        server.join().map_err(|_| "server failed")??;
        let error = result.err().ok_or("HTTP failure accepted")?;
        let facts = failure(error)?;
        assert_eq!(facts.reason(), Reason::HttpStatus);
        assert_eq!(facts.http_status(), Some(code));
        assert!(!error.to_string().contains("credential"));
        assert!(!error.to_string().contains("http://"));
    }
    let (url, server) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\nx".to_vec(),
        Duration::ZERO,
    )?;
    let result = http::get(
        &url,
        1024,
        &AtomicBool::new(false),
        Instant::now(),
        Stage::SourceHttp,
    );
    server.join().map_err(|_| "server failed")??;
    assert_eq!(
        failure(result.err().ok_or("truncated body accepted")?)?.reason(),
        Reason::HttpBodyRead
    );
    Ok(())
}

#[test]
fn actual_http_library_timeout_is_distinct_from_body_io_and_transport_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let (url, server) = server(Vec::new(), Duration::from_millis(150))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(30))
        .build()?;
    let error = client.get(url).send().err().ok_or("timeout not observed")?;
    assert!(error.is_timeout());
    assert_eq!(http_reason(&error), Reason::HttpTimeout);
    server.join().map_err(|_| "server failed")??;
    let arbitrary = std::io::Error::new(std::io::ErrorKind::TimedOut, "secret timeout prose");
    assert_eq!(body_reason(&arbitrary), Reason::HttpBodyRead);
    let io_error = std::io::Error::other(error);
    assert_eq!(body_reason(&io_error), Reason::HttpTimeout);
    let bad_url = client
        .get("invalid secret URL")
        .build()
        .err()
        .ok_or("bad URL accepted")?;
    assert_eq!(http_reason(&bad_url), Reason::HttpTransport);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let connect = client
        .get(format!("http://{address}/"))
        .send()
        .err()
        .ok_or("closed listener accepted a connection")?;
    assert!(connect.is_connect());
    assert_eq!(http_reason(&connect), Reason::HttpConnect);
    Ok(())
}

#[test]
fn body_timeout_is_reported_only_from_actual_http_library_error()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let mut stream = accept(&listener)?;
        let _ = stream.read(&mut [0; 1024])?;
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx")?;
        thread::sleep(Duration::from_millis(300));
        Ok(())
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()?;
    let mut response = client.get(format!("http://{address}/private")).send()?;
    let result = response.read_to_end(&mut Vec::new());
    server.join().map_err(|_| "body server failed")??;
    let error = result.err().ok_or("missing body did not time out")?;
    assert_eq!(body_reason(&error), Reason::HttpTimeout);
    Ok(())
}

#[test]
#[cfg(unix)]
fn compiler_cache_io_and_file_shape_fail_without_cache_trust_or_network()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = CacheDirectory::new()?;
    let build = solidity_compilation::compiler_build("0.8.7+commit.e28d00a7")?;
    std::fs::create_dir(directory.path().join(build.cache_name()))?;
    let result = compiler_cache::bytes(
        directory.path(),
        &build,
        &AtomicBool::new(false),
        Instant::now(),
    );
    assert_eq!(
        failure(result.err().ok_or("directory cache accepted")?)?.reason(),
        Reason::CacheLocation
    );
    std::fs::remove_dir(directory.path().join(build.cache_name()))?;
    std::os::unix::fs::symlink(
        directory.path().join("absent"),
        directory.path().join(build.cache_name()),
    )?;
    let result = compiler_cache::bytes(
        directory.path(),
        &build,
        &AtomicBool::new(false),
        Instant::now(),
    );
    assert_eq!(
        failure(result.err().ok_or("symlink cache accepted")?)?.reason(),
        Reason::CacheIo
    );
    Ok(())
}

struct CacheDirectory(std::path::PathBuf);
impl CacheDirectory {
    fn new() -> std::io::Result<Self> {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "minutus-source-cache-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for CacheDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn server(
    response: Vec<u8>,
    delay: Duration,
) -> Result<(String, thread::JoinHandle<std::io::Result<()>>), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let handle = thread::spawn(move || {
        let mut stream = accept(&listener)?;
        let mut request = [0; 1024];
        let _ = stream.read(&mut request)?;
        thread::sleep(delay);
        if !response.is_empty() {
            stream.write_all(&response)?;
        }
        Ok(())
    });
    Ok((
        format!("http://{address}/private?credential=secret"),
        handle,
    ))
}

fn accept(listener: &TcpListener) -> std::io::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() >= Duration::from_secs(2) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "test accept deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}
