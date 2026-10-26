//! Exact, model-vocabulary token counting without loading inference weights.

use std::{
    error::Error,
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, MutexGuard},
};

const MAX_PROMPT_BYTES: usize = 64 * 1024 * 1024;

mod huggingface;

struct ActiveTokenizer {
    model_path: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

/// Supervises the bundled vocabulary-only tokenizer and reuses its loaded
/// vocabulary until the selected model changes.
pub struct ExactTokenizer {
    huggingface: huggingface::HuggingFaceTokenizer,
    executable: PathBuf,
    active: Mutex<Option<ActiveTokenizer>>,
}

impl ExactTokenizer {
    /// Creates an idle tokenizer using one application-bundled executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            huggingface: huggingface::HuggingFaceTokenizer::default(),
            executable: executable.into(),
            active: Mutex::new(None),
        }
    }

    /// Counts the pinned tensor tokenizer JSON with Ollama 0.32.14 semantics.
    /// Only the qualified Qwen BPE pretokenizer shape is admitted.
    ///
    /// # Errors
    /// Rejects missing/indirect files, digest mismatches, invalid JSON and unbounded prompts.
    pub fn count_ollama_tensor(
        &self,
        path: impl AsRef<Path>,
        sha256: &str,
        prompt: &str,
    ) -> Result<u64, TokenizerError> {
        self.huggingface.count(path.as_ref(), sha256, prompt)
    }

    /// Counts the exact tokens emitted by the vocabulary embedded in `model_path`.
    /// The helper loads vocabulary metadata only and remains alive for subsequent
    /// prompts using the same model.
    ///
    /// # Errors
    ///
    /// Returns a bounded tokenizer error when inputs are invalid, the helper
    /// cannot be started, or its authenticated protocol fails.
    pub fn count(&self, model_path: impl AsRef<Path>, prompt: &str) -> Result<u64, TokenizerError> {
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(TokenizerError::InvalidPrompt);
        }
        let model_path = canonical_file(model_path.as_ref(), "model")?;
        let executable = canonical_file(&self.executable, "tokenizer executable")?;
        let mut active = self.lock()?;
        let must_start = active.as_mut().is_none_or(|process| {
            process.model_path != model_path || process.child.try_wait().ok().flatten().is_some()
        });
        if must_start {
            if let Some(mut process) = active.take() {
                let _ = process.child.kill();
                let _ = process.child.wait();
            }
            *active = Some(start(&executable, &model_path)?);
        }
        let process = active.as_mut().ok_or(TokenizerError::Unavailable)?;
        process
            .stdin
            .write_all(&(prompt.len() as u64).to_le_bytes())
            .and_then(|()| process.stdin.write_all(prompt.as_bytes()))
            .and_then(|()| process.stdin.flush())
            .map_err(|_| TokenizerError::Unavailable)?;
        let mut response = [0_u8; 8];
        process
            .stdout
            .read_exact(&mut response)
            .map_err(|_| TokenizerError::Unavailable)?;
        let count = u64::from_le_bytes(response);
        if count == u64::MAX {
            return Err(TokenizerError::InvalidResponse);
        }
        Ok(count)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<ActiveTokenizer>>, TokenizerError> {
        self.active.lock().map_err(|_| TokenizerError::Unavailable)
    }
}

impl Drop for ExactTokenizer {
    fn drop(&mut self) {
        if let Ok(active) = self.active.get_mut()
            && let Some(mut process) = active.take()
        {
            let _ = process.child.kill();
            let _ = process.child.wait();
        }
    }
}

fn start(executable: &Path, model_path: &Path) -> Result<ActiveTokenizer, TokenizerError> {
    let mut child = Command::new(executable)
        .arg(model_path)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| TokenizerError::Unavailable)?;
    let stdin = child.stdin.take().ok_or(TokenizerError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(TokenizerError::Unavailable)?;
    Ok(ActiveTokenizer {
        model_path: model_path.to_owned(),
        child,
        stdin,
        stdout,
    })
}

fn canonical_file(path: &Path, kind: &'static str) -> Result<PathBuf, TokenizerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| TokenizerError::InvalidPath(kind))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TokenizerError::InvalidPath(kind));
    }
    path.canonicalize()
        .map_err(|_| TokenizerError::InvalidPath(kind))
}

/// Sanitized tokenizer failure.
#[derive(Debug, Eq, PartialEq)]
pub enum TokenizerError {
    /// A model or helper path is absent, indirect, or not a regular file.
    InvalidPath(&'static str),
    /// Prompt input is empty or exceeds the hard byte limit.
    InvalidPrompt,
    /// The helper returned a reserved or malformed count.
    InvalidResponse,
    /// The helper process or its private pipe is unavailable.
    Unavailable,
}

impl fmt::Display for TokenizerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(kind) => write!(formatter, "invalid {kind}"),
            Self::InvalidPrompt => formatter.write_str("invalid tokenizer prompt"),
            Self::InvalidResponse => formatter.write_str("invalid tokenizer response"),
            Self::Unavailable => formatter.write_str("tokenizer unavailable"),
        }
    }
}

impl Error for TokenizerError {}
