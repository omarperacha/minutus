//! Build-time configuration for Zeta's Tauri command allow-list.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    watch_runtime_sources();
    verify_release_computation_runner()?;
    verify_release_ollama_payload()?;
    verify_release_whisper_helper()?;
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "wallet_state",
            "wallet_inventory",
            "select_wallet",
            "create_wallet",
            "restore_wallet",
            "import_wallet_private_key",
            "unlock_wallet",
            "lock_wallet",
            "change_wallet_password",
            "remove_wallet_account",
            "delete_wallet_vault",
            "reset_wallet",
            "reset_wallet_private_key",
            "release_updates_configured",
            "check_release_update",
            "save_verified_release_update",
            "app_status",
            "list_projects",
            "create_project",
            "list_project_workspaces",
            "add_project_workspace",
            "remove_project_workspace",
            "update_project_workspace_permission",
            "list_conversations",
            "load_conversation",
            "create_conversation",
            "rename_conversation",
            "delete_conversation",
            "filesystem_list",
            "filesystem_stat",
            "filesystem_read_text",
            "filesystem_search",
            "filesystem_undo_available",
            "undo_filesystem_write",
            "start_chat_generation",
            "stop_chat_generation",
            "pick_chat_images",
            "import_chat_image",
            "managed_image_preview",
            "discard_chat_image",
            "speech_models",
            "install_speech_model",
            "cancel_speech_download",
            "remove_speech_model",
            "start_dictation",
            "finish_dictation",
            "cancel_dictation",
            "dictation_events",
            "model_catalogue",
            "model_preferences",
            "set_default_model",
            "project_model",
            "set_project_model",
            "conversation_model",
            "set_conversation_model",
            "download_catalogue_model",
            "cancel_model_download",
            "register_external_model",
            "download_model_module",
            "forget_external_model",
            "brave_search_configured",
            "configure_brave_search",
            "remove_brave_search",
            "evm_rpc_settings",
            "configure_evm_rpc",
            "remove_evm_rpc",
            "evm_balances",
            "decode_evm_authorization",
            "prepare_evm_native_transfer",
            "prepare_evm_token_transfer",
            "review_evm_transaction",
            "review_x402_payment",
        ]),
    ))?;
    Ok(())
}

fn watch_runtime_sources() {
    println!("cargo:rerun-if-changed=icons/icon.icns");
    println!("cargo:rerun-if-changed=release-updater.pub");
    println!("cargo:rerun-if-changed=ollama-runtime/runtime.lock");
    println!("cargo:rerun-if-changed=ollama-runtime/payload.sha256");
    println!("cargo:rerun-if-changed=ollama-runtime/qwen-accel/payload.json");
    println!("cargo:rerun-if-changed=ComputationRunner.entitlements.plist");
    println!("cargo:rerun-if-changed=ComputationBroker.entitlements.plist");
    println!("cargo:rerun-if-changed=ComputationService.entitlements.plist");
    println!("cargo:rerun-if-changed=computation-xpc/broker.c");
    println!("cargo:rerun-if-changed=computation-xpc/service.c");
    println!("cargo:rerun-if-changed=computation-xpc/worker.c");
    println!("cargo:rerun-if-changed=computation-xpc/worker.h");
    println!("cargo:rerun-if-changed=computation-xpc/containment_probe.c");
    println!("cargo:rerun-if-changed=computation-xpc/Info.plist");
    println!("cargo:rerun-if-changed=computation-service/payload.sha256");
    println!("cargo:rerun-if-changed=computation-runtime.lock");
    println!("cargo:rerun-if-changed=speech-runtime/runtime.lock");
    println!("cargo:rerun-if-changed=WhisperHelper.entitlements.plist");
    println!("cargo:rerun-if-changed=Info.plist");
    println!("cargo:rerun-if-changed=../scripts/asr/helper.cpp");
    println!("cargo:rerun-if-changed=../scripts/asr/CMakeLists.txt");
    println!("cargo:rerun-if-changed=../scripts/asr/build_helper.py");
    println!("cargo:rerun-if-changed=../scripts/asr/stage_helper.py");
}

