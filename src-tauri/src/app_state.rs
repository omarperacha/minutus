//! Process-local controls for model downloads and chat generation.

use super::{
    Arc, AtomicBool, DownloadCancellation, IpcError, IpcErrorCode, Mutex, Ordering, PathBuf,
    RequestId,
};

pub(crate) const MODEL_DOWNLOAD_PROGRESS_EVENT: &str = "model-download-progress";
pub(crate) const CHAT_GENERATION_EVENT: &str = "chat-generation-event";
// Dependent workspace discovery commonly needs several sequential reads before
// the model can identify the exact source and destination. Context, time, output,
// and final-answer budgets remain independently enforced on every round.
pub(crate) const MAX_CHAT_TOOL_ROUNDS: usize = 12;
pub(crate) const MAX_CHAT_TOOL_CALLS: usize = 18;

pub(crate) struct InferenceExecutable(pub(crate) PathBuf);
pub(crate) struct SpeechInferenceExecutable(pub(crate) PathBuf);

pub(crate) struct ActiveChatGeneration {
    request_id: RequestId,
    cancellation_requested: Arc<AtomicBool>,
    _activity: app_core::ChatActivityLease,
}

pub(crate) struct ChatGenerationControl {
    active: Mutex<Option<ActiveChatGeneration>>,
}

impl ChatGenerationControl {
    pub(crate) const fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub(crate) fn begin(
        &self,
        request_id: RequestId,
        activity: app_core::ChatActivityLease,
    ) -> Result<Arc<AtomicBool>, IpcError> {
        let mut active = self.active.lock().map_err(|_| IpcError::internal())?;
        if active.is_some() {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "A response is already being generated".to_owned(),
                retryable: true,
            });
        }
        let cancellation_requested = Arc::new(AtomicBool::new(false));
        *active = Some(ActiveChatGeneration {
            request_id,
            cancellation_requested: Arc::clone(&cancellation_requested),
            _activity: activity,
        });
        Ok(cancellation_requested)
    }

    pub(crate) fn cancel(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<Arc<AtomicBool>>, IpcError> {
        let active = self.active.lock().map_err(|_| IpcError::internal())?;
        Ok(active
            .as_ref()
            .filter(|active| active.request_id == *request_id)
            .map(|active| {
                active.cancellation_requested.store(true, Ordering::Release);
                Arc::clone(&active.cancellation_requested)
            }))
    }

    pub(crate) fn finish(&self, request_id: &RequestId) {
        if let Ok(mut active) = self.active.lock()
            && active
                .as_ref()
                .is_some_and(|active| active.request_id == *request_id)
        {
            *active = None;
        }
    }

    pub(crate) fn cancel_active(&self) {
        if let Ok(active) = self.active.lock()
            && let Some(active) = active.as_ref()
        {
            active.cancellation_requested.store(true, Ordering::Release);
        }
    }

    pub(crate) fn is_active(&self) -> Result<bool, IpcError> {
        self.active
            .lock()
            .map(|active| active.is_some())
            .map_err(|_| IpcError::internal())
    }
}

pub(crate) struct ModelDownloadControl {
    active: Mutex<Option<DownloadCancellation>>,
}

impl ModelDownloadControl {
    pub(crate) const fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub(crate) fn begin(&self) -> Result<DownloadCancellation, IpcError> {
        let mut active = self.active.lock().map_err(|_| IpcError::internal())?;
        if active.is_some() {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "A model installation or file registration is already active".to_owned(),
                retryable: true,
            });
        }
        let cancellation = DownloadCancellation::new();
        *active = Some(cancellation.clone());
        Ok(cancellation)
    }

    pub(crate) fn finish(&self) {
        if let Ok(mut active) = self.active.lock() {
            *active = None;
        }
    }

    pub(crate) fn cancel(&self) -> Result<bool, IpcError> {
        let active = self.active.lock().map_err(|_| IpcError::internal())?;
        if let Some(cancellation) = active.as_ref() {
            cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
