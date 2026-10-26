//! Ollama provider access and independently callable process shutdown.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use model_runtime::{
    OllamaProvider, OllamaProviderConfig, OllamaSidecar, OllamaSidecarConfig, OllamaSidecarStatus,
};

use crate::AppCoreError;

struct ManagedOllamaSidecar {
    supervisor: Arc<OllamaSidecar>,
    config: OllamaSidecarConfig,
}

/// Provider state serialized across ordinary Ollama operations.
pub(crate) struct OllamaService {
    provider: OllamaProvider,
    managed: Option<ManagedOllamaSidecar>,
    shutdown_requested: Arc<AtomicBool>,
}

impl OllamaService {
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn external(provider: OllamaProvider) -> Self {
        Self {
            provider,
            managed: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn bundled(
        provider: OllamaProvider,
        supervisor: OllamaSidecar,
        config: OllamaSidecarConfig,
    ) -> Self {
        Self {
            provider,
            managed: Some(ManagedOllamaSidecar {
                supervisor: Arc::new(supervisor),
                config,
            }),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn provider(&mut self) -> Result<&OllamaProvider, AppCoreError> {
        if self.shutdown_requested.load(Ordering::Acquire) {
            return Err(AppCoreError::OllamaServiceUnavailable);
        }
        let Some(managed) = self.managed.as_ref() else {
            return Ok(&self.provider);
        };
        if managed.supervisor.status()? != OllamaSidecarStatus::Running {
            let endpoint = managed.supervisor.start(&managed.config)?;
            self.provider = OllamaProvider::new(
                OllamaProviderConfig::loopback(endpoint.address().into())?
                    .with_qwen_tuning(self.provider.qwen_tuning_policy()),
            )?;
        }
        Ok(&self.provider)
    }

    pub(crate) fn shutdown_handle(&self) -> OllamaShutdown {
        OllamaShutdown {
            shutdown_requested: Arc::clone(&self.shutdown_requested),
            supervisor: self
                .managed
                .as_ref()
                .map(|managed| Arc::clone(&managed.supervisor)),
        }
    }
}

/// Stop authority kept outside the provider-operation mutex.
///
/// A streaming request holds the service mutex for its full lifetime. Keeping
/// this handle independent lets application exit terminate Ollama immediately;
/// the closed connection then releases the blocked request and its mutex.
pub(crate) struct OllamaShutdown {
    shutdown_requested: Arc<AtomicBool>,
    supervisor: Option<Arc<OllamaSidecar>>,
}

impl OllamaShutdown {
    /// Immutable ownership metadata must remain readable while a stream holds
    /// the provider-operation mutex.
    pub(crate) const fn owns_runtime_store(&self) -> bool {
        self.supervisor.is_some()
    }

    pub(crate) fn shutdown(&self) -> Result<(), AppCoreError> {
        self.shutdown_requested.store(true, Ordering::Release);
        if let Some(supervisor) = self.supervisor.as_ref() {
            supervisor.stop()?;
        }
        Ok(())
    }

    pub(crate) fn stop_for_model_change(&self) -> Result<(), AppCoreError> {
        if let Some(supervisor) = self.supervisor.as_ref() {
            supervisor.stop()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn stop_managed(&self) -> Result<bool, AppCoreError> {
        let Some(supervisor) = self.supervisor.as_ref() else {
            return Ok(false);
        };
        supervisor.stop()?;
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn managed_status(&self) -> Result<Option<OllamaSidecarStatus>, AppCoreError> {
        self.supervisor
            .as_ref()
            .map(|supervisor| supervisor.status().map_err(AppCoreError::from))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use model_runtime::{OllamaProvider, OllamaProviderConfig};

    use super::*;

    #[test]
    fn shutdown_signal_prevents_provider_reentry() -> Result<(), Box<dyn std::error::Error>> {
        let mut service =
            OllamaService::external(OllamaProvider::new(OllamaProviderConfig::default())?);
        let shutdown = service.shutdown_handle();

        shutdown.shutdown()?;

        assert!(matches!(
            service.provider(),
            Err(AppCoreError::OllamaServiceUnavailable)
        ));
        Ok(())
    }
}
