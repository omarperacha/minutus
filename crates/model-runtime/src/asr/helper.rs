use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Instant,
};

use super::{
    AudioFormat, AudioFrame, RecognitionCancellation, RecognitionCapabilities, RecognitionConfig,
    RecognitionError, RecognitionEvent, RecognitionSession, SpeechModelLease, SpeechRecognizer,
    protocol,
    recognition::{MAX_AUDIO_BYTES, MAX_AUDIO_SAMPLES, MAX_FRAME_SAMPLES, RECOGNITION_DEADLINE},
    signal,
};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Pinned-helper launch settings with no audio-device or platform types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WhisperHelperConfig {
    executable: PathBuf,
    threads: u16,
}

impl WhisperHelperConfig {
    /// Creates a six-thread helper configuration.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            threads: 6,
        }
    }
    /// Applies a bounded native worker count.
    ///
    /// # Errors
    /// Rejects zero or more than 64 workers.
    pub fn with_threads(mut self, threads: u16) -> Result<Self, RecognitionError> {
        if !(1..=64).contains(&threads) {
            return Err(RecognitionError::InvalidConfiguration);
        }
        self.threads = threads;
        Ok(self)
    }
}

/// Final-only Whisper recognizer. One helper process is admitted at a time.
#[derive(Clone, Debug)]
pub struct WhisperHelper {
    config: WhisperHelperConfig,
    operation: Arc<Mutex<()>>,
}

impl WhisperHelper {
    /// Creates an idle recognizer; no model or process is loaded.
    #[must_use]
    pub fn new(config: WhisperHelperConfig) -> Self {
        Self {
            config,
            operation: Arc::new(Mutex::new(())),
        }
    }
}

impl SpeechRecognizer for WhisperHelper {
    fn capabilities(&self) -> RecognitionCapabilities {
        RecognitionCapabilities::FinalOnly
    }
    fn start(
        &self,
        model: SpeechModelLease,
        config: RecognitionConfig,
    ) -> Result<Box<dyn RecognitionSession>, RecognitionError> {
        Ok(Box::new(WhisperSession {
            helper: self.clone(),
            model,
            config,
            audio: FrameBuffer::default(),
        }))
    }
}

struct WhisperSession {
    helper: WhisperHelper,
    model: SpeechModelLease,
    config: RecognitionConfig,
    audio: FrameBuffer,
}

#[derive(Default)]
struct FrameBuffer {
    next_sequence: u64,
    samples: Vec<f32>,
}

impl FrameBuffer {
    fn push(&mut self, frame: AudioFrame) -> Result<(), RecognitionError> {
        if frame.sequence != self.next_sequence
            || frame.format != AudioFormat::WHISPER_PCM
            || frame.samples.is_empty()
            || frame.samples.len() > MAX_FRAME_SAMPLES
        {
            return Err(RecognitionError::InvalidAudio);
        }
        protocol::validate_samples(&frame.samples)?;
        let length = self
            .samples
            .len()
            .checked_add(frame.samples.len())
            .ok_or(RecognitionError::AudioLimitExceeded)?;
        let bytes = length
            .checked_mul(size_of::<f32>())
            .ok_or(RecognitionError::AudioLimitExceeded)?;
        if length > MAX_AUDIO_SAMPLES || bytes > MAX_AUDIO_BYTES {
            return Err(RecognitionError::AudioLimitExceeded);
        }
        self.samples.extend(frame.samples);
        self.next_sequence += 1;
        Ok(())
    }
}

impl RecognitionSession for WhisperSession {
    fn push_audio(&mut self, frame: AudioFrame) -> Result<(), RecognitionError> {
        self.audio.push(frame)
    }

    fn finish(
        self: Box<Self>,
        cancellation: &RecognitionCancellation,
    ) -> Result<RecognitionEvent, RecognitionError> {
        if cancellation.is_cancelled() {
            return Ok(RecognitionEvent::Cancelled { sequence: 1 });
        }
        if !signal::has_speech_energy(&self.audio.samples) {
            return Ok(RecognitionEvent::NoSpeech { sequence: 1 });
        }
        self.helper
            .execute(&self.model, &self.config, &self.audio.samples, cancellation)
    }
    fn cancel(self: Box<Self>) -> RecognitionEvent {
        RecognitionEvent::Cancelled { sequence: 1 }
    }
}

