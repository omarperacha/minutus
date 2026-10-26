use std::{
    error::Error,
    fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    sync::mpsc,
    thread,
};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const TYPE_UINT32: u32 = 4;
const TYPE_STRING: u32 = 8;

fn append_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), io::Error> {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .map_err(io::Error::other)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn append_string_metadata(bytes: &mut Vec<u8>, key: &str, value: &str) -> Result<(), io::Error> {
    append_string(bytes, key)?;
    bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
    append_string(bytes, value)
}

fn append_u32_metadata(bytes: &mut Vec<u8>, key: &str, value: u32) -> Result<(), io::Error> {
    append_string(bytes, key)?;
    bytes.extend_from_slice(&TYPE_UINT32.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn valid_gguf() -> Result<Vec<u8>, io::Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&5_u64.to_le_bytes());
    append_string_metadata(&mut bytes, "general.architecture", "muse-glimmer")?;
    append_string_metadata(&mut bytes, "general.type", "model")?;
    append_string_metadata(&mut bytes, "general.name", "Lifecycle Test")?;
    append_u32_metadata(&mut bytes, "muse-glimmer.context_length", 131_072)?;
    append_u32_metadata(&mut bytes, "general.alignment", 32)?;
    append_string(&mut bytes, "token_embd.weight")?;
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    while bytes.len() % 32 != 0 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    Ok(bytes)
}

fn fixture() -> Result<(TempDir, ModelLoadConfig), Box<dyn Error>> {
    let directory = TempDir::new()?;
    let executable = directory.path().join("llama-server");
    let model = directory.path().join("model.gguf");
    fs::write(&executable, b"helper")?;
    let bytes = valid_gguf()?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    fs::write(&model, &bytes)?;
    let config = ModelLoadConfig::new(executable, model, digest, u64::try_from(bytes.len())?)
        .with_limits(32_768, 4_096)
        .with_backend_compatibility_version(PINNED_LLAMA_CPP_VERSION);
    Ok((directory, config))
}

fn test_binding() -> crate::GenerationBinding {
    crate::GenerationBinding::glimmer_llama_cpp(
        "test/glimmer",
        "revision-1",
        "0".repeat(64),
        PINNED_LLAMA_CPP_VERSION,
        crate::ModelCapabilities::glimmer_runtime(32_768, true),
    )
}

#[test]
fn load_gate_rehashes_and_inspects_managed_model() -> Result<(), Box<dyn Error>> {
    let (_directory, config) = fixture()?;
    let verified = verify(&config)?;
    assert_eq!(verified.context_tokens, 32_768);
    assert_eq!(verified.output_tokens, 4_096);

    let mut wrong_digest = config.clone();
    wrong_digest.expected_sha256 = "0".repeat(64);
    assert!(matches!(
        verify(&wrong_digest),
        Err(InferenceError::ArtifactDigestMismatch)
    ));
    Ok(())
}

#[test]
fn load_gate_rejects_backend_and_limit_mismatches() -> Result<(), Box<dyn Error>> {
    let (_directory, config) = fixture()?;
    let incompatible = config
        .clone()
        .with_backend_compatibility_version("other-revision");
    assert!(matches!(
        verify(&incompatible),
        Err(InferenceError::IncompatibleBackend)
    ));
    let excessive_context = config.with_limits(131_073, 4_096);
    assert!(matches!(
        verify(&excessive_context),
        Err(InferenceError::ContextLimitExceeded)
    ));
    Ok(())
}

#[test]
fn load_gate_rehashes_companions_and_enforces_exact_compatibility() -> Result<(), Box<dyn Error>> {
    let (directory, config) = fixture()?;
    let companion_path = directory.path().join("projector.gguf");
    let companion_bytes = b"independently verified projector";
    fs::write(&companion_path, companion_bytes)?;
    let digest = format!("{:x}", Sha256::digest(companion_bytes));
    let companion = CompanionLoadConfig::new(
        crate::CompanionKind::VisionProjector,
        companion_path,
        digest,
        u64::try_from(companion_bytes.len())?,
        "release-1",
    );
    let verified = verify(
        &config
            .clone()
            .with_companions("release-1", vec![companion.clone()]),
    )?;
    assert_eq!(verified.companions.len(), 1);

    let incompatible = config
        .clone()
        .with_companions("release-2", vec![companion.clone()]);
    assert!(matches!(
        verify(&incompatible),
        Err(InferenceError::IncompatibleCompanion)
    ));

    let duplicate = config.with_companions("release-1", vec![companion.clone(), companion]);
    assert!(matches!(
        verify(&duplicate),
        Err(InferenceError::DuplicateCompanionKind)
    ));
    Ok(())
}

#[test]
fn multimodal_request_enforces_normalized_png_count_and_marker_alignment()
-> Result<(), Box<dyn Error>> {
    let png = b"\x89PNG\r\n\x1a\nvalidated derivative".to_vec();
    let valid = GenerationRequest::new(
        "image-generation",
        format!("describe {marker}", marker = types::GLIMMER_IMAGE_MARKER),
        512,
        test_binding(),
    )
    .with_normalized_images(vec![png.clone()])?;
    assert_eq!(valid.images.len(), 1);

    let mismatch = GenerationRequest::new("mismatch", "no marker", 512, test_binding())
        .with_normalized_images(vec![png.clone()]);
    assert!(matches!(mismatch, Err(InferenceError::ImageMarkerMismatch)));
    let unsupported = GenerationRequest::new(
        "unsupported",
        types::GLIMMER_IMAGE_MARKER,
        512,
        test_binding(),
    )
    .with_normalized_images(vec![b"https://example.test/image.png".to_vec()]);
    assert!(matches!(
        unsupported,
        Err(InferenceError::InvalidImageInput)
    ));
    let too_many = GenerationRequest::new(
        "too-many",
        types::GLIMMER_IMAGE_MARKER.repeat(MAX_GENERATION_IMAGES + 1),
        512,
        test_binding(),
    )
    .with_normalized_images(vec![png; MAX_GENERATION_IMAGES + 1]);
    assert!(matches!(too_many, Err(InferenceError::ImageCountExceeded)));
    Ok(())
}

#[test]
fn multimodal_generation_sends_inline_verified_bytes_and_completes() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let std::net::SocketAddr::V4(address) = listener.local_addr()? else {
        return Err(io::Error::other("test listener is not IPv4").into());
    };
    let (body_sender, body_receiver) = mpsc::channel();
    let server = thread::spawn(move || -> io::Result<()> {
        let (mut tokenize, _) = listener.accept()?;
        body_sender
            .send(read_http_body(&mut tokenize)?)
            .map_err(io::Error::other)?;
        tokenize.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"tokens\":[1,2,3]}",
        )?;

        let (mut completion, _) = listener.accept()?;
        body_sender
            .send(read_http_body(&mut completion)?)
            .map_err(io::Error::other)?;
        let stream = concat!(
            "data: {\"content\":\"<|start|>assistant to=user<|message|>Vision works<|eot|>\",\"stop\":false}\n\n",
            "data: {\"content\":\"\",\"stop\":true,\"stop_type\":\"eos\",\"truncated\":false}\n\n"
        );
        write!(
            completion,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{stream}",
            stream.len()
        )?;
        Ok(())
    });

    let runtime = LocalInferenceRuntime::new()?;
    let (_directory, config) = fixture()?;
    let binding = test_binding();
    runtime
        .state
        .lock()
        .map_err(|_| io::Error::other("state lock"))?
        .loaded = Some(LoadedModel {
        config,
        endpoint: crate::SidecarEndpoint::test_endpoint(SocketAddrV4::new(
            *address.ip(),
            address.port(),
        ))?,
        context_tokens: 32_768,
        output_tokens: 4_096,
        image_tokens: Some(GLIMMER_IMAGE_TOKENS),
        binding: binding.clone(),
    });
    let png = b"\x89PNG\r\n\x1a\nverified derivative".to_vec();
    let request = GenerationRequest::new(
        "vision-success",
        format!("describe {}", types::GLIMMER_IMAGE_MARKER),
        32,
        binding,
    )
    .with_normalized_images(vec![png])?;
    let mut events = Vec::new();
    runtime.start_generation(&request, |event| events.push(event))?;

    let tokenize_body = body_receiver.recv()?;
    let completion_body = body_receiver.recv()?;
    server
        .join()
        .map_err(|_| io::Error::other("test server panicked"))??;
    assert!(tokenize_body.contains(types::GLIMMER_IMAGE_MARKER));
    assert!(completion_body.contains(crate::sidecar::MEDIA_MARKER));
    assert!(completion_body.contains("multimodal_data"));
    assert!(!completion_body.contains("http:"));
    assert!(!completion_body.contains("https:"));
    assert!(!completion_body.contains("file:"));
    assert!(events.iter().any(|event| matches!(
        event,
        GenerationEvent::FinalContentDelta { text } if text == "Vision works"
    )));
    assert!(matches!(
        events.last(),
        Some(GenerationEvent::Completed {
            reason: crate::GenerationCompletion::Stop
        })
    ));
    Ok(())
}

