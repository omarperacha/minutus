use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use audio_runtime::{
    AudioCapture, AudioCaptureSession, CaptureAuthorization, CaptureError, CaptureEvent,
    CaptureStopReason, CpalInputBackend, MicrophoneCapture, NormalizedAudioFrame,
};
use ipc_types::{
    DictationAccepted, DictationBinding, DictationCommand, DictationEvent, DictationEventKind,
    DictationEventsRequest, DictationFailureCode, DictationSessionId, StartDictationRequest,
};
use model_runtime::{
    DownloadCancellation,
    asr::{
        AudioFormat, AudioFrame, RecognitionCancellation, RecognitionError, RecognitionEvent,
        RecognitionSession,
    },
};

use super::{SpeechModels, admission::DictationActivityLease};
use crate::{AppCore, AppCoreError};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const LEVEL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_RECOGNIZER_FRAME_SAMPLES: usize = 16_000;
const MAX_COMPLETED_SESSIONS: usize = 8;
const MAX_POLLED_EVENTS: usize = 16;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

/// Host lifecycle reason for cancelling the active dictation session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationCancelReason {
    /// Explicit composer cancellation.
    Cancelled,
    /// Initiating composer was disposed during navigation.
    Navigation,
    /// Desktop window is closing.
    WindowClosed,
    /// System sleep invalidated the active input stream.
    SystemSleep,
    /// Host memory pressure requires speech resources to be released.
    MemoryPressure,
}

#[derive(Clone)]
struct SessionControl {
    finish: Arc<AtomicBool>,
    cancel_reason: Arc<AtomicU8>,
    recognition_cancellation: RecognitionCancellation,
    model_cancellation: DownloadCancellation,
}

impl SessionControl {
    fn new() -> Self {
        Self {
            finish: Arc::new(AtomicBool::new(false)),
            cancel_reason: Arc::new(AtomicU8::new(0)),
            recognition_cancellation: RecognitionCancellation::new(),
            model_cancellation: DownloadCancellation::new(),
        }
    }

    fn cancel(&self, reason: DictationCancelReason) {
        let code = match reason {
            DictationCancelReason::Cancelled => 1,
            DictationCancelReason::Navigation => 2,
            DictationCancelReason::WindowClosed => 3,
            DictationCancelReason::SystemSleep => 4,
            DictationCancelReason::MemoryPressure => 5,
        };
        let _ = self
            .cancel_reason
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
        self.model_cancellation.cancel();
        self.recognition_cancellation.cancel();
    }

    fn capture_stop_reason(&self) -> Option<CaptureStopReason> {
        match self.cancel_reason.load(Ordering::Acquire) {
            1 | 5 => Some(CaptureStopReason::Cancelled),
            2 => Some(CaptureStopReason::Navigation),
            3 => Some(CaptureStopReason::WindowClosed),
            4 => Some(CaptureStopReason::SystemSleep),
            _ => None,
        }
    }
}

struct SessionRecord {
    id: DictationSessionId,
    binding: DictationBinding,
    events: VecDeque<DictationEvent>,
    next_sequence: u64,
    control: SessionControl,
    activity: Option<DictationActivityLease>,
}

struct SessionState {
    active: Option<SessionRecord>,
    completed: VecDeque<SessionRecord>,
}

pub(crate) struct DictationSessions {
    state: Mutex<SessionState>,
}