impl WhisperHelper {
    fn execute(
        &self,
        model: &SpeechModelLease,
        config: &RecognitionConfig,
        samples: &[f32],
        cancellation: &RecognitionCancellation,
    ) -> Result<RecognitionEvent, RecognitionError> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| RecognitionError::Unavailable)?;
        let executable = canonical_regular_file(&self.config.executable)?;
        let model = canonical_regular_file(model.artifact().path())?;
        let mut request = Vec::with_capacity(18 + samples.len() * 4);
        protocol::encode_request(&mut request, config, samples)?;
        if cancellation.is_cancelled() {
            return Ok(RecognitionEvent::Cancelled { sequence: 1 });
        }
        let mut child = launch(&executable, &model, self.config.threads)?;
        let result = supervise(&mut child, request, cancellation);
        let cleanup = terminate_and_reap(&mut child);
        match (result, cleanup) {
            (Ok(event), Ok(())) => Ok(event),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, RecognitionError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RecognitionError::Unavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RecognitionError::Unavailable);
    }
    path.canonicalize()
        .map_err(|_| RecognitionError::Unavailable)
}

fn launch(executable: &Path, model: &Path, threads: u16) -> Result<Child, RecognitionError> {
    Command::new(executable)
        .arg("--model")
        .arg(model)
        .arg("--threads")
        .arg(threads.to_string())
        .env_clear()
        .current_dir(Path::new("/"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| RecognitionError::Unavailable)
}

fn supervise(
    child: &mut Child,
    request: Vec<u8>,
    cancellation: &RecognitionCancellation,
) -> Result<RecognitionEvent, RecognitionError> {
    let mut stdin = child.stdin.take().ok_or(RecognitionError::Unavailable)?;
    let (write_tx, write_rx) = mpsc::sync_channel(1);
    let writer = thread::spawn(move || {
        let result = stdin
            .write_all(&request)
            .and_then(|()| stdin.flush())
            .map_err(|_| RecognitionError::Unavailable);
        drop(stdin);
        let _ = write_tx.send(result);
    });
    let mut stdout = child.stdout.take().ok_or(RecognitionError::Unavailable)?;
    let (read_tx, read_rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let result = protocol::decode_response(&mut stdout);
        let _ = read_tx.send(result);
    });
    let deadline = Instant::now() + RECOGNITION_DEADLINE;
    let result = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            break Ok(RecognitionEvent::Cancelled { sequence: 1 });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break Err(RecognitionError::DeadlineExceeded);
        }
        match read_rx.try_recv() {
            Ok(Err(RecognitionError::InvalidProtocol)) => {
                let status = child.wait().map_err(|_| RecognitionError::Unavailable)?;
                break if status.success() {
                    Err(RecognitionError::InvalidProtocol)
                } else {
                    Err(RecognitionError::HelperFailed)
                };
            }
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Disconnected) => break Err(RecognitionError::Unavailable),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Ok(Err(error)) = write_rx.try_recv() {
            let _ = child.kill();
            break Err(error);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| RecognitionError::Unavailable)?
        {
            let decoded = read_rx
                .recv_timeout(POLL_INTERVAL)
                .unwrap_or(Err(RecognitionError::HelperFailed));
            break if !status.success() && decoded == Err(RecognitionError::InvalidProtocol) {
                Err(RecognitionError::HelperFailed)
            } else {
                decoded
            };
        }
        thread::sleep(POLL_INTERVAL);
    };
    let _ = child.kill();
    let _ = reader.join();
    let _ = writer.join();
    result
}

