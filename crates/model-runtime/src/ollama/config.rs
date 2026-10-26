use super::{CHAT_DEADLINE, Duration, OllamaError, PULL_DEADLINE, SocketAddr};

/// Explicit loopback endpoint and bounded request deadlines for Ollama.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OllamaProviderConfig {
    pub(super) address: SocketAddr,
    pub(super) tuning_policy: super::QwenTuningPolicy,
    pub(super) connect_timeout: Duration,
    pub(super) control_timeout: Duration,
    pub(super) stream_timeout: Duration,
    pub(super) pull_timeout: Duration,
}

impl OllamaProviderConfig {
    /// Creates a configuration only for a concrete loopback TCP endpoint.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback addresses and the unspecified port.
    pub fn loopback(address: SocketAddr) -> Result<Self, OllamaError> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(OllamaError::InvalidConfiguration);
        }
        Ok(Self {
            address,
            tuning_policy: super::QwenTuningPolicy::Existing,
            connect_timeout: Duration::from_secs(2),
            control_timeout: Duration::from_secs(10),
            stream_timeout: CHAT_DEADLINE,
            pull_timeout: PULL_DEADLINE,
        })
    }

    /// Selects a startup-only policy after the host establishes its hardware/slot contract.
    #[must_use]
    pub const fn with_qwen_tuning(mut self, policy: super::QwenTuningPolicy) -> Self {
        self.tuning_policy = policy;
        self
    }

    /// Returns the host-selected startup policy for matching bundled-payload admission.
    #[must_use]
    pub const fn qwen_tuning(self) -> super::QwenTuningPolicy {
        self.tuning_policy
    }

    /// Returns the configured loopback socket without accepting a URL or path.
    #[must_use]
    pub const fn address(self) -> SocketAddr {
        self.address
    }
}

impl Default for OllamaProviderConfig {
    fn default() -> Self {
        Self {
            tuning_policy: super::QwenTuningPolicy::Existing,
            address: SocketAddr::from(([127, 0, 0, 1], 11_434)),
            connect_timeout: Duration::from_secs(2),
            control_timeout: Duration::from_secs(10),
            stream_timeout: CHAT_DEADLINE,
            pull_timeout: PULL_DEADLINE,
        }
    }
}

/// A pinned Ollama model verified against the local server catalogue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedOllamaModel {
    pub(super) tag: String,
    pub(super) manifest_sha256: String,
    pub(super) provider_version: String,
    pub(super) tuning_policy: super::QwenTuningPolicy,
}

impl VerifiedOllamaModel {
    #[cfg(test)]
    pub(crate) fn pinned_for_adapter(tag: &str, manifest_sha256: &str) -> Self {
        Self {
            tag: tag.to_owned(),
            manifest_sha256: manifest_sha256.to_owned(),
            provider_version: crate::PINNED_OLLAMA_VERSION.to_owned(),
            tuning_policy: super::QwenTuningPolicy::Existing,
        }
    }

    /// Returns the exact registered Ollama tag.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Returns the verified manifest digest without a scheme prefix.
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Exact Ollama server version that verified this model.
    #[must_use]
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }
}