impl DictationSessions {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(SessionState {
                active: None,
                completed: VecDeque::new(),
            }),
        }
    }

    fn begin(
        self: &Arc<Self>,
        binding: DictationBinding,
        activity: DictationActivityLease,
        resources: Box<dyn SessionResources>,
    ) -> Result<DictationAccepted, AppCoreError> {
        let id = DictationSessionId(uuid::Uuid::now_v7().to_string());
        let control = SessionControl::new();
        let mut record = SessionRecord {
            id: id.clone(),
            binding: binding.clone(),
            events: VecDeque::new(),
            next_sequence: 1,
            control: control.clone(),
            activity: Some(activity),
        };
        push_event(&mut record, DictationEventKind::Preparing);
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| AppCoreError::DictationUnavailable)?;
            if state.active.is_some() {
                return Err(AppCoreError::ForegroundActivityBusy);
            }
            state.active = Some(record);
        }
        let sessions = Arc::clone(self);
        let worker_id = id.clone();
        if thread::Builder::new()
            .name("zeta-dictation-session".to_owned())
            .spawn(move || run_session(&sessions, &worker_id, &control, resources.as_ref()))
            .is_err()
        {
            self.complete(&id, failed_event(SessionFailure::Internal));
            return Err(AppCoreError::DictationUnavailable);
        }
        Ok(DictationAccepted {
            session_id: id,
            binding,
        })
    }

    fn publish(&self, id: &DictationSessionId, event: DictationEventKind) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(record) = state.active.as_mut().filter(|record| record.id == *id) else {
            return;
        };
        if matches!(event, DictationEventKind::Recording { .. })
            && record.events.back().is_some_and(|existing| {
                matches!(existing.event, DictationEventKind::Recording { .. })
            })
        {
            record.events.pop_back();
        }
        push_event(record, event);
    }

    fn complete(&self, id: &DictationSessionId, event: DictationEventKind) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(mut record) = state.active.take().filter(|record| record.id == *id) else {
            return;
        };
        push_event(&mut record, event);
        record.activity.take();
        state.completed.push_back(record);
        while state.completed.len() > MAX_COMPLETED_SESSIONS {
            state.completed.pop_front();
        }
    }

    fn finish(&self, command: &DictationCommand) -> Result<bool, AppCoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AppCoreError::DictationUnavailable)?;
        let Some(record) = state.active.as_ref() else {
            return Ok(false);
        };
        verify_record(record, &command.session_id, &command.binding)?;
        if record.control.capture_stop_reason().is_some() {
            return Ok(false);
        }
        record.control.finish.store(true, Ordering::Release);
        Ok(true)
    }

    fn cancel(
        &self,
        command: &DictationCommand,
        reason: DictationCancelReason,
    ) -> Result<bool, AppCoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AppCoreError::DictationUnavailable)?;
        if let Some(record) = state.active.as_ref() {
            verify_record(record, &command.session_id, &command.binding)?;
            record.control.cancel(reason);
            return Ok(true);
        }
        let Some(record) = state
            .completed
            .iter()
            .find(|record| record.id == command.session_id)
        else {
            return Ok(false);
        };
        verify_record(record, &command.session_id, &command.binding)?;
        Ok(record
            .events
            .back()
            .is_some_and(|event| matches!(event.event, DictationEventKind::Cancelled)))
    }

    fn cancel_active(&self, reason: DictationCancelReason) {
        if let Ok(state) = self.state.lock()
            && let Some(record) = state.active.as_ref()
        {
            record.control.cancel(reason);
        }
    }

    fn events(
        &self,
        request: &DictationEventsRequest,
    ) -> Result<Vec<DictationEvent>, AppCoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AppCoreError::DictationUnavailable)?;
        let record = state
            .active
            .iter()
            .chain(state.completed.iter())
            .find(|record| record.id == request.session_id)
            .ok_or(AppCoreError::InvalidDictationBinding)?;
        verify_record(record, &request.session_id, &request.binding)?;
        Ok(record
            .events
            .iter()
            .filter(|event| event.sequence > request.after_sequence)
            .take(MAX_POLLED_EVENTS)
            .cloned()
            .collect())
    }
}

fn verify_record(
    record: &SessionRecord,
    id: &DictationSessionId,
    binding: &DictationBinding,
) -> Result<(), AppCoreError> {
    if record.id != *id || record.binding != *binding {
        return Err(AppCoreError::InvalidDictationBinding);
    }
    Ok(())
}

fn push_event(record: &mut SessionRecord, event: DictationEventKind) {
    let envelope = DictationEvent {
        session_id: record.id.clone(),
        binding: record.binding.clone(),
        sequence: record.next_sequence,
        event,
    };
    record.next_sequence = record.next_sequence.saturating_add(1);
    record.events.push_back(envelope);
}

trait SessionResources: Send + Sync + 'static {
    fn prepare_recognizer(
        &self,
        model_cancellation: &DownloadCancellation,
    ) -> Result<Box<dyn RecognitionSession>, SessionFailure>;
    fn start_capture(&self) -> Result<Box<dyn AudioCaptureSession>, SessionFailure>;
}

struct ProductionResources {
    speech_models: Arc<SpeechModels>,
    request: StartDictationRequest,
    helper_executable: PathBuf,
    capture: MicrophoneCapture<CpalInputBackend>,
}

impl SessionResources for ProductionResources {
    fn prepare_recognizer(
        &self,
        cancellation: &DownloadCancellation,
    ) -> Result<Box<dyn RecognitionSession>, SessionFailure> {
        let helper = std::fs::symlink_metadata(&self.helper_executable)
            .map_err(|_| SessionFailure::Recognition(RecognitionError::Unavailable))?;
        if helper.file_type().is_symlink() || !helper.is_file() {
            return Err(SessionFailure::Recognition(RecognitionError::Unavailable));
        }
        self.speech_models.recognition_session(
            &self.request.model,
            self.helper_executable.clone(),
            self.request.language.as_deref(),
            cancellation,
        )
    }

