use std::io::{self, Read, Write};

use super::{
    RecognitionAcceleration, RecognitionConfig, RecognitionError, RecognitionEvent,
    recognition::MAX_AUDIO_SAMPLES,
};

const MAGIC: [u8; 4] = *b"ZASR";
pub(super) const VERSION: u16 = 1;
const REQUEST: u8 = 1;
const FINAL: u8 = 1;
const NO_SPEECH: u8 = 2;
const FAILURE: u8 = 3;
const MAX_TRANSCRIPT_BYTES: usize = u16::MAX as usize;
const MAX_LANGUAGE_BYTES: usize = 8;

pub(super) fn encode_request(
    mut writer: impl Write,
    config: &RecognitionConfig,
    samples: &[f32],
) -> Result<(), RecognitionError> {
    writer.write_all(&MAGIC).map_err(io_error)?;
    writer.write_all(&VERSION.to_le_bytes()).map_err(io_error)?;
    writer
        .write_all(&[
            REQUEST,
            match config.acceleration() {
                RecognitionAcceleration::Automatic => 0,
                RecognitionAcceleration::Cpu => 1,
            },
        ])
        .map_err(io_error)?;
    writer
        .write_all(&(samples.len() as u64).to_le_bytes())
        .map_err(io_error)?;
    let language = config.language().unwrap_or("").as_bytes();
    let language_len =
        u16::try_from(language.len()).map_err(|_| RecognitionError::InvalidConfiguration)?;
    writer
        .write_all(&language_len.to_le_bytes())
        .map_err(io_error)?;
    writer.write_all(language).map_err(io_error)?;
    for sample in samples {
        writer.write_all(&sample.to_le_bytes()).map_err(io_error)?;
    }
    writer.flush().map_err(io_error)
}

pub(super) fn decode_response(mut reader: impl Read) -> Result<RecognitionEvent, RecognitionError> {
    let mut fixed = [0; 20];
    reader
        .read_exact(&mut fixed)
        .map_err(|_| RecognitionError::InvalidProtocol)?;
    if fixed[..4] != MAGIC || u16::from_le_bytes([fixed[4], fixed[5]]) != VERSION || fixed[7] != 0 {
        return Err(RecognitionError::InvalidProtocol);
    }
    let kind = fixed[6];
    let sequence = u64::from_le_bytes(
        fixed[8..16]
            .try_into()
            .map_err(|_| RecognitionError::InvalidProtocol)?,
    );
    if sequence != 1 {
        return Err(RecognitionError::InvalidProtocol);
    }
    let language_len = usize::from(u16::from_le_bytes([fixed[16], fixed[17]]));
    let text_len = usize::from(u16::from_le_bytes([fixed[18], fixed[19]]));
    if language_len > MAX_LANGUAGE_BYTES || text_len > MAX_TRANSCRIPT_BYTES {
        return Err(RecognitionError::InvalidProtocol);
    }
    let mut language = vec![0; language_len];
    let mut text = vec![0; text_len];
    reader
        .read_exact(&mut language)
        .and_then(|()| reader.read_exact(&mut text))
        .map_err(|_| RecognitionError::InvalidProtocol)?;
    let language = String::from_utf8(language).map_err(|_| RecognitionError::InvalidProtocol)?;
    let text = String::from_utf8(text).map_err(|_| RecognitionError::InvalidProtocol)?;
    let mut trailing = [0];
    if reader
        .read(&mut trailing)
        .map_err(|_| RecognitionError::InvalidProtocol)?
        != 0
    {
        return Err(RecognitionError::InvalidProtocol);
    }
    let valid_language = (2..=MAX_LANGUAGE_BYTES).contains(&language.len())
        && language
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-');
    match kind {
        FINAL if !text.trim().is_empty() && valid_language => Ok(RecognitionEvent::Final {
            sequence,
            text,
            language,
        }),
        NO_SPEECH if text.is_empty() && language.is_empty() => {
            Ok(RecognitionEvent::NoSpeech { sequence })
        }
        FAILURE => Err(RecognitionError::HelperFailed),
        _ => Err(RecognitionError::InvalidProtocol),
    }
}

