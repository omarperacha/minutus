use crate::{CaptureError, NativeAudioChunk, OUTPUT_SAMPLE_RATE_HZ};

pub(crate) struct Normalizer {
    sample_rate_hz: Option<u32>,
    channels: Option<u16>,
    previous: Option<f32>,
    position_numerator: u64,
}

impl Normalizer {
    pub(crate) const fn new() -> Self {
        Self {
            sample_rate_hz: None,
            channels: None,
            previous: None,
            position_numerator: 0,
        }
    }

    pub(crate) fn convert(&mut self, chunk: &NativeAudioChunk) -> Result<Vec<f32>, CaptureError> {
        validate(chunk)?;
        if self
            .sample_rate_hz
            .is_some_and(|rate| rate != chunk.sample_rate_hz)
            || self
                .channels
                .is_some_and(|channels| channels != chunk.channels)
        {
            return Err(CaptureError::DeviceLost);
        }
        self.sample_rate_hz = Some(chunk.sample_rate_hz);
        self.channels = Some(chunk.channels);
        let channels = usize::from(chunk.channels);
        let mono: Vec<f32> = chunk
            .samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / f32::from(chunk.channels))
            .collect();
        if chunk.sample_rate_hz == OUTPUT_SAMPLE_RATE_HZ {
            return Ok(mono);
        }
        self.resample(&mono, chunk.sample_rate_hz)
    }

    fn resample(&mut self, mono: &[f32], sample_rate_hz: u32) -> Result<Vec<f32>, CaptureError> {
        let mut input = Vec::with_capacity(mono.len() + 1);
        if let Some(previous) = self.previous {
            input.push(previous);
        }
        input.extend_from_slice(mono);
        self.previous = input.last().copied();
        if input.len() < 2 {
            return Ok(Vec::new());
        }
        let output_rate = u64::from(OUTPUT_SAMPLE_RATE_HZ);
        let input_rate = u64::from(sample_rate_hz);
        let last_index =
            u64::try_from(input.len() - 1).map_err(|_| CaptureError::UnsupportedFormat)?;
        let mut output = Vec::new();
        while self.position_numerator < last_index * output_rate {
            let lower = usize::try_from(self.position_numerator / output_rate)
                .map_err(|_| CaptureError::UnsupportedFormat)?;
            let remainder = u16::try_from(self.position_numerator % output_rate)
                .map_err(|_| CaptureError::UnsupportedFormat)?;
            let output_rate_f32 = f32::from(
                u16::try_from(OUTPUT_SAMPLE_RATE_HZ)
                    .map_err(|_| CaptureError::UnsupportedFormat)?,
            );
            let fraction = f32::from(remainder) / output_rate_f32;
            output.push(input[lower] + (input[lower + 1] - input[lower]) * fraction);
            self.position_numerator += input_rate;
        }
        self.position_numerator -= last_index * output_rate;
        Ok(output)
    }
}

fn validate(chunk: &NativeAudioChunk) -> Result<(), CaptureError> {
    if !(8_000..=192_000).contains(&chunk.sample_rate_hz)
        || !(1..=32).contains(&chunk.channels)
        || chunk.samples.is_empty()
        || !chunk
            .samples
            .len()
            .is_multiple_of(usize::from(chunk.channels))
        || chunk
            .samples
            .iter()
            .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
    {
        return Err(CaptureError::UnsupportedFormat);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(sample_rate_hz: u32, channels: u16, samples: Vec<f32>) -> NativeAudioChunk {
        NativeAudioChunk {
            sample_rate_hz,
            channels,
            samples,
        }
    }

    #[test]
    fn downmixes_stereo_and_preserves_sixteen_kilohertz() {
        let result = Normalizer::new().convert(&chunk(16_000, 2, vec![1.0, -1.0, 0.5, 0.25]));
        assert!(result.is_ok());
        let Ok(samples) = result else { return };

        assert_eq!(samples, vec![0.0, 0.375]);
    }

    #[test]
    fn resamples_across_callback_boundaries() {
        let mut normalizer = Normalizer::new();
        let first = normalizer.convert(&chunk(48_000, 1, vec![0.0, 0.25, 0.5, 0.75]));
        assert!(first.is_ok());
        let Ok(first) = first else { return };
        let second = normalizer.convert(&chunk(48_000, 1, vec![1.0, 0.75, 0.5, 0.25]));
        assert!(second.is_ok());
        let Ok(second) = second else { return };

        assert_eq!(first, vec![0.0]);
        assert_eq!(second, vec![0.75, 0.5]);
    }

    #[test]
    fn rejects_format_changes_and_invalid_samples() {
        let mut normalizer = Normalizer::new();
        assert!(normalizer.convert(&chunk(16_000, 1, vec![0.0])).is_ok());
        assert_eq!(
            normalizer.convert(&chunk(48_000, 1, vec![0.0])),
            Err(CaptureError::DeviceLost)
        );
        assert_eq!(
            Normalizer::new().convert(&chunk(16_000, 1, vec![f32::NAN])),
            Err(CaptureError::UnsupportedFormat)
        );
    }
}