    fn start_capture(&self) -> Result<Box<dyn AudioCaptureSession>, SessionFailure> {
        self.capture
            .start_capture(CaptureAuthorization::after_user_action())
            .map_err(SessionFailure::Capture)
    }
}

#[derive(Clone, Copy)]
pub(super) enum SessionFailure {
    Capture(CaptureError),
    Recognition(RecognitionError),
    SpeechModelUnavailable,
    InvalidResult,
    Internal,
}

enum PreparationOutcome {
    Ready(Result<Box<dyn RecognitionSession>, SessionFailure>),
    Cancelled,
    CaptureFailed(CaptureError),
}

enum RecordingOutcome {
    Finished { duration_limited: bool },
    Cancelled,
    Failed(SessionFailure),
}

fn run_session(
    sessions: &DictationSessions,
    id: &DictationSessionId,
    control: &SessionControl,
    resources: &dyn SessionResources,
) {
    let capture = resources.start_capture();
    if control.capture_stop_reason().is_some() {
        sessions.complete(id, DictationEventKind::Cancelled);
        return;
    }
    let Ok(mut capture) = capture else {
        sessions.complete(
            id,
            failed_event(capture.err().unwrap_or(SessionFailure::Internal)),
        );
        return;
    };
    sessions.publish(
        id,
        DictationEventKind::Recording {
            level_millipercent: 0,
            elapsed_ms: 0,
        },
    );
    let started = Instant::now();
    let mut last_level: Option<Instant> = None;
    let mut recognition_sequence = 0_u64;
    let (preparation, mut buffered_frames, capture_finished) = prepare_while_recording(
        sessions,
        id,
        control,
        resources,
        &mut capture,
        started,
        &mut last_level,
    );
    let mut recognizer = match preparation {
        PreparationOutcome::Ready(Ok(recognizer)) => recognizer,
        PreparationOutcome::Ready(Err(error)) => {
            if !capture_finished {
                let _ = capture.stop_capture(CaptureStopReason::Cancelled);
            }
            sessions.complete(id, failed_event(error));
            return;
        }
        PreparationOutcome::Cancelled => {
            sessions.complete(id, DictationEventKind::Cancelled);
            return;
        }
        PreparationOutcome::CaptureFailed(error) => {
            sessions.complete(id, failed_event(SessionFailure::Capture(error)));
            return;
        }
    };
    if let Err(error) = push_buffered_audio(
        &mut recognizer,
        &mut buffered_frames,
        &mut recognition_sequence,
    ) {
        if !capture_finished {
            let _ = capture.stop_capture(CaptureStopReason::Cancelled);
        }
        let _ = recognizer.cancel();
        sessions.complete(id, failed_event(SessionFailure::Recognition(error)));
        return;
    }
    if capture_finished {
        drop(capture);
        finish_recognition(sessions, id, control, recognizer);
        return;
    }
    let duration_limited = match record_until_finish(
        sessions,
        id,
        control,
        &mut capture,
        &mut recognizer,
        started,
        &mut last_level,
        &mut recognition_sequence,
    ) {
        RecordingOutcome::Finished { duration_limited } => duration_limited,
        RecordingOutcome::Cancelled => {
            drop(capture);
            let _ = recognizer.cancel();
            sessions.complete(id, DictationEventKind::Cancelled);
            return;
        }
        RecordingOutcome::Failed(error) => {
            drop(capture);
            let _ = recognizer.cancel();
            sessions.complete(id, failed_event(error));
            return;
        }
    };
    drop(capture);
    publish_transcribing(sessions, id, duration_limited);
    finish_recognition(sessions, id, control, recognizer);
}

