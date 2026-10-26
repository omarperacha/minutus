use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    ActiveInputStream, CaptureAuthorization, CaptureError, CaptureEvent, CaptureStopReason,
    InputBackend, InputLevel, InputSink, MAX_CAPTURE_SAMPLES, NativeAudioChunk,
    NormalizedAudioFrame, normalize::Normalizer,
};

const RAW_QUEUE_CAPACITY: usize = 32;
const EVENT_QUEUE_CAPACITY: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const COLD_START_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(300),
];

/// Object-safe capture factory used by the application composition layer.
pub trait AudioCapture: Send + Sync {
    /// Starts capture following a direct user action.
    ///
    /// # Errors
    /// Returns a typed permission, device, format or startup error.
    fn start_capture(
        &self,
        authorization: CaptureAuthorization,
    ) -> Result<Box<dyn AudioCaptureSession>, CaptureError>;
}

/// Object-safe active capture boundary with no native platform types.
pub trait AudioCaptureSession: Send {
    /// Returns the next event without waiting.
    fn try_event(&mut self) -> Option<CaptureEvent>;
    /// Stops and releases capture for a host-owned reason.
    fn stop_capture(&mut self, reason: CaptureStopReason) -> CaptureEvent;
}

/// Starts bounded capture through a replaceable native backend.
pub struct MicrophoneCapture<B> {
    backend: B,
}

