use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
    mpsc::SyncSender,
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{CaptureError, MAX_CALLBACK_SAMPLES, NativeAudioChunk};

const NO_ERROR: u8 = 0;
const DEVICE_LOST: u8 = 1;
const PERMISSION_DENIED: u8 = 2;
const START_FAILED: u8 = 3;
const UNSUPPORTED_FORMAT: u8 = 4;

/// Non-blocking callback endpoint passed to an input backend.
#[derive(Clone)]
pub struct InputSink {
    sender: SyncSender<NativeAudioChunk>,
    overflowed: Arc<AtomicBool>,
    error: Arc<AtomicU8>,
    stopped: Arc<AtomicBool>,
}

impl InputSink {
    pub(crate) fn new(
        sender: SyncSender<NativeAudioChunk>,
        overflowed: Arc<AtomicBool>,
        error: Arc<AtomicU8>,
        stopped: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender,
            overflowed,
            error,
            stopped,
        }
    }

    /// Copies one callback buffer without waiting for downstream work.
    pub fn push(&self, chunk: NativeAudioChunk) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        if chunk.samples.len() > MAX_CALLBACK_SAMPLES || self.sender.try_send(chunk).is_err() {
            self.overflowed.store(true, Ordering::Release);
            self.stopped.store(true, Ordering::Release);
        }
    }

    /// Reports a callback buffer that could not be copied within the hard bound.
    pub fn overflow(&self) {
        self.overflowed.store(true, Ordering::Release);
        self.stopped.store(true, Ordering::Release);
    }

    /// Reports terminal device failure without carrying backend text.
    pub fn fail(&self, error: CaptureError) {
        let code = match error {
            CaptureError::PermissionDenied => PERMISSION_DENIED,
            CaptureError::DeviceLost => DEVICE_LOST,
            CaptureError::UnsupportedFormat => UNSUPPORTED_FORMAT,
            _ => START_FAILED,
        };
        self.error.store(code, Ordering::Release);
        self.stopped.store(true, Ordering::Release);
    }

    pub(crate) fn terminal_error(&self) -> Option<CaptureError> {
        match self.error.load(Ordering::Acquire) {
            NO_ERROR => None,
            DEVICE_LOST => Some(CaptureError::DeviceLost),
            PERMISSION_DENIED => Some(CaptureError::PermissionDenied),
            UNSUPPORTED_FORMAT => Some(CaptureError::UnsupportedFormat),
            _ => Some(CaptureError::StartFailed),
        }
    }
}

/// Opaque native stream whose drop releases the input device.
pub trait ActiveInputStream: Send {}

/// Portable input backend boundary used by production and fake devices.
pub trait InputBackend: Send + Sync {
    /// Opens and starts the default input device.
    ///
    /// # Errors
    /// Returns a closed capture error if no usable stream can start.
    fn start(&self, sink: InputSink) -> Result<Box<dyn ActiveInputStream>, CaptureError>;
}

/// CPAL default-device backend. Platform types do not cross this boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpalInputBackend;

struct CpalActiveStream {
    _stream: cpal::Stream,
}
impl ActiveInputStream for CpalActiveStream {}

impl InputBackend for CpalInputBackend {
    fn start(&self, sink: InputSink) -> Result<Box<dyn ActiveInputStream>, CaptureError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(CaptureError::NoInputDevice)?;
        let supported = device
            .default_input_config()
            .map_err(|error| map_start_error(&error))?;
        let sample_format = supported.sample_format();
        let config = supported.into();
        let stream = match sample_format {
            SampleFormat::I8 => build::<i8>(&device, config, sink)?,
            SampleFormat::I16 => build::<i16>(&device, config, sink)?,
            SampleFormat::I32 => build::<i32>(&device, config, sink)?,
            SampleFormat::I64 => build::<i64>(&device, config, sink)?,
            SampleFormat::U8 => build::<u8>(&device, config, sink)?,
            SampleFormat::U16 => build::<u16>(&device, config, sink)?,
            SampleFormat::U32 => build::<u32>(&device, config, sink)?,
            SampleFormat::U64 => build::<u64>(&device, config, sink)?,
            SampleFormat::F32 => build::<f32>(&device, config, sink)?,
            SampleFormat::F64 => build::<f64>(&device, config, sink)?,
            _ => return Err(CaptureError::UnsupportedFormat),
        };
        stream.play().map_err(|error| map_start_error(&error))?;
        Ok(Box::new(CpalActiveStream { _stream: stream }))
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: InputSink,
) -> Result<cpal::Stream, CaptureError>
where
    T: SizedSample + Copy,
    f32: FromSample<T>,
{
    let sample_rate_hz = config.sample_rate;
    let channels = config.channels;
    let failure_sink = sink.clone();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _| {
                if data.len() > MAX_CALLBACK_SAMPLES {
                    sink.overflow();
                    return;
                }
                let samples = data
                    .iter()
                    .copied()
                    .map(<f32 as Sample>::from_sample)
                    .collect();
                sink.push(NativeAudioChunk {
                    sample_rate_hz,
                    channels,
                    samples,
                });
            },
            move |error| failure_sink.fail(map_runtime_error(&error)),
            Some(std::time::Duration::from_secs(5)),
        )
        .map_err(|error| map_start_error(&error))
}

fn map_start_error(error: &cpal::Error) -> CaptureError {
    match error.kind() {
        cpal::ErrorKind::PermissionDenied => CaptureError::PermissionDenied,
        cpal::ErrorKind::DeviceNotAvailable => CaptureError::NoInputDevice,
        cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
            CaptureError::UnsupportedFormat
        }
        _ => CaptureError::StartFailed,
    }
}

fn map_runtime_error(error: &cpal::Error) -> CaptureError {
    match error.kind() {
        cpal::ErrorKind::PermissionDenied => CaptureError::PermissionDenied,
        _ => CaptureError::DeviceLost,
    }
}
