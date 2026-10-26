//! Startup composition of the measured runtime profile and explicit rollback.

use model_runtime::{OllamaProviderConfig, OllamaSidecarConfig, QwenTuningPolicy};

use crate::AppCoreError;

pub(crate) fn configure_sidecar(
    config: OllamaSidecarConfig,
) -> Result<OllamaSidecarConfig, AppCoreError> {
    let policy = configure(OllamaProviderConfig::default(), true)?;
    // Keep hardware and explicit debug rollback admission identical to the
    // provider's startup tuning policy. External experimental servers own theirs.
    if policy.qwen_tuning() == QwenTuningPolicy::M1ProBaselineV1 {
        Ok(config.with_qwen_acceleration())
    } else {
        Ok(config)
    }
}

pub(crate) fn configure(
    config: OllamaProviderConfig,
    managed_single_slot: bool,
) -> Result<OllamaProviderConfig, AppCoreError> {
    let measured_hardware = measured_hardware();
    #[cfg(debug_assertions)]
    let policy = {
        let requested = std::env::var("ZETA_DEV_QWEN_PROFILE");
        match requested {
            Ok(value) => select(measured_hardware, managed_single_slot, Some(&value))?,
            Err(std::env::VarError::NotPresent) => {
                select(measured_hardware, managed_single_slot, None)?
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(model_runtime::OllamaError::InvalidConfiguration.into());
            }
        }
    };
    #[cfg(not(debug_assertions))]
    let policy = select(measured_hardware, managed_single_slot, None)?;
    Ok(config.with_qwen_tuning(policy))
}

fn select(
    measured_hardware: bool,
    managed_single_slot: bool,
    requested: Option<&str>,
) -> Result<QwenTuningPolicy, model_runtime::OllamaError> {
    match requested {
        Some("rollback") => Ok(QwenTuningPolicy::Existing),
        // An external qualification server must also be configured with one slot.
        Some("baseline") if measured_hardware => Ok(QwenTuningPolicy::M1ProBaselineV1),
        Some(_) => Err(model_runtime::OllamaError::InvalidConfiguration),
        None if measured_hardware && managed_single_slot => Ok(QwenTuningPolicy::M1ProBaselineV1),
        None => Ok(QwenTuningPolicy::Existing),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measured_hardware() -> bool {
    fn read(name: &str) -> Option<String> {
        let result = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", name])
            .output()
            .ok()?;
        result
            .status
            .success()
            .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
    }
    read("machdep.cpu.brand_string").as_deref() == Some("Apple M1 Pro")
        && read("hw.memsize").as_deref() == Some("34359738368")
        && read("hw.physicalcpu").as_deref() == Some("10")
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const fn measured_hardware() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_bounded_and_does_not_qualify_other_hardware() {
        assert_eq!(
            select(true, true, None),
            Ok(QwenTuningPolicy::M1ProBaselineV1)
        );
        assert_eq!(select(false, true, None), Ok(QwenTuningPolicy::Existing));
        assert_eq!(select(true, false, None), Ok(QwenTuningPolicy::Existing));
        assert_eq!(
            select(true, true, Some("rollback")),
            Ok(QwenTuningPolicy::Existing)
        );
        assert_eq!(
            select(true, false, Some("baseline")),
            Ok(QwenTuningPolicy::M1ProBaselineV1)
        );
        assert!(select(false, true, Some("baseline")).is_err());
        for invalid in ["", "off", "0", "2", "auto", "secret prompt"] {
            assert!(select(true, true, Some(invalid)).is_err());
        }
    }
}
