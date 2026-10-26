//! Evaluator composition for an external experiment or the actual packaged provider.

use std::{ffi::OsStr, fs, path::Path, process::Command};

use app_core::AppCore;
use model_runtime::OllamaProviderConfig;

use super::EvalResult;

pub(super) fn initialize(
    directory: &Path,
    tokenizer: &Path,
    models: &OsStr,
) -> EvalResult<AppCore> {
    match std::env::var("ZETA_DEV_OLLAMA_ADDRESS") {
        Ok(address) => Ok(AppCore::initialize_with_development_ollama(
            directory.join("app-data"),
            OllamaProviderConfig::loopback(address.parse()?)?,
            tokenizer,
            models,
        )?),
        Err(std::env::VarError::NotPresent) => {
            let executable = std::env::current_exe()?;
            let contents = executable
                .parent()
                .and_then(Path::parent)
                .ok_or("packaged evaluator has no Contents directory")?;
            let runtime = contents.join("Resources/ollama-runtime/ollama");
            if !runtime.is_file() {
                return Err("managed evaluation requires the packaged app runtime".into());
            }
            let data = directory.join("app-data");
            fs::create_dir(&data)?;
            // APFS clones preserve regular model files and isolate provider writes.
            // No fallback to a full multi-gigabyte copy or shared writable store.
            let status = Command::new("/bin/cp")
                .args(["-cR"])
                .arg(Path::new(models).canonicalize()?)
                .arg(data.join("ollama-models"))
                .status()?;
            if !status.success() {
                return Err("could not clone the isolated model store".into());
            }
            Ok(AppCore::initialize_with_guarded_bundled_ollama(
                data, runtime, tokenizer, executable,
            )?)
        }
        Err(error) => Err(error.into()),
    }
}
