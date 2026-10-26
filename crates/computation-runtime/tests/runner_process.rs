//! Process-boundary coverage for the bundled one-shot runner.

use std::{thread, time::Duration};

use computation_runtime::{
    ComputationCancellation, ComputationOutput, ComputationRequest, ComputationRunner,
    OutputExpectation, OutputKind, RunnerError, RunnerStatus,
};

fn runner() -> ComputationRunner {
    ComputationRunner::new(env!("CARGO_BIN_EXE_zeta-computation-runner"))
}

fn request(id: &str, source: &str) -> Result<ComputationRequest, Box<dyn std::error::Error>> {
    ComputationRequest::new(
        id,
        source,
        Vec::new(),
        OutputExpectation::new(OutputKind::Json, 4_096, 64)?,
    )
    .map_err(Into::into)
}

#[test]
fn dedicated_process_returns_one_authenticated_result_and_exits()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(
        "process-success",
        "export default function compute() { return { answer: 21 * 2 }; }",
    )?;
    let response = runner().execute(&request, &ComputationCancellation::new())?;

    assert_eq!(response.request_id(), "process-success");
    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(serde_json::json!({"answer": 42})))
    );
    Ok(())
}

#[test]
fn cancellation_terminates_and_reaps_an_infinite_program() -> Result<(), Box<dyn std::error::Error>>
{
    let request = request(
        "process-cancel",
        "export default function compute() { while (true) {} }",
    )?;
    let cancellation = ComputationCancellation::new();
    let worker_cancellation = cancellation.clone();
    let worker = thread::spawn(move || runner().execute(&request, &worker_cancellation));
    thread::sleep(Duration::from_millis(75));
    cancellation.cancel();
    let result = worker.join().map_err(|_| "runner thread panicked")?;

    assert_eq!(result?.status(), RunnerStatus::Cancelled);
    Ok(())
}

#[test]
fn cancellation_before_start_returns_without_launching() -> Result<(), Box<dyn std::error::Error>> {
    let request = request(
        "cancel-before-start",
        "export default function compute() { return 42; }",
    )?;
    let cancellation = ComputationCancellation::new();
    cancellation.cancel();

    assert_eq!(
        runner().execute(&request, &cancellation)?.status(),
        RunnerStatus::Cancelled
    );
    Ok(())
}

