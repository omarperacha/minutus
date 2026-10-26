use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use ipc_types::{
    ModelDownloadPhase, SpeechDownloadProgress, SpeechInstallState, SpeechModelRequest,
    SpeechModelView, ValidateIpc,
};
use model_runtime::{
    DownloadCancellation, DownloadPhase, DownloadProgress, SpeechCatalogue, SpeechProfile,
    asr::{SpeechModelError, SpeechModelStatus, SpeechModelStore},
    bundled_speech_catalogue,
};

use crate::{AppCore, AppCoreError};

pub(crate) struct SpeechModels {
    catalogue: SpeechCatalogue,
    store: SpeechModelStore,
    transfer: Mutex<Option<Transfer>>,
}

struct Transfer {
    selection: SpeechModelRequest,
    cancellation: DownloadCancellation,
    progress: Option<DownloadProgress>,
}

impl SpeechModels {
    pub(crate) fn initialize(root: &Path) -> Result<Self, AppCoreError> {
        Ok(Self {
            catalogue: bundled_speech_catalogue()?,
            store: SpeechModelStore::initialize(root)?,
            transfer: Mutex::new(None),
        })
    }

    fn profile(&self, request: &SpeechModelRequest) -> Result<&SpeechProfile, AppCoreError> {
        request
            .validate()
            .map_err(|_| AppCoreError::CatalogueModelNotFound)?;
        self.catalogue
            .profiles()
            .iter()
            .find(|profile| {
                profile.key().id() == request.model_id
                    && profile.key().version() == request.model_version
            })
            .ok_or(AppCoreError::CatalogueModelNotFound)
    }

    fn default_selection(&self) -> Result<SpeechModelRequest, AppCoreError> {
        self.catalogue
            .profiles()
            .first()
            .map(|profile| SpeechModelRequest {
                model_id: profile.key().id().to_owned(),
                model_version: profile.key().version().to_owned(),
            })
            .ok_or(AppCoreError::CatalogueModelNotFound)
    }

    pub(super) fn require_install_candidate(
        &self,
        request: &SpeechModelRequest,
    ) -> Result<(), AppCoreError> {
        let profile = self
            .profile(request)
            .map_err(|_| AppCoreError::SpeechModelNotInstalled)?;
        if self.store.has_install_candidate(profile)? {
            Ok(())
        } else {
            Err(AppCoreError::SpeechModelNotInstalled)
        }
    }

    pub(super) fn recognition_session(
        &self,
        request: &SpeechModelRequest,
        helper_executable: PathBuf,
        language: Option<&str>,
        cancellation: &DownloadCancellation,
    ) -> Result<Box<dyn model_runtime::asr::RecognitionSession>, super::session::SessionFailure>
    {
        use model_runtime::asr::{
            RecognitionConfig, SpeechRecognizer, WhisperHelper, WhisperHelperConfig,
        };

        let profile = self
            .profile(request)
            .map_err(|_| super::session::SessionFailure::SpeechModelUnavailable)?;
        let lease = self
            .store
            .acquire(profile, cancellation)
            .map_err(|_| super::session::SessionFailure::SpeechModelUnavailable)?;
        let config = match language {
            Some(language) => RecognitionConfig::automatic()
                .with_language(language)
                .map_err(super::session::SessionFailure::Recognition)?,
            None => RecognitionConfig::automatic(),
        };
        WhisperHelper::new(WhisperHelperConfig::new(helper_executable))
            .start(lease, config)
            .map_err(super::session::SessionFailure::Recognition)
    }
}

impl AppCore {
    /// Returns the configured dictation profile without reading model bytes.
    ///
    /// Recognition admission independently verifies the selected artifact.
    ///
    /// # Errors
    /// Returns an error when the configured speech catalogue is empty.
    pub fn speech_model_selection(&self) -> Result<SpeechModelRequest, AppCoreError> {
        self.speech_models.default_selection()
    }