#[allow(clippy::too_many_arguments)]
// These are the live session capabilities and counters; grouping them would
// obscure which owner each mutable value belongs to.
fn record_until_finish(
    sessions: &DictationSessions,
    id: &DictationSessionId,
    control: &SessionControl,
    capture: &mut Box<dyn AudioCaptureSession>,
    recognizer: &mut Box<dyn RecognitionSession>,
    started: Instant,
    last_level: &mut Option<Instant>,
    recognition_sequence: &mut u64,
) -> RecordingOutcome {
    loop {
        if let Some(reason) = control.capture_stop_reason() {
            let _ = capture.stop_capture(reason);
            return RecordingOutcome::Cancelled;
        }
        if control.finish.load(Ordering::Acquire) {
            let _ = capture.stop_capture(CaptureStopReason::Finished);
            return RecordingOutcome::Finished {
                duration_limited: false,
            };
        }
        match capture.try_event() {
            Some(CaptureEvent::Frame { frame, level }) => {
                if let Err(error) = push_recognizer_frame(recognizer, &frame, recognition_sequence)
                {
                    let _ = capture.stop_capture(CaptureStopReason::Cancelled);
                    return RecordingOutcome::Failed(SessionFailure::Recognition(error));
                }
                if last_level.is_none_or(|last| last.elapsed() >= LEVEL_INTERVAL) {
                    sessions.publish(id, recording_event(started, level.value()));
                    *last_level = Some(Instant::now());
                }
            }
            Some(CaptureEvent::Stopped(CaptureStopReason::DurationLimit)) => {
                return RecordingOutcome::Finished {
                    duration_limited: true,
                };
            }
            Some(CaptureEvent::Stopped(_)) => return RecordingOutcome::Cancelled,
            Some(CaptureEvent::Failed(error)) => {
                return RecordingOutcome::Failed(SessionFailure::Capture(error));
            }
            None => thread::sleep(POLL_INTERVAL),
        }
    }
}

fn prepare_while_recording(
    sessions: &DictationSessions,
    id: &DictationSessionId,
    control: &SessionControl,
    resources: &dyn SessionResources,
    capture: &mut Box<dyn AudioCaptureSession>,
    started: Instant,
    last_level: &mut Option<Instant>,
) -> (PreparationOutcome, VecDeque<NormalizedAudioFrame>, bool) {
    let mut buffered_frames = VecDeque::new();
    let mut capture_finished = false;
    let preparation = thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(1);
        scope.spawn(move || {
            let result = resources.prepare_recognizer(&control.model_cancellation);
            let _ = sender.send(result);
        });
        loop {
            if let Some(reason) = control.capture_stop_reason() {
                control.model_cancellation.cancel();
                if !capture_finished {
                    let _ = capture.stop_capture(reason);
                }
                return PreparationOutcome::Cancelled;
            }
            if !capture_finished && control.finish.load(Ordering::Acquire) {
                let _ = capture.stop_capture(CaptureStopReason::Finished);
                capture_finished = true;
                publish_transcribing(sessions, id, false);
            }
            if !capture_finished {
                match capture.try_event() {
                    Some(CaptureEvent::Frame { frame, level }) => {
                        buffered_frames.push_back(frame);
                        if last_level.is_none_or(|last| last.elapsed() >= LEVEL_INTERVAL) {
                            sessions.publish(id, recording_event(started, level.value()));
                            *last_level = Some(Instant::now());
                        }
                    }
                    Some(CaptureEvent::Stopped(CaptureStopReason::DurationLimit)) => {
                        capture_finished = true;
                        publish_transcribing(sessions, id, true);
                    }
                    Some(CaptureEvent::Stopped(_)) => {
                        control.model_cancellation.cancel();
                        return PreparationOutcome::Cancelled;
                    }
                    Some(CaptureEvent::Failed(error)) => {
                        control.model_cancellation.cancel();
                        return PreparationOutcome::CaptureFailed(error);
                    }
                    None => {}
                }
            }
            match receiver.try_recv() {
                Ok(result) => return PreparationOutcome::Ready(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return PreparationOutcome::Ready(Err(SessionFailure::Internal));
                }
                Err(mpsc::TryRecvError::Empty) => thread::sleep(POLL_INTERVAL),
            }
        }
    });
    (preparation, buffered_frames, capture_finished)
}

fn push_buffered_audio(
    recognizer: &mut Box<dyn RecognitionSession>,
    frames: &mut VecDeque<NormalizedAudioFrame>,
    sequence: &mut u64,
) -> Result<(), RecognitionError> {
    while let Some(frame) = frames.pop_front() {
        push_recognizer_frame(recognizer, &frame, sequence)?;
    }
    Ok(())
}

fn push_recognizer_frame(
    recognizer: &mut Box<dyn RecognitionSession>,
    frame: &NormalizedAudioFrame,
    sequence: &mut u64,
) -> Result<(), RecognitionError> {
    for samples in frame.samples.chunks(MAX_RECOGNIZER_FRAME_SAMPLES) {
        recognizer.push_audio(AudioFrame {
            sequence: *sequence,
            format: AudioFormat::WHISPER_PCM,
            samples: samples.to_vec(),
        })?;
        *sequence = sequence.saturating_add(1);
    }
    Ok(())
}