fn verify_release_whisper_helper() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROFILE").as_deref() != Some(std::ffi::OsStr::new("release"))
        || std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(std::ffi::OsStr::new("macos"))
    {
        return Ok(());
    }
    let target = std::env::var("TARGET")?;
    if target != "aarch64-apple-darwin" {
        return Err("the qualified macOS Whisper package target is Apple Silicon".into());
    }
    let helper = std::path::Path::new("binaries").join(format!("zeta-whisper-helper-{target}"));
    verify_regular_digest(&helper)?;
    verify_arm64(&helper)?;

    let lock = std::fs::read_to_string("speech-runtime/runtime.lock")?;
    for required in [
        "schema_version=1",
        "runtime_revision=371b5a7561823ab2bb32142d2751e35e7534727b",
        "source_archive_sha256=59f16e008b0fe4f283b5bede96952aed5a8f2e16c2f2f721f263218982a153e7",
        "protocol_version=1",
        "model_sha256=394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        "macos_target=aarch64-apple-darwin",
        "minimum_macos_version=13.3",
    ] {
        if !lock.lines().any(|line| line == required) {
            return Err("the speech runtime lock differs from reviewed policy".into());
        }
    }
    for (key, path) in [
        ("helper_source_sha256", "../scripts/asr/helper.cpp"),
        ("cmake_source_sha256", "../scripts/asr/CMakeLists.txt"),
        ("build_script_sha256", "../scripts/asr/build_helper.py"),
        ("stage_script_sha256", "../scripts/asr/stage_helper.py"),
        (
            "helper_entitlements_sha256",
            "WhisperHelper.entitlements.plist",
        ),
        ("app_info_sha256", "Info.plist"),
        (
            "whisper_cpp_license_sha256",
            "speech-runtime/WHISPER-CPP-LICENSE.txt",
        ),
        (
            "openai_whisper_license_sha256",
            "speech-runtime/OPENAI-WHISPER-LICENSE.txt",
        ),
    ] {
        if sha256(std::path::Path::new(path))? != lock_value(&lock, key)? {
            return Err("a speech helper build input differs from its runtime lock".into());
        }
    }
    Ok(())
}

fn verify_release_computation_runner() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROFILE").as_deref() != Some(std::ffi::OsStr::new("release"))
        || std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(std::ffi::OsStr::new("macos"))
    {
        return Ok(());
    }
    let target = std::env::var("TARGET")?;
    let runner = std::path::Path::new("binaries").join(format!("zeta-computation-runner-{target}"));
    let metadata = std::fs::symlink_metadata(&runner).ok();
    if !metadata.is_some_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink()) {
        return Err(
            "the computation runner is missing; run scripts/build-computation-runner.sh".into(),
        );
    }
    let lock = std::fs::read_to_string("computation-runtime.lock")?;
    for required in [
        "target=aarch64-apple-darwin",
        "transport=xpc-service-v1",
        "contract_version=zeta-contract-1",
        "runtime_version=quickjs-ng-0.15.1+rquickjs-0.12.2+zeta-contract-1",
        "rquickjs_version=0.12.2",
        "max_child_processes=1",
    ] {
        if !lock.lines().any(|line| line == required) {
            return Err("the computation runtime lock differs from reviewed policy".into());
        }
    }
    for (key, path) in [
        (
            "broker_entitlements_sha256",
            "ComputationBroker.entitlements.plist",
        ),
        (
            "service_entitlements_sha256",
            "ComputationService.entitlements.plist",
        ),
        (
            "runner_entitlements_sha256",
            "ComputationRunner.entitlements.plist",
        ),
        ("broker_source_sha256", "computation-xpc/broker.c"),
        ("service_source_sha256", "computation-xpc/service.c"),
        ("worker_source_sha256", "computation-xpc/worker.c"),
        ("worker_header_sha256", "computation-xpc/worker.h"),
        (
            "containment_probe_sha256",
            "computation-xpc/containment_probe.c",
        ),
        ("service_info_sha256", "computation-xpc/Info.plist"),
    ] {
        if sha256(std::path::Path::new(path))? != lock_value(&lock, key)? {
            return Err("a computation XPC source or policy differs from its runtime lock".into());
        }
    }
    let broker = std::path::Path::new("binaries").join(format!("zeta-computation-broker-{target}"));
    for executable in [runner.as_path(), broker.as_path()] {
        verify_regular_digest(executable)?;
        verify_arm64(executable)?;
    }
    verify_computation_service()?;
    Ok(())
}

