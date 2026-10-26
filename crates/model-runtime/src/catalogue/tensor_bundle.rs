//! Pinned provider-owned safetensors bundles, including their tokenizer identity.

use std::collections::HashSet;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    CatalogueError, ModelArtifact,
    validation::{invalid, validate_sha256},
};

/// One digest-addressed blob in a verified Ollama tensor distribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OllamaBlob {
    sha256: String,
    bytes: u64,
}

impl OllamaBlob {
    /// Expected lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Exact blob length.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TensorBundle {
    pub(super) blobs: Vec<OllamaBlob>,
    pub(super) tokenizer: OllamaBlob,
    pub(super) total_bytes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    media_type: String,
    config: Layer,
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Layer {
    media_type: String,
    digest: String,
    size: u64,
    name: Option<String>,
}

impl TensorBundle {
    pub(super) fn parse(
        text: &str,
        manifest_digest: &str,
        config_digest: &str,
        config_bytes: u64,
        index: usize,
    ) -> Result<Self, CatalogueError> {
        let error = || {
            invalid(
                index,
                "backend.ollama.tensorBundleManifest",
                "invalid pinned tensor bundle",
            )
        };
        if text.len() > 768 * 1024
            || format!("{:x}", Sha256::digest(text.as_bytes())) != manifest_digest
        {
            return Err(error());
        }
        let manifest: Manifest = serde_json::from_str(text).map_err(|_| error())?;
        if manifest.schema_version != 2
            || manifest.media_type != "application/vnd.docker.distribution.manifest.v2+json"
            || manifest.config.digest != format!("sha256:{config_digest}")
            || manifest.config.size != config_bytes
            || manifest.config.media_type != "application/vnd.docker.container.image.v1+json"
            || manifest.layers.is_empty()
            || manifest.layers.len() > 2048
        {
            return Err(error());
        }
        let mut names = HashSet::new();
        let mut digests = HashSet::new();
        let mut blobs = Vec::new();
        let mut tokenizer = None;
        let mut total_bytes = 0_u64;
        for layer in std::iter::once(manifest.config).chain(manifest.layers) {
            let digest = layer.digest.strip_prefix("sha256:").ok_or_else(error)?;
            validate_sha256(digest).map_err(|_| error())?;
            if layer.size == 0
                || layer.size > 128 * 1024 * 1024 * 1024
                || !digests.insert(digest.to_owned())
            {
                return Err(error());
            }
            if let Some(name) = &layer.name
                && (name.is_empty()
                    || name.len() > 255
                    || name.contains(['/', '\\'])
                    || name.chars().any(char::is_control)
                    || !names.insert(name.clone()))
            {
                return Err(error());
            }
            let blob = OllamaBlob {
                sha256: digest.to_owned(),
                bytes: layer.size,
            };
            if layer.name.as_deref() == Some("tokenizer.json") {
                if layer.media_type != "application/vnd.ollama.image.json"
                    || layer.size > 32 * 1024 * 1024
                {
                    return Err(error());
                }
                tokenizer = Some(blob.clone());
            }
            total_bytes = total_bytes.checked_add(layer.size).ok_or_else(error)?;
            blobs.push(blob);
        }
        Ok(Self {
            blobs,
            tokenizer: tokenizer.ok_or_else(error)?,
            total_bytes,
        })
    }

    pub(super) fn contains_artifact(&self, artifact: &ModelArtifact) -> bool {
        self.blobs.iter().any(|b| {
            b.sha256 == artifact.sha256()
                && b.bytes == artifact.installed_bytes()
                && b.bytes == artifact.download_bytes()
        })
    }
}