fn publish_transcribing(
    sessions: &DictationSessions,
    id: &DictationSessionId,
    duration_limit_reached: bool,
) {
    sessions.publish(
        id,
        DictationEventKind::Transcribing {
            duration_limit_reached,
        },
    );
}

fn finish_recognition(
    sessions: &DictationSessions,
    id: &DictationSessionId,
    control: &SessionControl,
    recognizer: Box<dyn RecognitionSession>,
) {
    let result = recognizer.finish(&control.recognition_cancellation);
    if control.capture_stop_reason().is_some() {
        sessions.complete(id, DictationEventKind::Cancelled);
        return;
    }
    sessions.complete(id, recognition_event(result));
}

fn recording_event(started: Instant, level: f32) -> DictationEventKind {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // `audio-runtime` guarantees finite 0..=1 levels, so scaling is bounded to u16.
    let level_millipercent = (level.clamp(0.0, 1.0) * 1000.0).round() as u16;
    DictationEventKind::Recording {
        level_millipercent,
        elapsed_ms,
    }
}

fn recognition_event(result: Result<RecognitionEvent, RecognitionError>) -> DictationEventKind {
    match result {
        Ok(RecognitionEvent::Final { text, language, .. })
            if !text.is_empty()
                && text.len() <= MAX_TRANSCRIPT_BYTES
                && (2..=8).contains(&language.len())
                && language.is_ascii() =>
        {
            DictationEventKind::Completed { text, language }
        }
        Ok(RecognitionEvent::NoSpeech { .. }) => DictationEventKind::NoSpeech,
        Ok(RecognitionEvent::Cancelled { .. }) => DictationEventKind::Cancelled,
        Ok(RecognitionEvent::Final { .. }) => failed_event(SessionFailure::InvalidResult),
        Err(error) => failed_event(SessionFailure::Recognition(error)),
    }
}

fn failed_event(failure: SessionFailure) -> DictationEventKind {
    let (code, message) = match failure {
        SessionFailure::Capture(CaptureError::PermissionDenied) => (
            DictationFailureCode::PermissionDenied,
            "Microphone access is denied. Enable it in system settings.",
        ),
        SessionFailure::Capture(CaptureError::NoInputDevice) => (
            DictationFailureCode::NoInputDevice,
            "No microphone input device is available.",
        ),
        SessionFailure::Capture(CaptureError::DeviceLost) => (
            DictationFailureCode::DeviceLost,
            "The microphone became unavailable.",
        ),
        SessionFailure::Capture(CaptureError::Overflow) => (
            DictationFailureCode::AudioOverflow,
            "Microphone audio exceeded the local processing limit.",
        ),
        SessionFailure::Capture(CaptureError::UnsupportedFormat | CaptureError::StartFailed) => (
            DictationFailureCode::Internal,
            "The microphone could not start with a supported format.",
        ),
        SessionFailure::SpeechModelUnavailable => (
            DictationFailureCode::SpeechModelUnavailable,
            "Install or repair the local speech model before dictating.",
        ),
        SessionFailure::Recognition(RecognitionError::Unavailable) => (
            DictationFailureCode::RecognitionUnavailable,
            "Local speech recognition is unavailable.",
        ),
        SessionFailure::Recognition(RecognitionError::DeadlineExceeded) => (
            DictationFailureCode::DeadlineExceeded,
            "Local speech recognition exceeded its time limit.",
        ),
        SessionFailure::Recognition(_) | SessionFailure::InvalidResult => (
            DictationFailureCode::RecognitionFailed,
            "Local speech recognition failed.",
        ),
        SessionFailure::Internal => (
            DictationFailureCode::Internal,
            "Dictation could not be started.",
        ),
    };
    DictationEventKind::Failed {
        code,
        message: message.to_owned(),
    }
}