fn verify_regular_digest(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = std::fs::symlink_metadata(path).ok();
    if !metadata.is_some_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink()) {
        return Err("a computation release executable is missing or indirect".into());
    }
    let digest_manifest = path.with_extension(format!(
        "{}sha256",
        path.extension()
            .map_or("", |extension| extension.to_str().unwrap_or(""))
    ));
    let manifest = std::fs::read_to_string(digest_manifest)
        .map_err(|_| "a computation release digest is missing")?;
    let expected = manifest
        .split_whitespace()
        .next()
        .ok_or("a computation release digest is invalid")?;
    if expected.len() != 64 || sha256(path)? != expected {
        return Err("a computation executable differs from its release digest".into());
    }
    Ok(())
}

fn verify_arm64(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let architectures = std::process::Command::new("lipo")
        .args(["-archs"])
        .arg(path)
        .output()?;
    if !architectures.status.success() || String::from_utf8(architectures.stdout)?.trim() != "arm64"
    {
        return Err("a computation executable is not arm64-only".into());
    }
    Ok(())
}

fn verify_computation_service() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new("computation-service");
    let manifest = std::fs::read_to_string(root.join("payload.sha256"))?;
    let mut expected_files = std::collections::BTreeSet::new();
    for line in manifest.lines() {
        let (digest, path) = line
            .split_once("  ")
            .ok_or("the computation XPC payload manifest is invalid")?;
        if digest.len() != 64 || path.starts_with('/') || path.contains("..") {
            return Err("the computation XPC payload manifest is invalid".into());
        }
        let file = root.join(path);
        if sha256(&file)? != digest {
            return Err("the computation XPC payload differs from its manifest".into());
        }
        expected_files.insert(format!("./{path}"));
    }
    let mut actual_files = std::collections::BTreeSet::new();
    collect_runtime_files(root, root, &mut actual_files)?;
    actual_files.remove("./payload.sha256");
    if actual_files != expected_files {
        return Err("the computation XPC payload file set differs from its manifest".into());
    }
    let service = root
        .join("com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-service");
    let runner = root
        .join("com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-runner");
    verify_arm64(&service)?;
    verify_arm64(&runner)
}

fn lock_value<'a>(lock: &'a str, key: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    let prefix = format!("{key}=");
    lock.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("the computation runtime lock is missing {key}").into())
}

fn sha256(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err("computation release integrity verification failed".into());
    }
    String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "computation release integrity verification returned no digest".into())
}

fn verify_release_ollama_payload() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROFILE").as_deref() != Some(std::ffi::OsStr::new("release"))
        || std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(std::ffi::OsStr::new("macos"))
    {
        return Ok(());
    }

    let runtime = std::path::Path::new("ollama-runtime");
    if !runtime.join("ollama").is_file() || !runtime.join("payload.sha256").is_file() {
        return Err(
            "the verified Ollama runtime is missing; run scripts/fetch-ollama-runtime.sh".into(),
        );
    }
    for file in ["ollama", "llama-server", "payload.json"] {
        if !runtime.join("qwen-accel").join(file).is_file() {
            return Err("the pinned Qwen acceleration payload is missing; run scripts/stage_qwen_acceleration.py".into());
        }
    }
    let manifest = std::fs::read_to_string(runtime.join("payload.sha256"))?;
    let expected_files = manifest
        .lines()
        .filter_map(|line| line.split_once("  ").map(|(_, path)| path.to_owned()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut packaged_files = std::collections::BTreeSet::new();
    collect_runtime_files(runtime, runtime, &mut packaged_files)?;
    packaged_files.remove("./payload.sha256");
    if expected_files != packaged_files {
        return Err("the bundled Ollama file set differs from its pinned manifest".into());
    }
    let verified = std::process::Command::new("shasum")
        .args(["-a", "256", "-c", "payload.sha256"])
        .current_dir(runtime)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success();
    if !verified {
        return Err("the bundled Ollama payload differs from its per-file manifest".into());
    }
    Ok(())
}

fn collect_runtime_files(
    root: &std::path::Path,
    directory: &std::path::Path,
    files: &mut std::collections::BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_runtime_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root)?.to_string_lossy();
            files.insert(format!("./{relative}"));
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&path)?;
            if target.is_absolute()
                || target
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
                || !path.canonicalize()?.starts_with(root.canonicalize()?)
                || !path.canonicalize()?.is_file()
            {
                return Err("the Ollama runtime contains an unsafe symbolic link".into());
            }
            let relative = path.strip_prefix(root)?.to_string_lossy();
            files.insert(format!("./{relative}"));
        } else {
            return Err("the Ollama runtime contains an unsupported filesystem entry".into());
        }
    }
    Ok(())
}
