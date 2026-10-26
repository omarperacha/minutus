use std::io::Cursor;

use serde_json::json;

use crate::{
    ComputationInput, ComputationOutput, ComputationRequest, InputFormat, OutputExpectation,
    OutputKind, RunnerError, RunnerStatus,
    contract::{MAX_INPUT_BYTES, MAX_SOURCE_BYTES},
    engine,
    protocol::{RequestEnvelope, decode_response, encode_request},
};

fn request(
    source: &str,
    kind: OutputKind,
    max_bytes: usize,
) -> Result<ComputationRequest, crate::ContractError> {
    let max_items = if kind == OutputKind::Text { 1 } else { 64 };
    ComputationRequest::new(
        "request-1",
        source,
        Vec::new(),
        OutputExpectation::new(kind, max_bytes, max_items)?,
    )
}

#[test]
fn contract_rejects_invalid_source_input_and_output_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    let valid_output = OutputExpectation::new(OutputKind::Text, 1, 1)?;
    assert!(ComputationRequest::new("request-1", "", Vec::new(), valid_output.clone()).is_err());
    assert!(
        ComputationRequest::new(
            "request-1",
            "x".repeat(MAX_SOURCE_BYTES + 1),
            Vec::new(),
            valid_output,
        )
        .is_err()
    );
    assert!(ComputationInput::new("1bad", InputFormat::Bytes, b"x").is_err());
    assert!(ComputationInput::new("data", InputFormat::Utf8, &[0xff]).is_err());
    assert!(ComputationInput::new("data", InputFormat::Json, b"{").is_err());
    assert!(
        ComputationInput::new("data", InputFormat::Bytes, &vec![0_u8; MAX_INPUT_BYTES + 1],)
            .is_err()
    );
    assert!(OutputExpectation::new(OutputKind::Text, 1, 2).is_err());
    assert!(OutputExpectation::new(OutputKind::Json, 0, 1).is_err());
    Ok(())
}

#[test]
fn engine_runs_pure_computation_and_preserves_output_kind() -> Result<(), Box<dyn std::error::Error>>
{
    let json_request = request(
        "export default function compute() { return { answer: 6 * 7 }; }",
        OutputKind::Json,
        1_024,
    )?;
    let response = engine::execute(&json_request);
    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(json!({"answer": 42})))
    );

    let text_request = request(
        "export default function compute() { return 'forty-two'; }",
        OutputKind::Text,
        32,
    )?;
    let response = engine::execute(&text_request);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Text("forty-two".to_owned()))
    );
    Ok(())
}

#[test]
fn engine_exposes_only_typed_bound_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let inputs = vec![
        ComputationInput::new("text", InputFormat::Utf8, b"alpha")?,
        ComputationInput::new("data", InputFormat::Json, br#"{"count": 2}"#)?,
        ComputationInput::new("bytes", InputFormat::Bytes, &[3, 4, 5])?,
    ];
    let request = ComputationRequest::new(
        "request-inputs",
        r#"
          export default function compute(zeta) {
            const bytes = zeta.inputBytes("bytes");
            return {
              text: zeta.inputText("text"),
              count: zeta.inputJson("data").count,
              sum: bytes[0] + bytes[1] + bytes[2]
            };
          }
        "#,
        inputs,
        OutputExpectation::new(OutputKind::Json, 1_024, 16)?,
    )?;
    let response = engine::execute(&request);

    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(
            json!({"text": "alpha", "count": 2, "sum": 12})
        ))
    );
    Ok(())
}

#[test]
fn engine_removes_ambient_and_nondeterministic_javascript_apis()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(
        r#"
          export default function compute() {
            let random;
            try { Math.random(); random = "available"; }
            catch (_) { random = "blocked"; }
            return {
              process: typeof process,
              require: typeof require,
              fetch: typeof fetch,
              console: typeof console,
              date: typeof Date,
              webAssembly: typeof WebAssembly,
              sharedMemory: typeof SharedArrayBuffer,
              random
            };
          }
        "#,
        OutputKind::Json,
        2_048,
    )?;
    let response = engine::execute(&request);

    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(json!({
            "process": "undefined",
            "require": "undefined",
            "fetch": "undefined",
            "console": "undefined",
            "date": "undefined",
            "webAssembly": "undefined",
            "sharedMemory": "undefined",
            "random": "blocked"
        })))
    );
    Ok(())
}