impl AppCore {
    /// Starts one host-owned dictation session after validating project ownership.
    ///
    /// # Errors
    /// Rejects stale bindings, concurrent foreground work and unavailable state.
    pub fn start_dictation(
        self: &Arc<Self>,
        request: StartDictationRequest,
        helper_executable: PathBuf,
    ) -> Result<DictationAccepted, AppCoreError> {
        let project_exists = self
            .projects()?
            .iter()
            .any(|project| project.id == request.binding.project_id);
        if !project_exists {
            return Err(AppCoreError::ProjectNotFound);
        }
        if let Some(conversation_id) = &request.binding.conversation_id
            && !self
                .conversation(conversation_id)?
                .is_some_and(|conversation| conversation.project_id == request.binding.project_id)
        {
            return Err(AppCoreError::InvalidDictationBinding);
        }
        self.speech_models
            .require_install_candidate(&request.model)?;
        let activity = self.foreground_activity.begin_dictation()?;
        self.dictation_sessions.begin(
            request.binding.clone(),
            activity,
            Box::new(ProductionResources {
                speech_models: Arc::clone(&self.speech_models),
                request,
                helper_executable,
                capture: MicrophoneCapture::new(CpalInputBackend),
            }),
        )
    }

    /// Requests recording completion idempotently for the exact composer binding.
    ///
    /// # Errors
    /// Rejects an unavailable coordinator or a mismatched composer binding.
    pub fn finish_dictation(&self, command: &DictationCommand) -> Result<bool, AppCoreError> {
        self.dictation_sessions.finish(command)
    }

    /// Cancels an exact session idempotently. No later final result can win.
    ///
    /// # Errors
    /// Rejects an unavailable coordinator or a mismatched composer binding.
    pub fn cancel_dictation(&self, command: &DictationCommand) -> Result<bool, AppCoreError> {
        self.dictation_sessions
            .cancel(command, DictationCancelReason::Cancelled)
    }

    /// Cancels active capture or transcription for a trusted host lifecycle event.
    pub fn cancel_active_dictation(&self, reason: DictationCancelReason) {
        self.dictation_sessions.cancel_active(reason);
    }

