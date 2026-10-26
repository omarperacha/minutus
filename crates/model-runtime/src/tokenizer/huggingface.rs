//! Digest-bound native tokenizer JSON; no model weights or network execution.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;

use super::{MAX_PROMPT_BYTES, TokenizerError, canonical_file};

mod ollama;

const MAX_TOKENIZER_BYTES: u64 = 32 * 1024 * 1024;

struct Loaded {
    path: PathBuf,
    digest: String,
    tokenizer: Tokenizer,
}

#[derive(Default)]
pub(super) struct HuggingFaceTokenizer(Mutex<Option<Loaded>>);

impl HuggingFaceTokenizer {
    pub(super) fn count(
        &self,
        path: &Path,
        digest: &str,
        prompt: &str,
    ) -> Result<u64, TokenizerError> {
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(TokenizerError::InvalidPrompt);
        }
        let path = canonical_file(path, "tokenizer")?;
        let mut cached = self.0.lock().map_err(|_| TokenizerError::Unavailable)?;
        if cached
            .as_ref()
            .is_none_or(|loaded| loaded.path != path || loaded.digest != digest)
        {
            let mut bytes = Vec::new();
            fs::File::open(&path)
                .map_err(|_| TokenizerError::Unavailable)?
                .take(MAX_TOKENIZER_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| TokenizerError::Unavailable)?;
            if bytes.len() as u64 > MAX_TOKENIZER_BYTES
                || format!("{:x}", Sha256::digest(&bytes)) != digest
            {
                return Err(TokenizerError::InvalidResponse);
            }
            let mut tokenizer =
                Tokenizer::from_bytes(&bytes).map_err(|_| TokenizerError::InvalidResponse)?;
            ollama::configure(&mut tokenizer)?;
            *cached = Some(Loaded {
                path,
                digest: digest.to_owned(),
                tokenizer,
            });
        }
        cached
            .as_ref()
            .ok_or(TokenizerError::Unavailable)?
            .tokenizer
            .encode(prompt, false)
            .map(|encoding| encoding.len() as u64)
            .map_err(|_| TokenizerError::InvalidResponse)
    }
}

#[cfg(test)]
mod tests;