#[test]
fn admission_rejects_a_stale_generation_binding_before_provider_io() -> Result<(), Box<dyn Error>> {
    let runtime = LocalInferenceRuntime::new()?;
    let (_directory, config) = fixture()?;
    let loaded_binding = test_binding();
    runtime
        .state
        .lock()
        .map_err(|_| io::Error::other("state lock"))?
        .loaded = Some(LoadedModel {
        config,
        endpoint: crate::SidecarEndpoint::test_endpoint(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9))?,
        context_tokens: 32_768,
        output_tokens: 4_096,
        image_tokens: None,
        binding: loaded_binding.clone(),
    });
    let stale_binding = crate::GenerationBinding::glimmer_llama_cpp(
        "test/glimmer",
        "different-revision",
        "1".repeat(64),
        PINNED_LLAMA_CPP_VERSION,
        crate::ModelCapabilities::glimmer_runtime(32_768, false),
    );
    assert!(matches!(
        runtime.tokenize_prompt(&stale_binding, "hello"),
        Err(InferenceError::ProfileBindingMismatch)
    ));
    let request = GenerationRequest::new("stale-binding", "hello", 32, stale_binding);
    assert!(matches!(
        runtime.start_generation(&request, |_| {}),
        Err(InferenceError::ProfileBindingMismatch)
    ));
    assert_eq!(runtime.active_profile()?.binding(), &loaded_binding);
    assert!(
        runtime
            .state
            .lock()
            .map_err(|_| io::Error::other("state lock"))?
            .active
            .is_none()
    );
    Ok(())
}

