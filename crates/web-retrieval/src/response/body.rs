use super::{ResponseValidationError, ValidatedWebMediaType};

pub(crate) struct BodyAccumulator {
    media_type: ValidatedWebMediaType,
    declared_length: Option<usize>,
    encoded_bytes: usize,
    decoded_bytes: usize,
    max_encoded_bytes: usize,
    max_decoded_bytes: usize,
    body: Vec<u8>,
}

impl BodyAccumulator {
    pub(crate) fn new(
        media_type: ValidatedWebMediaType,
        declared_length: Option<usize>,
        max_encoded_bytes: usize,
        max_decoded_bytes: usize,
    ) -> Self {
        Self {
            media_type,
            declared_length,
            encoded_bytes: 0,
            decoded_bytes: 0,
            max_encoded_bytes,
            max_decoded_bytes,
            body: Vec::new(),
        }
    }

    pub(crate) fn push_identity_chunk(
        &mut self,
        chunk: &[u8],
    ) -> Result<(), ResponseValidationError> {
        self.encoded_bytes = checked_body_size(
            self.encoded_bytes,
            chunk.len(),
            self.max_encoded_bytes,
            ResponseValidationError::EncodedBodyLimit,
        )?;
        self.decoded_bytes = checked_body_size(
            self.decoded_bytes,
            chunk.len(),
            self.max_decoded_bytes,
            ResponseValidationError::DecodedBodyLimit,
        )?;
        self.body.extend_from_slice(chunk);
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<Vec<u8>, ResponseValidationError> {
        if self
            .declared_length
            .is_some_and(|declared| declared != self.encoded_bytes)
        {
            return Err(ResponseValidationError::IncompleteBody);
        }
        validate_media_signature(self.media_type, &self.body)?;
        Ok(self.body)
    }
}

fn checked_body_size(
    current: usize,
    additional: usize,
    maximum: usize,
    error: ResponseValidationError,
) -> Result<usize, ResponseValidationError> {
    let next = current.checked_add(additional).ok_or(error)?;
    if next > maximum {
        return Err(error);
    }
    Ok(next)
}

pub(super) fn validate_media_signature(
    media_type: ValidatedWebMediaType,
    body: &[u8],
) -> Result<(), ResponseValidationError> {
    let matches = match media_type {
        ValidatedWebMediaType::Jpeg => body.starts_with(&[0xff, 0xd8, 0xff]),
        ValidatedWebMediaType::Png => body.starts_with(b"\x89PNG\r\n\x1a\n"),
        ValidatedWebMediaType::Webp => {
            body.len() >= 12 && body.starts_with(b"RIFF") && &body[8..12] == b"WEBP"
        }
        ValidatedWebMediaType::Html => {
            !looks_binary_or_active(body)
                && !trim_text_prefix(body).starts_with_ignore_ascii_case(b"<svg")
        }
        ValidatedWebMediaType::Json => {
            let first = trim_text_prefix(body).first().copied();
            !looks_binary_or_active(body) && matches!(first, Some(b'{' | b'['))
        }
        ValidatedWebMediaType::PlainText | ValidatedWebMediaType::Markdown => {
            !looks_binary_or_active(body)
        }
    };
    if matches {
        Ok(())
    } else {
        Err(ResponseValidationError::MediaTypeMismatch)
    }
}

fn looks_binary_or_active(body: &[u8]) -> bool {
    let candidate = trim_text_prefix(body);
    body.contains(&0)
        || candidate.starts_with(b"MZ")
        || candidate.starts_with(b"\x7fELF")
        || candidate.starts_with(b"PK\x03\x04")
        || candidate.starts_with(b"\x1f\x8b")
        || candidate.starts_with(b"%PDF-")
        || candidate.starts_with(b"\0asm")
        || candidate.starts_with(b"Rar!\x1a\x07")
        || candidate.starts_with(b"7z\xbc\xaf\x27\x1c")
        || candidate.starts_with(&[0xfe, 0xed, 0xfa, 0xce])
        || candidate.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || candidate.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || candidate.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    bytes
}

fn trim_text_prefix(bytes: &[u8]) -> &[u8] {
    let bytes = trim_ascii(bytes);
    trim_ascii(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
}

trait StartsWithIgnoreAsciiCase {
    fn starts_with_ignore_ascii_case(&self, prefix: &[u8]) -> bool;
}

impl StartsWithIgnoreAsciiCase for [u8] {
    fn starts_with_ignore_ascii_case(&self, prefix: &[u8]) -> bool {
        self.get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    }
}