pub(super) fn validate_samples(samples: &[f32]) -> Result<(), RecognitionError> {
    if samples.len() > MAX_AUDIO_SAMPLES
        || samples
            .iter()
            .any(|value| !value.is_finite() || !(-1.0..=1.0).contains(value))
    {
        return Err(RecognitionError::InvalidAudio);
    }
    Ok(())
}

fn io_error(_: io::Error) -> RecognitionError {
    RecognitionError::Unavailable
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::*;

    struct Fragmented {
        bytes: Vec<u8>,
        offset: usize,
        fragment: usize,
    }

    impl Read for Fragmented {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let length = output
                .len()
                .min(self.fragment)
                .min(self.bytes.len().saturating_sub(self.offset));
            output[..length].copy_from_slice(&self.bytes[self.offset..self.offset + length]);
            self.offset += length;
            Ok(length)
        }
    }

    fn response(kind: u8, sequence: u64, language: &[u8], text: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&[kind, 0]);
        bytes.extend_from_slice(&sequence.to_le_bytes());
        bytes.extend_from_slice(
            &u16::try_from(language.len())
                .unwrap_or(u16::MAX)
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&u16::try_from(text.len()).unwrap_or(u16::MAX).to_le_bytes());
        bytes.extend_from_slice(language);
        bytes.extend_from_slice(text);
        bytes
    }

    #[test]
    fn arbitrary_fragmentation_preserves_final_result() {
        let bytes = response(FINAL, 1, b"en", b"hello");
        for fragment in 1..=bytes.len() {
            let result = decode_response(Fragmented {
                bytes: bytes.clone(),
                offset: 0,
                fragment,
            });
            assert_eq!(
                result,
                Ok(RecognitionEvent::Final {
                    sequence: 1,
                    text: "hello".into(),
                    language: "en".into(),
                })
            );
        }
    }

    #[test]
    fn malformed_and_mismatched_results_are_rejected() {
        let mut bad_magic = response(FINAL, 1, b"en", b"hello");
        bad_magic[0] ^= 1;
        let mut bad_version = response(FINAL, 1, b"en", b"hello");
        bad_version[4] = VERSION.to_le_bytes()[0].wrapping_add(1);
        let mut invalid_utf8 = response(FINAL, 1, b"en", &[0xff]);
        let mut trailing = response(NO_SPEECH, 1, b"", b"");
        trailing.push(0);
        for bytes in [
            Vec::new(),
            bad_magic,
            bad_version,
            response(FINAL, 2, b"en", b"hello"),
            response(FINAL, 1, b"", b"hello"),
            response(FINAL, 1, b"EN", b"hello"),
            response(NO_SPEECH, 1, b"en", b""),
            std::mem::take(&mut invalid_utf8),
            trailing,
        ] {
            assert_eq!(
                decode_response(bytes.as_slice()),
                Err(RecognitionError::InvalidProtocol)
            );
        }
    }

    #[test]
    fn request_has_bounded_versioned_shape() -> Result<(), RecognitionError> {
        let config = RecognitionConfig::automatic()
            .with_language("es")?
            .with_acceleration(RecognitionAcceleration::Cpu);
        let mut bytes = Vec::new();
        encode_request(&mut bytes, &config, &[0.25, -0.5])?;
        assert_eq!(&bytes[..4], b"ZASR");
        assert_eq!(&bytes[4..6], &VERSION.to_le_bytes());
        assert_eq!(&bytes[6..8], &[REQUEST, 1]);
        let sample_count = bytes[8..16]
            .try_into()
            .map_err(|_| RecognitionError::InvalidProtocol)?;
        assert_eq!(u64::from_le_bytes(sample_count), 2);
        assert_eq!(&bytes[18..20], b"es");
        assert_eq!(bytes.len(), 28);
        Ok(())
    }
}
