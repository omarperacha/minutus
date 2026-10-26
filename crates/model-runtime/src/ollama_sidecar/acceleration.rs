//! Bundled retained Qwen payload selection; application hardware admission stays upstream.

use std::{path::Path, path::PathBuf, process::Command};

use super::{OllamaSidecarError, canonical_file};

pub(super) fn executable(stock: &Path) -> Result<PathBuf, OllamaSidecarError> {
    let root = stock
        .parent()
        .ok_or(OllamaSidecarError::InvalidExecutable)?;
    let acceleration = root.join("qwen-accel");
    // A missing or substituted optimized payload must fail before a provider starts.
    // The complete file hashes are checked by staging/build verification; signed
    // application integrity owns final code-signed bytes, which differ from input hashes.
    if acceleration.is_symlink() {
        return Err(OllamaSidecarError::InvalidExecutable);
    }
    for name in ["ollama", "llama-server", "payload.json"] {
        let path = acceleration.join(name);
        if path.is_symlink() || !path.is_file() {
            return Err(OllamaSidecarError::InvalidExecutable);
        }
    }
    canonical_file(&acceleration.join("ollama"))
}

pub(super) fn configure(command: &mut Command) {
    // The pinned public payload owns these selectors. Keep the profile and its
    // checkpoint budget bound to the exact artifact selected by the bundle.
    command
        .env("QWEN_ACCEL_DECODE", "rows4")
        .env("QWEN_ACCEL_DECODE_Q4_ONLY", "1")
        .env("QWEN_ACCEL_STATE_COPY", "1")
        .env("QWEN_ACCEL_DIRECT_STATE", "1")
        .env("QWEN_ACCEL_FP32_SCALE", "0")
        .env("LLAMA_ARG_CHECKPOINT_MIN_SPACING_NT", "2048")
        .env("LLAMA_ARG_CTX_CHECKPOINTS", "8");
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, fs};

    use super::*;

    #[test]
    fn missing_acceleration_cannot_silently_run_stock() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let stock = directory.path().join("ollama");
        fs::write(&stock, "stock")?;
        assert!(executable(&stock).is_err());
        let optimized = directory.path().join("qwen-accel");
        fs::create_dir(&optimized)?;
        for name in ["ollama", "llama-server", "payload.json"] {
            fs::write(optimized.join(name), name)?;
        }
        assert_eq!(
            executable(&stock)?,
            optimized.join("ollama").canonicalize()?
        );
        fs::remove_file(optimized.join("llama-server"))?;
        assert!(executable(&stock).is_err());
        Ok(())
    }

    #[test]
    fn retained_profile_enables_only_the_accepted_variants() {
        let mut command = Command::new("ollama");
        command.env_clear();
        configure(&mut command);
        let environment = command
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(environment.len(), 7);
        for (name, expected) in [
            ("QWEN_ACCEL_DECODE", "rows4"),
            ("QWEN_ACCEL_DECODE_Q4_ONLY", "1"),
            ("QWEN_ACCEL_STATE_COPY", "1"),
            ("QWEN_ACCEL_DIRECT_STATE", "1"),
            ("QWEN_ACCEL_FP32_SCALE", "0"),
            ("LLAMA_ARG_CHECKPOINT_MIN_SPACING_NT", "2048"),
            ("LLAMA_ARG_CTX_CHECKPOINTS", "8"),
        ] {
            assert_eq!(environment[OsStr::new(name)], Some(OsStr::new(expected)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_payload_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let outside = directory.path().join("outside");
        fs::create_dir(&outside)?;
        std::os::unix::fs::symlink(&outside, directory.path().join("qwen-accel"))?;
        assert!(executable(&directory.path().join("ollama")).is_err());
        Ok(())
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore = "requires the staged retained artifact and ZETA_LIVE_OLLAMA_MODELS"]
    fn live_bundled_accelerated_gguf() -> Result<(), Box<dyn std::error::Error>> {
        use std::time::Duration;

        use crate::{OllamaSidecar, OllamaSidecarConfig};

        let models = std::env::var_os("ZETA_LIVE_OLLAMA_MODELS")
            .ok_or("ZETA_LIVE_OLLAMA_MODELS is required")?;
        let stock =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
        let sidecar = OllamaSidecar::new();
        let endpoint = sidecar.start(
            &OllamaSidecarConfig::new(stock, models)
                .with_qwen_acceleration()
                .with_guardian_executable(
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
                ),
        )?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_mins(3))
            .build()?;
        let response: serde_json::Value = client
            .post(format!("http://{}/api/chat", endpoint.address()))
            .json(&serde_json::json!({
                "model": "qwen3.8:27b", "stream": false, "think": false,
                "messages": [{"role": "user", "content": "Count from 1 to 100, separated by spaces. No prose."}],
                "options": {"num_ctx": 32768, "num_predict": 16, "draft_num_predict": 4,
                            "temperature": 0, "seed": 42}
            }))
            .send()?
            .error_for_status()?
            .json()?;
        assert_eq!(response["model"], "qwen3.8:27b");
        assert_eq!(response["done"], true);
        assert_eq!(response["eval_count"], 16);
        assert!(
            !response["message"]["content"]
                .as_str()
                .unwrap_or("")
                .is_empty()
        );
        sidecar.stop()?;
        Ok(())
    }
}