    /// Lists optional speech profiles with offline, verified installation status.
    ///
    /// # Errors
    /// Returns a storage error if status cannot be established safely.
    pub fn speech_models(&self) -> Result<Vec<SpeechModelView>, AppCoreError> {
        self.speech_models
            .catalogue
            .profiles()
            .iter()
            .map(|profile| {
                let selection = SpeechModelRequest {
                    model_id: profile.key().id().to_owned(),
                    model_version: profile.key().version().to_owned(),
                };
                let transfer = self
                    .speech_models
                    .transfer
                    .lock()
                    .map_err(|_| AppCoreError::StorageUnavailable)?;
                let state = if let Some(active) = transfer
                    .as_ref()
                    .filter(|active| active.selection == selection)
                {
                    SpeechInstallState::Downloading {
                        progress: active.progress.map(project_progress),
                    }
                } else {
                    drop(transfer);
                    match self.speech_models.store.status(profile)? {
                        SpeechModelStatus::Missing => SpeechInstallState::Missing,
                        SpeechModelStatus::Installed => SpeechInstallState::Installed,
                        SpeechModelStatus::Incompatible => SpeechInstallState::Incompatible,
                        SpeechModelStatus::Busy => SpeechInstallState::InUse,
                    }
                };
                Ok(SpeechModelView {
                    selection,
                    display_name: profile.display_name().to_owned(),
                    download_bytes: profile.artifact().download_bytes(),
                    license_url: profile.license_url().to_string(),
                    state,
                })
            })
            .collect()
    }

    /// Installs only the requested speech weights. Completion never starts capture.
    ///
    /// # Errors
    /// Rejects unknown releases, active use, cancellation and failed verification.
    pub fn install_speech_model(&self, request: &SpeechModelRequest) -> Result<(), AppCoreError> {
        let profile = self.speech_models.profile(request)?;
        let cancellation = DownloadCancellation::new();
        {
            let mut transfer = self
                .speech_models
                .transfer
                .lock()
                .map_err(|_| AppCoreError::StorageUnavailable)?;
            if transfer.is_some() {
                return Err(SpeechModelError::Busy.into());
            }
            *transfer = Some(Transfer {
                selection: request.clone(),
                cancellation: cancellation.clone(),
                progress: None,
            });
        }
        let result = self
            .speech_models
            .store
            .install(profile, &cancellation, |progress| {
                if let Ok(mut transfer) = self.speech_models.transfer.lock()
                    && let Some(active) = transfer.as_mut()
                {
                    active.progress = Some(progress);
                }
            });
        *self
            .speech_models
            .transfer
            .lock()
            .map_err(|_| AppCoreError::StorageUnavailable)? = None;
        result.map_err(Into::into)
    }

    /// Cancels a speech transfer while preserving resumable partial bytes.
    ///
    /// # Errors
    /// Returns an error if the transfer state is unavailable.
    pub fn cancel_speech_download(&self) -> Result<bool, AppCoreError> {
        let transfer = self
            .speech_models
            .transfer
            .lock()
            .map_err(|_| AppCoreError::StorageUnavailable)?;
        if let Some(active) = transfer.as_ref() {
            active.cancellation.cancel();
            return Ok(true);
        }
        Ok(false)
    }

    /// Removes speech weights without touching chat preferences or installed chat models.
    ///
    /// # Errors
    /// Rejects active speech use/installation, unknown releases and storage failures.
    pub fn remove_speech_model(&self, request: &SpeechModelRequest) -> Result<(), AppCoreError> {
        let profile = self.speech_models.profile(request)?;
        // Hold admission until removal finishes so a new transfer cannot start between checks.
        let transfer = self
            .speech_models
            .transfer
            .lock()
            .map_err(|_| AppCoreError::StorageUnavailable)?;
        if transfer.is_some() {
            return Err(SpeechModelError::Busy.into());
        }
        self.speech_models.store.remove(profile)?;
        Ok(())
    }
}

fn project_progress(progress: DownloadProgress) -> SpeechDownloadProgress {
    SpeechDownloadProgress {
        phase: match progress.phase() {
            DownloadPhase::Downloading => ModelDownloadPhase::Downloading,
            DownloadPhase::Verifying => ModelDownloadPhase::Verifying,
            DownloadPhase::Installing => ModelDownloadPhase::Installing,
        },
        downloaded_bytes: progress.downloaded_bytes(),
        total_bytes: progress.total_bytes(),
        resumed: progress.resumed(),
    }
}