#[test]
fn cpu_and_memory_exhaustion_have_typed_outcomes() -> Result<(), Box<dyn std::error::Error>> {
    let cpu = request(
        "cpu-limit",
        "export default function compute() { while (true) {} }",
    )?;
    assert_eq!(
        runner()
            .execute(&cpu, &ComputationCancellation::new())?
            .status(),
        RunnerStatus::CpuLimit
    );

    let memory = request(
        "memory-limit",
        "export default function compute() { const held = []; while (true) { held.push(new Uint8Array(1024 * 1024)); } }",
    )?;
    assert_eq!(
        runner()
            .execute(&memory, &ComputationCancellation::new())?
            .status(),
        RunnerStatus::MemoryLimit
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn host_wall_watchdog_terminates_a_stalled_runner() -> Result<(), Box<dyn std::error::Error>> {
    use std::{fs, os::unix::fs::PermissionsExt, time::Instant};

    let directory = tempfile::tempdir()?;
    let stalled = directory.path().join("stalled-runner");
    fs::write(&stalled, "#!/bin/sh\nexec /bin/sleep 30\n")?;
    fs::set_permissions(&stalled, fs::Permissions::from_mode(0o700))?;
    let request = request(
        "wall-limit",
        "export default function compute() { return 42; }",
    )?;
    let started = Instant::now();
    let response =
        ComputationRunner::new(stalled).execute(&request, &ComputationCancellation::new())?;

    assert_eq!(response.status(), RunnerStatus::WallLimit);
    assert!(started.elapsed() < Duration::from_secs(7));
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn host_memory_watchdog_terminates_a_process_above_the_physical_limit()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{fs, os::unix::fs::PermissionsExt};

    let directory = tempfile::tempdir()?;
    let allocator = directory.path().join("allocating-runner");
    fs::write(
        &allocator,
        "#!/bin/sh\nexec /usr/bin/python3 -c 'x=[]\nwhile True: x.append(bytearray(1048576))'\n",
    )?;
    fs::set_permissions(&allocator, fs::Permissions::from_mode(0o700))?;
    let request = request(
        "host-memory-limit",
        "export default function compute() { return 42; }",
    )?;

    assert_eq!(
        ComputationRunner::new(allocator)
            .execute(&request, &ComputationCancellation::new())?
            .status(),
        RunnerStatus::MemoryLimit
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn crash_and_malformed_protocol_are_sanitized() -> Result<(), Box<dyn std::error::Error>> {
    use std::{fs, os::unix::fs::PermissionsExt};

    let directory = tempfile::tempdir()?;
    let crash = directory.path().join("crash-runner");
    fs::write(&crash, "#!/bin/sh\nexit 9\n")?;
    fs::set_permissions(&crash, fs::Permissions::from_mode(0o700))?;
    let malformed = directory.path().join("malformed-runner");
    fs::write(&malformed, "#!/bin/sh\nprintf '\\001\\000\\000\\000x'\n")?;
    fs::set_permissions(&malformed, fs::Permissions::from_mode(0o700))?;
    let request = request(
        "process-failure",
        "export default function compute() { return 42; }",
    )?;

    assert_eq!(
        ComputationRunner::new(crash).execute(&request, &ComputationCancellation::new()),
        Err(RunnerError::RunnerFailed)
    );
    assert_eq!(
        ComputationRunner::new(malformed).execute(&request, &ComputationCancellation::new()),
        Err(RunnerError::InvalidProtocol)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_crashed_or_malformed_runner_does_not_poison_the_next_launch()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{fs, os::unix::fs::PermissionsExt};

    let directory = tempfile::tempdir()?;
    let crash = directory.path().join("crash-on-restart");
    fs::write(&crash, "#!/bin/sh\nexit 9\n")?;
    fs::set_permissions(&crash, fs::Permissions::from_mode(0o700))?;
    let request = request(
        "restart-recovery",
        "export default function compute() { return 42; }",
    )?;
    assert_eq!(
        ComputationRunner::new(crash).execute(&request, &ComputationCancellation::new()),
        Err(RunnerError::RunnerFailed)
    );

    let recovered = runner().execute(&request, &ComputationCancellation::new())?;
    assert_eq!(recovered.status(), RunnerStatus::Completed);
    assert_eq!(
        recovered.output(),
        Some(&ComputationOutput::Json(serde_json::json!(42)))
    );
    Ok(())
}

#[test]
fn wrong_launch_token_is_rejected_without_a_response() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
    };

    let request = request(
        "wrong-token",
        "export default function compute() { return 42; }",
    )?;
    let payload = serde_json::to_vec(&serde_json::json!({
        "protocolVersion": 1,
        "launchToken": "ab".repeat(32),
        "request": request
    }))?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_zeta-computation-runner"))
        .env_clear()
        .env("ZETA_COMPUTATION_LAUNCH_TOKEN", "cd".repeat(32))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("missing child input")?;
    stdin.write_all(&u32::try_from(payload.len())?.to_le_bytes())?;
    stdin.write_all(&payload)?;
    drop(stdin);
    let mut stdout = child.stdout.take().ok_or("missing child output")?;
    let mut response = Vec::new();
    stdout.read_to_end(&mut response)?;
    let exit = child.wait()?;

    assert!(!exit.success());
    assert!(response.is_empty());
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn runner_launch_clears_the_ambient_environment() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    let directory = tempfile::tempdir()?;
    let source = directory.path().join("environment-probe.c");
    let executable = directory.path().join("environment-probe");
    let capture = directory.path().join("environment.txt");
    let capture_literal = capture.to_string_lossy().replace('"', "\\\"");
    fs::write(
        &source,
        format!(
            "#include <stdio.h>\nextern char **environ;\nint main(void) {{ FILE *f=fopen(\"{capture_literal}\",\"w\"); if(!f) return 2; for(char **e=environ; *e; ++e) fprintf(f,\"%s\\n\",*e); return fclose(f); }}\n"
        ),
    )?;
    assert!(
        std::process::Command::new("/usr/bin/clang")
            .args(["-Os", "-o"])
            .arg(&executable)
            .arg(&source)
            .status()?
            .success()
    );
    let request = request(
        "environment-probe",
        "export default function compute() { return 42; }",
    )?;
    assert!(matches!(
        ComputationRunner::new(executable).execute(&request, &ComputationCancellation::new()),
        Err(RunnerError::RunnerFailed | RunnerError::InvalidProtocol)
    ));
    let environment = fs::read_to_string(capture)?;
    let entries = environment.lines().collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].starts_with("ZETA_COMPUTATION_LAUNCH_TOKEN="));
    assert_eq!(
        entries[0].len(),
        "ZETA_COMPUTATION_LAUNCH_TOKEN=".len() + 64
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires scripts/test-computation-xpc.sh to build and sign the packaged XPC fixture"]
fn packaged_xpc_service_executes_in_its_independent_sandbox()
-> Result<(), Box<dyn std::error::Error>> {
    let broker = std::env::var_os("ZETA_XPC_SMOKE_BROKER")
        .ok_or("ZETA_XPC_SMOKE_BROKER is required; run scripts/test-computation-xpc.sh")?;
    let success_request = request(
        "packaged-xpc-smoke",
        "export default function compute() { return { transport: 'xpc', answer: 6 * 7 }; }",
    )?;
    let response = ComputationRunner::new(&broker)
        .execute(&success_request, &ComputationCancellation::new())
        .map_err(|error| format!("packaged XPC success launch failed: {error}"))?;

    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(
            serde_json::json!({"transport": "xpc", "answer": 42})
        ))
    );

    let cpu = request(
        "packaged-xpc-cpu",
        "export default function compute() { while (true) {} }",
    )?;
    assert!(matches!(
        ComputationRunner::new(&broker)
            .execute(&cpu, &ComputationCancellation::new())
            .map_err(|error| format!("packaged XPC CPU launch failed: {error}"))?
            .status(),
        RunnerStatus::CpuLimit | RunnerStatus::WallLimit
    ));

    let memory = request(
        "packaged-xpc-memory",
        "export default function compute() { const held = []; while (true) { held.push(new Uint8Array(1024 * 1024)); } }",
    )?;
    assert_eq!(
        ComputationRunner::new(&broker)
            .execute(&memory, &ComputationCancellation::new())
            .map_err(|error| format!("packaged XPC memory launch failed: {error}"))?
            .status(),
        RunnerStatus::MemoryLimit
    );

    let cancellation = ComputationCancellation::new();
    let worker_cancellation = cancellation.clone();
    let worker_broker = broker.clone();
    let cancelled_request = request(
        "packaged-xpc-cancel",
        "export default function compute() { while (true) {} }",
    )?;
    let worker = thread::spawn(move || {
        ComputationRunner::new(worker_broker).execute(&cancelled_request, &worker_cancellation)
    });
    thread::sleep(Duration::from_millis(75));
    cancellation.cancel();
    assert_eq!(
        worker
            .join()
            .map_err(|_| "packaged XPC worker panicked")??
            .status(),
        RunnerStatus::Cancelled
    );

    let recovered = ComputationRunner::new(broker)
        .execute(&success_request, &ComputationCancellation::new())
        .map_err(|error| format!("packaged XPC recovery launch failed: {error}"))?;
    assert_eq!(recovered.status(), RunnerStatus::Completed);
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires scripts/test-computation-sandbox-host.sh stress"]
fn packaged_xpc_child_accounting_survives_repeated_fast_exits()
-> Result<(), Box<dyn std::error::Error>> {
    let broker = std::env::var_os("ZETA_XPC_SMOKE_BROKER").ok_or("signed broker required")?;
    let request = request(
        "repeated-child",
        "export default function compute() { return 42; }",
    )?;
    for index in 0..100 {
        let response =
            ComputationRunner::new(&broker).execute(&request, &ComputationCancellation::new())?;
        assert_eq!(
            response.status(),
            RunnerStatus::Completed,
            "iteration {index}"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires scripts/test-computation-sandbox-host.sh and its signed native worker fixtures"]
fn packaged_xpc_native_worker_limits_are_independent_of_the_interpreter()
-> Result<(), Box<dyn std::error::Error>> {
    let broker = std::env::var_os("ZETA_XPC_SMOKE_BROKER").ok_or("signed broker required")?;
    let mode = std::env::var("ZETA_XPC_RESOURCE_MODE")?;
    let source = format!(
        "/*{}*/ export default function compute() {{ return 42; }}",
        "\u{1}".repeat(60_000)
    );
    let request = request("native-resource-probe", &source)?;
    let result = ComputationRunner::new(broker).execute(&request, &ComputationCancellation::new());
    match mode.as_str() {
        "cpu" => assert_eq!(result?.status(), RunnerStatus::CpuLimit),
        "memory" => assert_eq!(result?.status(), RunnerStatus::MemoryLimit),
        "wall" => assert_eq!(result?.status(), RunnerStatus::WallLimit),
        "early_exit" => assert!(matches!(
            result,
            Err(RunnerError::RunnerFailed | RunnerError::InvalidProtocol)
        )),
        "closed_input" => assert_eq!(result, Err(RunnerError::RunnerFailed)),
        _ => return Err("unknown fixed resource probe mode".into()),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires scripts/test-computation-xpc-containment.sh to build and sign native probes"]
fn packaged_xpc_native_probe_has_no_ambient_authority() -> Result<(), Box<dyn std::error::Error>> {
    let broker = std::env::var_os("ZETA_XPC_CONTAINMENT_BROKER").ok_or(
        "ZETA_XPC_CONTAINMENT_BROKER is required; run scripts/test-computation-xpc-containment.sh",
    )?;
    let baseline = std::env::var_os("ZETA_XPC_SMOKE_BROKER")
        .ok_or("ZETA_XPC_SMOKE_BROKER is required to qualify the signed XPC baseline")?;
    // A generic launch failure is not evidence of confinement. Establish that
    // this environment can execute the signed transport before accepting an OS
    // termination of the deliberately hostile native runner below.
    let baseline_request = request(
        "containment-baseline",
        "export default function compute() { return 42; }",
    )?;
    let baseline_response = ComputationRunner::new(baseline)
        .execute(&baseline_request, &ComputationCancellation::new())?;
    assert_eq!(baseline_response.status(), RunnerStatus::Completed);
    assert_eq!(
        baseline_response.output(),
        Some(&ComputationOutput::Json(serde_json::json!(42)))
    );
    let probe_request = request(
        "packaged-xpc-containment",
        "export default function compute() { return {}; }",
    )?;
    let response = match ComputationRunner::new(broker)
        .execute(&probe_request, &ComputationCancellation::new())
    {
        Ok(response) => response,
        Err(RunnerError::RunnerFailed) => {
            // Some App Sandbox denials terminate the deliberately hostile
            // native probe rather than returning an errno. The signed baseline
            // XPC probe runs immediately before this test, so a post-launch
            // termination is itself a contained outcome.
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    assert_eq!(response.status(), RunnerStatus::Completed);
    assert_eq!(
        response.output(),
        Some(&ComputationOutput::Json(serde_json::json!({
            "filesystemAllowed": false,
            "symlinkAllowed": false,
            "networkClientAllowed": false,
            "networkServerAllowed": false,
            "processAllowed": false,
            "dynamicLoadingAllowed": false,
            "processPolicyApplied": true,
            "environmentCount": 1
        })))
    );
    Ok(())
}