fn terminate_and_reap(child: &mut Child) -> Result<(), RecognitionError> {
    if child
        .try_wait()
        .map_err(|_| RecognitionError::Unavailable)?
        .is_none()
    {
        child.kill().map_err(|_| RecognitionError::Unavailable)?;
    }
    child.wait().map_err(|_| RecognitionError::Unavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, thread, time::Duration};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    use super::*;

    fn frame(sequence: u64, samples: Vec<f32>) -> AudioFrame {
        AudioFrame {
            sequence,
            format: AudioFormat::WHISPER_PCM,
            samples,
        }
    }

    #[test]
    fn malformed_frames_and_aggregate_limits_are_rejected() -> Result<(), RecognitionError> {
        let mut audio = FrameBuffer::default();
        assert_eq!(
            audio.push(frame(1, vec![0.0])),
            Err(RecognitionError::InvalidAudio)
        );
        assert_eq!(
            audio.push(frame(0, Vec::new())),
            Err(RecognitionError::InvalidAudio)
        );
        assert_eq!(
            audio.push(frame(0, vec![f32::NAN])),
            Err(RecognitionError::InvalidAudio)
        );
        assert_eq!(
            audio.push(frame(0, vec![1.01])),
            Err(RecognitionError::InvalidAudio)
        );
        assert_eq!(
            audio.push(frame(0, vec![0.0; MAX_FRAME_SAMPLES + 1])),
            Err(RecognitionError::InvalidAudio)
        );
        for sequence in 0..120 {
            audio.push(frame(sequence, vec![0.0; MAX_FRAME_SAMPLES]))?;
        }
        assert_eq!(
            audio.push(frame(120, vec![0.0])),
            Err(RecognitionError::AudioLimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn capability_and_configuration_are_model_neutral() {
        let helper = WhisperHelper::new(WhisperHelperConfig::new("helper"));
        assert_eq!(helper.capabilities(), RecognitionCapabilities::FinalOnly);
        assert!(RecognitionConfig::automatic().with_language("EN").is_err());
        assert!(RecognitionConfig::automatic().with_language("e").is_err());
        assert!(WhisperHelperConfig::new("helper").with_threads(0).is_err());
        assert!(WhisperHelperConfig::new("helper").with_threads(65).is_err());
    }

    #[cfg(unix)]
    fn executable_script(root: &TempDir, body: &str) -> Result<PathBuf, Box<dyn Error>> {
        let path = root.path().join("fixture-helper.py");
        fs::write(&path, format!("#!/usr/bin/python3\n{body}\n"))?;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions)?;
        Ok(path)
    }

    #[cfg(unix)]
    fn run_fixture(
        body: &str,
        cancellation: &RecognitionCancellation,
    ) -> Result<RecognitionEvent, RecognitionError> {
        let root = TempDir::new().map_err(|_| RecognitionError::Unavailable)?;
        let executable =
            executable_script(&root, body).map_err(|_| RecognitionError::Unavailable)?;
        let model = root.path().join("model.bin");
        fs::write(&model, b"fixture").map_err(|_| RecognitionError::Unavailable)?;
        let mut child = launch(&executable, &model, 1)?;
        let result = supervise(&mut child, vec![0], cancellation);
        let cleanup = terminate_and_reap(&mut child);
        cleanup?;
        result
    }

    #[cfg(unix)]
    const FINAL_HEX: &str = "5a41535201000100010000000000000002000500656e68656c6c6f";

    #[cfg(unix)]
    #[test]
    fn fragmented_child_result_is_decoded() {
        let body = format!(
            "import sys, time\nsys.stdin.buffer.read()\nfor value in bytes.fromhex('{FINAL_HEX}'):\n sys.stdout.buffer.write(bytes([value])); sys.stdout.buffer.flush(); time.sleep(.001)"
        );
        assert_eq!(
            run_fixture(&body, &RecognitionCancellation::new()),
            Ok(RecognitionEvent::Final {
                sequence: 1,
                text: "hello".into(),
                language: "en".into(),
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn protocol_mismatch_and_child_exit_are_sanitized() {
        assert_eq!(
            run_fixture(
                "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write(b'wrong')",
                &RecognitionCancellation::new(),
            ),
            Err(RecognitionError::InvalidProtocol)
        );
        assert_eq!(
            run_fixture(
                "import sys\nsys.stdin.buffer.read()\nsys.exit(7)",
                &RecognitionCancellation::new(),
            ),
            Err(RecognitionError::HelperFailed)
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_wins_while_helper_is_loading_and_process_is_reaped()
    -> Result<(), Box<dyn Error>> {
        let cancellation = RecognitionCancellation::new();
        let trigger = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            trigger.cancel();
        });
        let result = run_fixture(
            "import sys, time\nsys.stdin.buffer.read()\ntime.sleep(60)",
            &cancellation,
        );
        canceller
            .join()
            .map_err(|_| "cancellation thread panicked")?;
        assert_eq!(result, Ok(RecognitionEvent::Cancelled { sequence: 1 }));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn terminal_race_produces_only_the_winning_result() -> Result<(), Box<dyn Error>> {
        let final_body = format!(
            "import sys, time\nsys.stdin.buffer.read()\ntime.sleep(.02)\nsys.stdout.buffer.write(bytes.fromhex('{FINAL_HEX}'))"
        );
        let final_cancellation = RecognitionCancellation::new();
        let late = final_cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_secs(2));
            late.cancel();
        });
        let final_result = run_fixture(&final_body, &final_cancellation);
        assert!(
            matches!(final_result, Ok(RecognitionEvent::Final { .. })),
            "{final_result:?}"
        );
        canceller
            .join()
            .map_err(|_| "cancellation thread panicked")?;

        let cancel_first = RecognitionCancellation::new();
        let early = cancel_first.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            early.cancel();
        });
        assert_eq!(
            run_fixture(&final_body, &cancel_first),
            Ok(RecognitionEvent::Cancelled { sequence: 1 })
        );
        canceller
            .join()
            .map_err(|_| "cancellation thread panicked")?;
        Ok(())
    }
}