impl<B> MicrophoneCapture<B>
where
    B: InputBackend,
{
    /// Creates an idle capture service and performs no device access.
    #[must_use]
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Starts only with a desktop-issued user-action authorization.
    ///
    /// # Errors
    /// Returns a closed permission, device, format or startup error.
    pub fn start(
        &self,
        _authorization: CaptureAuthorization,
    ) -> Result<CaptureSession, CaptureError> {
        self.start_with_limit(MAX_CAPTURE_SAMPLES)
    }

    fn start_with_limit(&self, capture_limit: usize) -> Result<CaptureSession, CaptureError> {
        for delay in COLD_START_RETRY_DELAYS {
            match self.start_once(capture_limit) {
                Err(CaptureError::NoInputDevice | CaptureError::StartFailed) => {
                    thread::sleep(delay);
                }
                result => return result,
            }
        }
        self.start_once(capture_limit)
    }

    fn start_once(&self, capture_limit: usize) -> Result<CaptureSession, CaptureError> {
        let (raw_tx, raw_rx) = mpsc::sync_channel(RAW_QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let overflowed = Arc::new(AtomicBool::new(false));
        let duration_limited = Arc::new(AtomicBool::new(false));
        let error = Arc::new(AtomicU8::new(0));
        let sink = InputSink::new(
            raw_tx,
            Arc::clone(&overflowed),
            Arc::clone(&error),
            Arc::clone(&stopped),
        );
        let stream = self.backend.start(sink.clone())?;
        let worker_stopped = Arc::clone(&stopped);
        let worker_overflowed = Arc::clone(&overflowed);
        let worker_duration_limited = Arc::clone(&duration_limited);
        let worker = thread::spawn(move || {
            normalize_loop(
                &raw_rx,
                &event_tx,
                &sink,
                &worker_stopped,
                &worker_overflowed,
                &worker_duration_limited,
                capture_limit,
            );
        });
        let stream_stopped = Arc::clone(&stopped);
        let stream_owner = thread::spawn(move || own_stream(stream, &stream_stopped));
        Ok(CaptureSession {
            events: event_rx,
            stopped,
            overflowed,
            duration_limited,
            error,
            terminal_delivered: false,
            worker: Some(worker),
            stream_owner: Some(stream_owner),
        })
    }
}

impl<B> AudioCapture for MicrophoneCapture<B>
where
    B: InputBackend,
{
    fn start_capture(
        &self,
        authorization: CaptureAuthorization,
    ) -> Result<Box<dyn AudioCaptureSession>, CaptureError> {
        self.start(authorization)
            .map(|session| Box::new(session) as Box<dyn AudioCaptureSession>)
    }
}

/// Active capture handle. Drop always stops and releases the native stream.
pub struct CaptureSession {
    events: Receiver<CaptureEvent>,
    stopped: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    duration_limited: Arc<AtomicBool>,
    error: Arc<AtomicU8>,
    terminal_delivered: bool,
    worker: Option<JoinHandle<()>>,
    stream_owner: Option<JoinHandle<()>>,
}

impl CaptureSession {
    /// Returns the next event without waiting.
    #[must_use]
    pub fn try_event(&mut self) -> Option<CaptureEvent> {
        if self.terminal_delivered {
            return None;
        }
        if let Ok(event) = self.events.try_recv() {
            if matches!(event, CaptureEvent::Failed(_) | CaptureEvent::Stopped(_)) {
                self.terminal_delivered = true;
            }
            return Some(event);
        }
        if self.duration_limited.load(Ordering::Acquire) {
            self.terminal_delivered = true;
            return Some(CaptureEvent::Stopped(CaptureStopReason::DurationLimit));
        }
        let error = if self.overflowed.load(Ordering::Acquire) {
            Some(CaptureError::Overflow)
        } else {
            match self.error.load(Ordering::Acquire) {
                1 => Some(CaptureError::DeviceLost),
                2 => Some(CaptureError::PermissionDenied),
                4 => Some(CaptureError::UnsupportedFormat),
                3 | 5..=u8::MAX => Some(CaptureError::StartFailed),
                0 => None,
            }
        };
        if error.is_some() {
            self.terminal_delivered = true;
        }
        error.map(CaptureEvent::Failed)
    }

    /// Stops capture for a host-owned reason. Repeated calls are harmless.
    pub fn stop(&mut self, reason: CaptureStopReason) -> CaptureEvent {
        self.stopped.store(true, Ordering::Release);
        self.join();
        CaptureEvent::Stopped(reason)
    }

    fn join(&mut self) {
        if let Some(owner) = self.stream_owner.take() {
            let _ = owner.join();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl AudioCaptureSession for CaptureSession {
    fn try_event(&mut self) -> Option<CaptureEvent> {
        Self::try_event(self)
    }

    fn stop_capture(&mut self, reason: CaptureStopReason) -> CaptureEvent {
        self.stop(reason)
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.join();
    }
}

fn own_stream(_stream: Box<dyn ActiveInputStream>, stopped: &AtomicBool) {
    while !stopped.load(Ordering::Acquire) {
        thread::sleep(POLL_INTERVAL);
    }
}

fn normalize_loop(
    raw: &Receiver<NativeAudioChunk>,
    events: &SyncSender<CaptureEvent>,
    sink: &InputSink,
    stopped: &AtomicBool,
    overflowed: &AtomicBool,
    duration_limited: &AtomicBool,
    capture_limit: usize,
) {
    let mut normalizer = Normalizer::new();
    let mut sequence = 0;
    let mut sample_count = 0;
    loop {
        if overflowed.load(Ordering::Acquire) {
            emit_terminal(
                events,
                CaptureEvent::Failed(CaptureError::Overflow),
                stopped,
            );
            return;
        }
        if let Some(error) = sink.terminal_error() {
            emit_terminal(events, CaptureEvent::Failed(error), stopped);
            return;
        }
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let chunk = match raw.recv_timeout(POLL_INTERVAL) {
            Ok(chunk) => chunk,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let samples = match normalizer.convert(&chunk) {
            Ok(samples) => samples,
            Err(error) => {
                sink.fail(error);
                emit_terminal(events, CaptureEvent::Failed(error), stopped);
                return;
            }
        };
        if samples.is_empty() {
            continue;
        }
        let remaining = capture_limit.saturating_sub(sample_count);
        let limit_reached = samples.len() >= remaining;
        let samples = samples.into_iter().take(remaining).collect::<Vec<_>>();
        sample_count += samples.len();
        let level = InputLevel::from_samples(&samples);
        if events
            .try_send(CaptureEvent::Frame {
                frame: NormalizedAudioFrame { sequence, samples },
                level,
            })
            .is_err()
        {
            overflowed.store(true, Ordering::Release);
            stopped.store(true, Ordering::Release);
            return;
        }
        sequence += 1;
        if limit_reached {
            duration_limited.store(true, Ordering::Release);
            emit_terminal(
                events,
                CaptureEvent::Stopped(CaptureStopReason::DurationLimit),
                stopped,
            );
            return;
        }
    }
}

fn emit_terminal(events: &SyncSender<CaptureEvent>, event: CaptureEvent, stopped: &AtomicBool) {
    let _ = events.try_send(event);
    stopped.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::OUTPUT_SAMPLE_RATE_HZ;

    #[derive(Clone)]
    struct FakeBackend {
        action: FakeAction,
        drops: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    enum FakeAction {
        Idle,
        Chunks(Vec<NativeAudioChunk>),
        Fail(CaptureError),
        Reject(CaptureError),
        RejectOnce(CaptureError, Arc<AtomicUsize>),
    }

    struct FakeStream(Arc<AtomicUsize>);

    impl Drop for FakeStream {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Release);
        }
    }

    impl ActiveInputStream for FakeStream {}

    impl InputBackend for FakeBackend {
        fn start(&self, sink: InputSink) -> Result<Box<dyn ActiveInputStream>, CaptureError> {
            match &self.action {
                FakeAction::Chunks(chunks) => {
                    for chunk in chunks {
                        sink.push(chunk.clone());
                    }
                }
                FakeAction::Fail(error) => sink.fail(*error),
                FakeAction::Reject(error) => return Err(*error),
                FakeAction::RejectOnce(error, attempts)
                    if attempts.fetch_add(1, Ordering::AcqRel) == 0 =>
                {
                    return Err(*error);
                }
                FakeAction::Idle | FakeAction::RejectOnce(_, _) => {}
            }
            Ok(Box::new(FakeStream(Arc::clone(&self.drops))))
        }
    }

    fn authorization() -> CaptureAuthorization {
        CaptureAuthorization::after_user_action()
    }

    fn chunk(samples: Vec<f32>) -> NativeAudioChunk {
        NativeAudioChunk {
            sample_rate_hz: OUTPUT_SAMPLE_RATE_HZ,
            channels: 1,
            samples,
        }
    }

    fn wait_for_event(session: &mut CaptureSession) -> CaptureEvent {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(event) = session.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "capture event timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn explicit_stop_releases_stream_and_preserves_reason() {
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = MicrophoneCapture::new(FakeBackend {
            action: FakeAction::Idle,
            drops: Arc::clone(&drops),
        });
        let session = capture.start(authorization());
        assert!(session.is_ok());
        let Ok(mut session) = session else { return };

        assert_eq!(
            session.stop(CaptureStopReason::Navigation),
            CaptureEvent::Stopped(CaptureStopReason::Navigation)
        );
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn duration_limit_truncates_frame_and_stops_stream() {
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = MicrophoneCapture::new(FakeBackend {
            action: FakeAction::Chunks(vec![chunk(vec![0.25; 8])]),
            drops: Arc::clone(&drops),
        });
        let session = capture.start_with_limit(4);
        assert!(session.is_ok());
        let Ok(mut session) = session else { return };

        let event = wait_for_event(&mut session);
        assert!(matches!(event, CaptureEvent::Frame { .. }));
        let CaptureEvent::Frame { frame, level } = event else {
            return;
        };
        assert_eq!(frame.samples, vec![0.25; 4]);
        assert!((level.value() - 0.25).abs() < f32::EPSILON);
        assert_eq!(
            wait_for_event(&mut session),
            CaptureEvent::Stopped(CaptureStopReason::DurationLimit)
        );
        session.join();
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn callback_overflow_is_terminal_and_releases_stream() {
        let drops = Arc::new(AtomicUsize::new(0));
        let chunks = (0..=RAW_QUEUE_CAPACITY).map(|_| chunk(vec![0.0])).collect();
        let capture = MicrophoneCapture::new(FakeBackend {
            action: FakeAction::Chunks(chunks),
            drops: Arc::clone(&drops),
        });
        let session = capture.start(authorization());
        assert!(session.is_ok());
        let Ok(mut session) = session else { return };

        assert_eq!(
            wait_for_event(&mut session),
            CaptureEvent::Failed(CaptureError::Overflow)
        );
        session.join();
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn runtime_device_loss_is_typed_and_releases_stream() {
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = MicrophoneCapture::new(FakeBackend {
            action: FakeAction::Fail(CaptureError::DeviceLost),
            drops: Arc::clone(&drops),
        });
        let session = capture.start(authorization());
        assert!(session.is_ok());
        let Ok(mut session) = session else { return };

        assert_eq!(
            wait_for_event(&mut session),
            CaptureEvent::Failed(CaptureError::DeviceLost)
        );
        session.join();
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn startup_errors_do_not_create_a_session() {
        for error in [
            CaptureError::PermissionDenied,
            CaptureError::NoInputDevice,
            CaptureError::StartFailed,
        ] {
            let capture = MicrophoneCapture::new(FakeBackend {
                action: FakeAction::Reject(error),
                drops: Arc::new(AtomicUsize::new(0)),
            });
            assert!(matches!(capture.start(authorization()), Err(actual) if actual == error));
        }
    }

    #[test]
    fn transient_cold_start_failure_is_retried_without_user_action() {
        for error in [CaptureError::NoInputDevice, CaptureError::StartFailed] {
            let attempts = Arc::new(AtomicUsize::new(0));
            let drops = Arc::new(AtomicUsize::new(0));
            let capture = MicrophoneCapture::new(FakeBackend {
                action: FakeAction::RejectOnce(error, Arc::clone(&attempts)),
                drops: Arc::clone(&drops),
            });
            let session = capture.start(authorization());
            assert!(session.is_ok());
            assert_eq!(attempts.load(Ordering::Acquire), 2);
            drop(session);
            assert_eq!(drops.load(Ordering::Acquire), 1);
        }
    }
}