    /// Returns at most sixteen ordered events for the exact initiating binding.
    ///
    /// # Errors
    /// Rejects an unknown session, unavailable coordinator or mismatched binding.
    pub fn dictation_events(
        &self,
        request: &DictationEventsRequest,
    ) -> Result<Vec<DictationEvent>, AppCoreError> {
        self.dictation_sessions.events(request)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use audio_runtime::{InputLevel, NormalizedAudioFrame};

    use super::*;
    use crate::dictation::ForegroundActivityGate;

    struct FakeResources {
        prepare_until_cancelled: bool,
        preparation_release: Option<Arc<AtomicBool>>,
        capture_events: VecDeque<CaptureEvent>,
        recognition_release: Arc<AtomicBool>,
        recognition_result: RecognitionEvent,
        capture_drops: Arc<AtomicUsize>,
        recognition_drops: Arc<AtomicUsize>,
        received_samples: Arc<AtomicUsize>,
    }

    impl SessionResources for FakeResources {
        fn prepare_recognizer(
            &self,
            cancellation: &DownloadCancellation,
        ) -> Result<Box<dyn RecognitionSession>, SessionFailure> {
            while self.prepare_until_cancelled && !cancellation.is_cancelled() {
                thread::sleep(POLL_INTERVAL);
            }
            if let Some(release) = &self.preparation_release {
                while !release.load(Ordering::Acquire) && !cancellation.is_cancelled() {
                    thread::sleep(POLL_INTERVAL);
                }
            }
            if cancellation.is_cancelled() {
                return Err(SessionFailure::SpeechModelUnavailable);
            }
            Ok(Box::new(FakeRecognition {
                release: Arc::clone(&self.recognition_release),
                result: Some(self.recognition_result.clone()),
                drops: Arc::clone(&self.recognition_drops),
                received_samples: Arc::clone(&self.received_samples),
            }))
        }

        fn start_capture(&self) -> Result<Box<dyn AudioCaptureSession>, SessionFailure> {
            Ok(Box::new(FakeCapture {
                events: self.capture_events.clone(),
                drops: Arc::clone(&self.capture_drops),
            }))
        }
    }

    struct FakeCapture {
        events: VecDeque<CaptureEvent>,
        drops: Arc<AtomicUsize>,
    }

    impl AudioCaptureSession for FakeCapture {
        fn try_event(&mut self) -> Option<CaptureEvent> {
            self.events.pop_front()
        }

        fn stop_capture(&mut self, reason: CaptureStopReason) -> CaptureEvent {
            CaptureEvent::Stopped(reason)
        }
    }

    impl Drop for FakeCapture {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Release);
        }
    }

    struct FakeRecognition {
        release: Arc<AtomicBool>,
        result: Option<RecognitionEvent>,
        drops: Arc<AtomicUsize>,
        received_samples: Arc<AtomicUsize>,
    }

    impl RecognitionSession for FakeRecognition {
        fn push_audio(&mut self, frame: AudioFrame) -> Result<(), RecognitionError> {
            self.received_samples
                .fetch_add(frame.samples.len(), Ordering::Release);
            Ok(())
        }

        fn finish(
            mut self: Box<Self>,
            _cancellation: &RecognitionCancellation,
        ) -> Result<RecognitionEvent, RecognitionError> {
            while !self.release.load(Ordering::Acquire) {
                thread::sleep(POLL_INTERVAL);
            }
            self.result.take().ok_or(RecognitionError::HelperFailed)
        }

        fn cancel(self: Box<Self>) -> RecognitionEvent {
            RecognitionEvent::Cancelled { sequence: 1 }
        }
    }

    impl Drop for FakeRecognition {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Release);
        }
    }

    struct Fixture {
        sessions: Arc<DictationSessions>,
        gate: Arc<ForegroundActivityGate>,
        binding: DictationBinding,
        capture_drops: Arc<AtomicUsize>,
        recognition_drops: Arc<AtomicUsize>,
        received_samples: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                sessions: Arc::new(DictationSessions::new()),
                gate: Arc::new(ForegroundActivityGate::new()),
                binding: DictationBinding {
                    project_id: ipc_types::ProjectId("project".to_owned()),
                    conversation_id: None,
                    composer_id: "composer".to_owned(),
                    draft_revision: 7,
                },
                capture_drops: Arc::new(AtomicUsize::new(0)),
                recognition_drops: Arc::new(AtomicUsize::new(0)),
                received_samples: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn resources(
            &self,
            prepare_until_cancelled: bool,
            recognition_release: Arc<AtomicBool>,
        ) -> FakeResources {
            FakeResources {
                prepare_until_cancelled,
                preparation_release: None,
                capture_events: VecDeque::from([CaptureEvent::Frame {
                    frame: NormalizedAudioFrame {
                        sequence: 0,
                        samples: vec![0.25; 20_000],
                    },
                    level: InputLevel::silence(),
                }]),
                recognition_release,
                recognition_result: RecognitionEvent::Final {
                    sequence: 1,
                    text: "hello world".to_owned(),
                    language: "en".to_owned(),
                },
                capture_drops: Arc::clone(&self.capture_drops),
                recognition_drops: Arc::clone(&self.recognition_drops),
                received_samples: Arc::clone(&self.received_samples),
            }
        }

        fn resources_with_delayed_preparation(
            &self,
            preparation_release: Arc<AtomicBool>,
            recognition_release: Arc<AtomicBool>,
        ) -> FakeResources {
            let mut resources = self.resources(false, recognition_release);
            resources.preparation_release = Some(preparation_release);
            resources
        }

        fn begin(&self, resources: FakeResources) -> Result<DictationAccepted, AppCoreError> {
            self.sessions.begin(
                self.binding.clone(),
                self.gate.begin_dictation()?,
                Box::new(resources),
            )
        }

        fn command(&self, accepted: &DictationAccepted) -> DictationCommand {
            DictationCommand {
                session_id: accepted.session_id.clone(),
                binding: self.binding.clone(),
            }
        }

        fn events(
            &self,
            accepted: &DictationAccepted,
        ) -> Result<Vec<DictationEvent>, AppCoreError> {
            self.sessions.events(&DictationEventsRequest {
                session_id: accepted.session_id.clone(),
                binding: self.binding.clone(),
                after_sequence: 0,
            })
        }
    }

    fn wait_for(
        fixture: &Fixture,
        accepted: &DictationAccepted,
        predicate: impl Fn(&DictationEventKind) -> bool,
    ) -> Result<Vec<DictationEvent>, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let events = fixture.events(accepted)?;
            if events.iter().any(|event| predicate(&event.event)) {
                return Ok(events);
            }
            if Instant::now() >= deadline {
                return Err("dictation event timed out".into());
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn finish_emits_one_bound_result_and_releases_resources() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new();
        let release = Arc::new(AtomicBool::new(false));
        let accepted = fixture.begin(fixture.resources(false, Arc::clone(&release)))?;
        let command = fixture.command(&accepted);
        wait_for(&fixture, &accepted, |event| {
            matches!(event, DictationEventKind::Recording { .. })
        })?;
        assert!(fixture.sessions.finish(&command)?);
        assert!(fixture.sessions.finish(&command)?);
        release.store(true, Ordering::Release);
        let events = wait_for(&fixture, &accepted, DictationEventKind::is_terminal)?;

        assert_eq!(
            events
                .iter()
                .filter(|event| event.event.is_terminal())
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            &event.event,
            DictationEventKind::Completed { text, language }
                if text == "hello world" && language == "en"
        )));
        assert_eq!(fixture.received_samples.load(Ordering::Acquire), 20_000);
        assert_eq!(fixture.capture_drops.load(Ordering::Acquire), 1);
        assert_eq!(fixture.recognition_drops.load(Ordering::Acquire), 1);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert!(fixture.gate.begin_chat().is_ok());
        Ok(())
    }

    #[test]
    fn audio_is_buffered_while_model_verification_runs() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new();
        let preparation_release = Arc::new(AtomicBool::new(false));
        let recognition_release = Arc::new(AtomicBool::new(true));
        let accepted = fixture.begin(fixture.resources_with_delayed_preparation(
            Arc::clone(&preparation_release),
            recognition_release,
        ))?;
        let command = fixture.command(&accepted);
        wait_for(&fixture, &accepted, |event| {
            matches!(event, DictationEventKind::Recording { .. })
        })?;
        thread::sleep(Duration::from_millis(20));
        preparation_release.store(true, Ordering::Release);
        assert!(fixture.sessions.finish(&command)?);
        let events = wait_for(&fixture, &accepted, DictationEventKind::is_terminal)?;
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, DictationEventKind::Completed { .. }))
        );
        assert_eq!(fixture.received_samples.load(Ordering::Acquire), 20_000);
        Ok(())
    }

    #[test]
    fn cancellation_wins_a_final_result_race() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new();
        let release = Arc::new(AtomicBool::new(false));
        let accepted = fixture.begin(fixture.resources(false, Arc::clone(&release)))?;
        let command = fixture.command(&accepted);
        wait_for(&fixture, &accepted, |event| {
            matches!(event, DictationEventKind::Recording { .. })
        })?;
        assert!(fixture.sessions.finish(&command)?);
        wait_for(&fixture, &accepted, |event| {
            matches!(event, DictationEventKind::Transcribing { .. })
        })?;
        assert!(
            fixture
                .sessions
                .cancel(&command, DictationCancelReason::Navigation)?
        );
        assert!(
            fixture
                .sessions
                .cancel(&command, DictationCancelReason::Navigation)?
        );
        release.store(true, Ordering::Release);
        let events = wait_for(&fixture, &accepted, DictationEventKind::is_terminal)?;
        assert!(matches!(
            events.last().map(|event| &event.event),
            Some(DictationEventKind::Cancelled)
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, DictationEventKind::Completed { .. }))
        );
        Ok(())
    }

    #[test]
    fn capture_starts_while_model_verification_remains_cancellable() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new();
        let release = Arc::new(AtomicBool::new(true));
        let accepted = fixture.begin(fixture.resources(true, release))?;
        let command = fixture.command(&accepted);
        wait_for(&fixture, &accepted, |event| {
            matches!(event, DictationEventKind::Recording { .. })
        })?;
        assert_eq!(fixture.capture_drops.load(Ordering::Acquire), 0);
        assert!(
            fixture
                .sessions
                .cancel(&command, DictationCancelReason::Navigation)?
        );
        let events = wait_for(&fixture, &accepted, DictationEventKind::is_terminal)?;
        assert!(matches!(
            events.last().map(|event| &event.event),
            Some(DictationEventKind::Cancelled)
        ));
        assert_eq!(fixture.capture_drops.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[test]
    fn stale_or_cross_composer_commands_are_rejected() -> Result<(), Box<dyn Error>> {
        let fixture = Fixture::new();
        let accepted = fixture.begin(fixture.resources(false, Arc::new(AtomicBool::new(true))))?;
        let mut command = fixture.command(&accepted);
        command.binding.draft_revision += 1;
        assert!(matches!(
            fixture.sessions.finish(&command),
            Err(AppCoreError::InvalidDictationBinding)
        ));
        fixture
            .sessions
            .cancel_active(DictationCancelReason::Cancelled);
        wait_for(&fixture, &accepted, DictationEventKind::is_terminal)?;
        Ok(())
    }

    #[test]
    fn chat_and_dictation_share_exclusive_admission() -> Result<(), Box<dyn Error>> {
        let gate = Arc::new(ForegroundActivityGate::new());
        let chat = gate.begin_chat()?;
        assert!(matches!(
            gate.begin_dictation(),
            Err(AppCoreError::ForegroundActivityBusy)
        ));
        drop(chat);
        assert!(gate.begin_dictation().is_ok());
        Ok(())
    }
}