fn read_http_body(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "missing HTTP body",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(io::Error::other)?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .ok_or_else(|| io::Error::other("missing content length"))?
        .parse::<usize>()
        .map_err(io::Error::other)?;
    while bytes.len().saturating_sub(header_end) < content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated HTTP body",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
        .map_err(io::Error::other)
}

#[test]
fn restart_window_is_strict_and_recovers_after_expiry() -> Result<(), Box<dyn Error>> {
    let now = Instant::now();
    let mut attempts = VecDeque::new();
    for _ in 0..MAX_RESTARTS {
        reserve_restart(&mut attempts, now)?;
    }
    assert!(matches!(
        reserve_restart(&mut attempts, now),
        Err(InferenceError::RestartLimitExceeded)
    ));
    reserve_restart(
        &mut attempts,
        now + RESTART_WINDOW + Duration::from_millis(1),
    )?;
    assert_eq!(attempts.len(), 1);
    Ok(())
}

#[test]
fn unloaded_runtime_handles_explicit_pressure_without_side_effects() -> Result<(), Box<dyn Error>> {
    let runtime = LocalInferenceRuntime::new()?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    runtime.handle_memory_pressure(MemoryPressureLevel::Warning)?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    runtime.handle_memory_pressure(MemoryPressureLevel::Critical)?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    runtime.handle_memory_pressure(MemoryPressureLevel::Normal)?;
    Ok(())
}

#[test]
fn critical_pressure_discards_the_multimodal_profile() -> Result<(), Box<dyn Error>> {
    let runtime = LocalInferenceRuntime::new()?;
    let (_directory, config) = fixture()?;
    runtime
        .state
        .lock()
        .map_err(|_| io::Error::other("state lock"))?
        .loaded = Some(LoadedModel {
        config,
        endpoint: crate::SidecarEndpoint::test_endpoint(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9))?,
        context_tokens: 32_768,
        output_tokens: 4_096,
        image_tokens: Some(GLIMMER_IMAGE_TOKENS),
        binding: test_binding(),
    });
    assert_eq!(
        runtime.status()?,
        InferenceRuntimeStatus::Loaded {
            context_tokens: 32_768,
            output_tokens: 4_096,
            multimodal: true,
        }
    );
    runtime.handle_memory_pressure(MemoryPressureLevel::Critical)?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    Ok(())
}
