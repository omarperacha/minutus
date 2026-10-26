//! Conservative, model-neutral rejection of buffers without speech energy.

const ANALYSIS_FRAME_SAMPLES: usize = 320;
const MIN_ACTIVE_FRAMES: usize = 3;
const MIN_ACTIVE_FRAME_RMS: f32 = 0.004;
const SPEECH_TO_NOISE_RATIO: f32 = 3.0;

/// Returns whether a 16 kHz mono buffer has enough sustained energy to warrant
/// model inference. This rejects silence and low device noise, while leaving
/// speech/noise discrimination to the recognizer.
pub(super) fn has_speech_energy(samples: &[f32]) -> bool {
    let levels = samples
        .chunks_exact(ANALYSIS_FRAME_SAMPLES)
        .map(root_mean_square)
        .collect::<Vec<_>>();
    if levels.len() < MIN_ACTIVE_FRAMES {
        return false;
    }
    let mut ordered = levels.clone();
    ordered.sort_by(f32::total_cmp);
    let noise_floor = ordered[ordered.len() / 5];
    let active_threshold = (noise_floor * SPEECH_TO_NOISE_RATIO).max(MIN_ACTIVE_FRAME_RMS);
    levels
        .into_iter()
        .filter(|level| *level >= active_threshold)
        .take(MIN_ACTIVE_FRAMES)
        .count()
        == MIN_ACTIVE_FRAMES
}

fn root_mean_square(samples: &[f32]) -> f32 {
    let squared = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    #[allow(clippy::cast_possible_truncation)]
    // Validated samples and a fixed 320-sample frame bound the result to 0..=1.
    let rms = (squared / 320.0).sqrt() as f32;
    rms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_stationary_device_noise_do_not_reach_inference() {
        assert!(!has_speech_energy(&vec![0.0; ANALYSIS_FRAME_SAMPLES * 10]));
        assert!(!has_speech_energy(&vec![
            0.006;
            ANALYSIS_FRAME_SAMPLES * 10
        ]));
    }

    #[test]
    fn quiet_speech_above_its_recording_noise_floor_is_admitted() {
        let mut samples = vec![0.001; ANALYSIS_FRAME_SAMPLES * 10];
        samples[ANALYSIS_FRAME_SAMPLES * 3..ANALYSIS_FRAME_SAMPLES * 6].fill(0.005);
        assert!(has_speech_energy(&samples));
    }

    #[test]
    fn a_short_click_is_not_mistaken_for_speech() {
        let mut samples = vec![0.0; ANALYSIS_FRAME_SAMPLES * 10];
        samples[..ANALYSIS_FRAME_SAMPLES].fill(0.5);
        assert!(!has_speech_energy(&samples));
    }
}