#[test]
fn engine_has_no_filesystem_network_process_or_dynamic_loading_escape_surface()
-> Result<(), Box<dyn std::error::Error>> {
    let absent = request(
        r#"
          export default function compute(zeta) {
            const names = [
              "process", "require", "Deno", "Bun", "fetch", "XMLHttpRequest",
              "WebSocket", "Worker", "WebAssembly", "load", "readFile", "std", "os"
            ];
            const types = Object.fromEntries(names.map(name => [name, typeof globalThis[name]]));
            let traversal = "not-attempted";
            try { zeta.inputText("../outside-secret"); }
            catch (_) { traversal = "blocked"; }
            return { types, traversal };
          }
        "#,
        OutputKind::Json,
        4_096,
    )?;
    let response = engine::execute(&absent);
    assert_eq!(response.status(), RunnerStatus::Completed);
    let Some(ComputationOutput::Json(output)) = response.output() else {
        return Err("missing security probe output".into());
    };
    assert_eq!(output["traversal"], "blocked");
    let types = output["types"].as_object().ok_or("missing surface map")?;
    assert!(types.values().all(|value| value == "undefined"));

    for source in [
        "import value from '/tmp/host-secret.js'; export default function compute() { return value; }",
        "export default async function compute() { return import('./native-addon.js'); }",
        "export default function compute() { return Function('return process')(); }",
    ] {
        assert_ne!(
            engine::execute(&request(source, OutputKind::Json, 1_024)?).status(),
            RunnerStatus::Completed
        );
    }
    Ok(())
}

#[test]
fn engine_rejects_imports_async_results_and_output_flooding()
-> Result<(), Box<dyn std::error::Error>> {
    let import = request(
        "import value from './other.js'; export default function compute() { return value; }",
        OutputKind::Json,
        1_024,
    )?;
    assert_ne!(engine::execute(&import).status(), RunnerStatus::Completed);

    let asynchronous = request(
        "export default async function compute() { return 42; }",
        OutputKind::Json,
        1_024,
    )?;
    assert_eq!(
        engine::execute(&asynchronous).status(),
        RunnerStatus::RuntimeFailed
    );

    let flooding = request(
        "export default function compute() { return 'x'.repeat(1000); }",
        OutputKind::Text,
        16,
    )?;
    assert_eq!(
        engine::execute(&flooding).status(),
        RunnerStatus::OutputLimit
    );

    let too_many_items = ComputationRequest::new(
        "request-items",
        "export default function compute() { return [1, 2, 3]; }",
        Vec::new(),
        OutputExpectation::new(OutputKind::Json, 1_024, 2)?,
    )?;
    assert_eq!(
        engine::execute(&too_many_items).status(),
        RunnerStatus::OutputLimit
    );
    Ok(())
}

#[test]
fn protocol_rejects_truncated_oversized_and_non_json_frames() {
    assert!(matches!(
        decode_response(Cursor::new([]), &"ab".repeat(32)),
        Err(RunnerError::InvalidProtocol)
    ));
    assert!(matches!(
        decode_response(Cursor::new([1, 0, 0, 0, b'{']), &"ab".repeat(32)),
        Err(RunnerError::InvalidProtocol)
    ));
    assert!(matches!(
        decode_response(Cursor::new(u32::MAX.to_le_bytes()), &"ab".repeat(32)),
        Err(RunnerError::InvalidProtocol)
    ));

    let payload = serde_json::to_vec(&json!({
        "protocolVersion": 1,
        "launchToken": "ab".repeat(32),
        "response": {
            "requestId": "request-1",
            "runtimeVersion": crate::RUNTIME_VERSION,
            "status": "runtimeFailed",
            "output": null
        }
    }));
    assert!(payload.is_ok());
    if let Ok(payload) = payload {
        let mut frame = Vec::from(u32::try_from(payload.len()).unwrap_or(0).to_le_bytes());
        frame.extend_from_slice(&payload);
        assert!(matches!(
            decode_response(Cursor::new(frame), &"cd".repeat(32)),
            Err(RunnerError::InvalidProtocol)
        ));
    }
}

#[test]
fn request_protocol_is_bounded_and_contains_no_executable_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(
        "export default function compute() { return 42; }",
        OutputKind::Json,
        1_024,
    )?;
    let envelope = RequestEnvelope::new("ab".repeat(32), request);
    let mut frame = Vec::new();
    encode_request(&envelope, &mut frame)?;
    let text = String::from_utf8_lossy(&frame[4..]);

    assert!(!text.contains("executable"));
    assert!(!text.contains("environment"));
    assert!(!text.contains("workingDirectory"));
    assert!(!text.contains("shell"));
    Ok(())
}
