use std::{fmt, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::{
    StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName},
};
use sha2::{Digest, Sha256};

mod body;

pub(crate) use body::BodyAccumulator;
#[cfg(test)]
use body::validate_media_signature;

use crate::{
    CanonicalWebUrl, HEADER_PROFILE_REVISION, REQUEST_CONTRACT_REVISION, RESPONSE_POLICY_REVISION,
    WebMethod,
};

/// Maximum accepted HTTP/1 status-line bytes.
pub const MAX_RESPONSE_STATUS_LINE_BYTES: usize = 1_024;
/// Maximum response header-field count.
pub const MAX_RESPONSE_HEADER_FIELDS: usize = 64;
/// Maximum bytes in one response header name plus value.
pub const MAX_RESPONSE_HEADER_FIELD_BYTES: usize = 8 * 1_024;
/// Maximum aggregate response-header wire bytes.
pub const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1_024;
/// Maximum encoded bytes accepted for one static document.
pub const MAX_ENCODED_BODY_BYTES: usize = 4 * 1_024 * 1_024;
/// Maximum decoded bytes accepted for one static document.
pub const MAX_DECODED_BODY_BYTES: usize = 4 * 1_024 * 1_024;
/// Maximum encoded bytes accepted for one remote static image.
pub const MAX_REMOTE_ASSET_ENCODED_BODY_BYTES: usize = 20 * 1_024 * 1_024;
/// Maximum decoded transport bytes accepted for one remote static image.
///
/// Identity is the only accepted content encoding, so this is deliberately
/// equal to the encoded ceiling while both counters remain independent.
pub const MAX_REMOTE_ASSET_DECODED_BODY_BYTES: usize = 20 * 1_024 * 1_024;
/// Maximum decoded bytes retained while recognizing one x402 challenge.
pub const MAX_PAYMENT_REQUIRED_CHALLENGE_BYTES: usize = 8 * 1_024;
/// Maximum requirements counted in one generic retrieval challenge.
pub const MAX_PAYMENT_REQUIREMENTS: usize = 8;
/// Maximum untrusted payment error text projected by generic retrieval.
pub const MAX_PAYMENT_ERROR_BYTES: usize = 512;
pub(crate) const PAYMENT_REQUIRED_HEADER: HeaderName = HeaderName::from_static("payment-required");

/// Bounded non-authoritative recognition of one x402 payment challenge.
///
/// Generic retrieval retains no payment target, signing payload or continuation
/// method. This type has no serialization or debug projection.
pub struct WebPaymentRequiredChallenge {
    version: u64,
    requirement_count: usize,
    challenge_sha256: String,
    error: Option<String>,
}

impl WebPaymentRequiredChallenge {
    /// Returns the declared x402 protocol version.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Returns the bounded number of advertised payment requirements.
    #[must_use]
    pub const fn requirement_count(&self) -> usize {
        self.requirement_count
    }

    /// Returns the digest of the decoded challenge, not its bytes.
    #[must_use]
    pub fn challenge_sha256(&self) -> &str {
        &self.challenge_sha256
    }

    /// Returns optional bounded remote error prose as explicitly untrusted data.
    #[must_use]
    pub fn untrusted_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Closed media types admitted to later bounded extractors or asset validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidatedWebMediaType {
    /// Static HTML document.
    Html,
    /// Static JSON document.
    Json,
    /// Static plain-text document.
    PlainText,
    /// Static Markdown retained as untrusted text, never rendered or executed.
    Markdown,
    /// JPEG image awaiting WEB-006 asset validation.
    Jpeg,
    /// PNG image awaiting WEB-006 asset validation.
    Png,
    /// WebP image awaiting WEB-006 asset validation.
    Webp,
}

/// Host-derived terminal classification for one HTTP response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebResponseKind {
    /// A complete successful HTTP response.
    Completed,
    /// A redirect was observed and deliberately not followed.
    RedirectBlocked,
    /// HTTP 402 was observed without retaining its untrusted challenge body.
    PaymentRequired,
    /// Another non-success HTTP status was observed with its body discarded.
    HttpFailure,
}

impl WebResponseKind {
    pub(crate) const fn from_status(status: u16) -> Self {
        if status == 402 {
            Self::PaymentRequired
        } else if status / 100 == 3 {
            Self::RedirectBlocked
        } else if status / 100 == 2 {
            Self::Completed
        } else {
            Self::HttpFailure
        }
    }
}

impl ValidatedWebMediaType {
    /// Returns the canonical media-type spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Html => "text/html",
            Self::Json => "application/json",
            Self::PlainText => "text/plain",
            Self::Markdown => "text/markdown",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }

    const fn is_textual(self) -> bool {
        matches!(
            self,
            Self::Html | Self::Json | Self::PlainText | Self::Markdown
        )
    }

    pub(crate) const fn is_image(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::Webp)
    }
}

/// Trusted provenance for one bounded remote observation.
///
/// This value deliberately has no serialization or debug projection because
/// its complete canonical URL may contain sensitive query values.
pub struct WebResponseProvenance {
    canonical_url: CanonicalWebUrl,
    final_origin: String,
    method: WebMethod,
    status: u16,
    media_type: Option<ValidatedWebMediaType>,
    byte_count: usize,
    content_sha256: String,
    retrieved_at_unix_millis: u64,
    elapsed_millis: u64,
    truncated: bool,
}

impl WebResponseProvenance {
    pub(crate) fn new(
        canonical_url: CanonicalWebUrl,
        method: WebMethod,
        status: u16,
        media_type: Option<ValidatedWebMediaType>,
        body: &[u8],
        retrieved_at_unix_millis: u64,
        elapsed: Duration,
    ) -> Self {
        let final_origin = canonical_url.origin().to_owned();
        Self {
            canonical_url,
            final_origin,
            method,
            status,
            media_type,
            byte_count: body.len(),
            content_sha256: format!("{:x}", Sha256::digest(body)),
            retrieved_at_unix_millis,
            elapsed_millis: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            truncated: false,
        }
    }

    /// Returns the exact canonical source URL.
    #[must_use]
    pub const fn canonical_url(&self) -> &CanonicalWebUrl {
        &self.canonical_url
    }

    /// Returns the final origin, equal to the initial origin while redirects are disabled.
    #[must_use]
    pub fn final_origin(&self) -> &str {
        &self.final_origin
    }

    /// Returns the exact request method.
    #[must_use]
    pub const fn method(&self) -> WebMethod {
        self.method
    }

    /// Returns the numeric response status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns the validated representation type, when a body was eligible.
    #[must_use]
    pub const fn media_type(&self) -> Option<ValidatedWebMediaType> {
        self.media_type
    }

    /// Returns the exact accepted body-byte count.
    #[must_use]
    pub const fn byte_count(&self) -> usize {
        self.byte_count
    }

    /// Returns the lowercase SHA-256 digest of accepted bytes.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    /// Returns the host retrieval timestamp as Unix milliseconds.
    #[must_use]
    pub const fn retrieved_at_unix_millis(&self) -> u64 {
        self.retrieved_at_unix_millis
    }

    /// Returns bounded end-to-end elapsed milliseconds.
    #[must_use]
    pub const fn elapsed_millis(&self) -> u64 {
        self.elapsed_millis
    }

    /// Returns whether accepted content was intentionally truncated.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Returns the canonical request-contract revision.
    #[must_use]
    pub const fn request_contract_revision(&self) -> &'static str {
        REQUEST_CONTRACT_REVISION
    }

    /// Returns the fixed outbound-header revision.
    #[must_use]
    pub const fn header_profile_revision(&self) -> &'static str {
        HEADER_PROFILE_REVISION
    }

    /// Returns the bounded response-policy revision.
    #[must_use]
    pub const fn response_policy_revision(&self) -> &'static str {
        RESPONSE_POLICY_REVISION
    }
}

/// Sanitized response-validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseValidationError {
    /// The HTTP/1 status line exceeded its fixed ceiling.
    StatusLineLimit,
    /// Too many response header fields were received.
    HeaderCountLimit,
    /// One response header exceeded its fixed ceiling.
    HeaderFieldLimit,
    /// Aggregate response headers exceeded their fixed ceiling.
    HeaderAggregateLimit,
    /// Content length was invalid or internally conflicting.
    InvalidContentLength,
    /// The declared or observed encoded body exceeded its ceiling.
    EncodedBodyLimit,
    /// The observed decoded body exceeded its ceiling.
    DecodedBodyLimit,
    /// A declared length did not match the complete observed representation.
    IncompleteBody,
    /// The peer selected a non-identity content encoding.
    UnsupportedEncoding,
    /// The response lacks one unambiguous allow-listed media type.
    UnsupportedMediaType,
    /// Declared media type and observed bytes conflict.
    MediaTypeMismatch,
    /// A partial representation was returned despite the absence of a range request.
    UnexpectedPartialResponse,
}

impl fmt::Display for ResponseValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::StatusLineLimit => "web response status line exceeded its limit",
            Self::HeaderCountLimit => "web response had too many headers",
            Self::HeaderFieldLimit => "web response header exceeded its limit",
            Self::HeaderAggregateLimit => "web response headers exceeded their limit",
            Self::InvalidContentLength => "web response length metadata was invalid",
            Self::EncodedBodyLimit => "web response encoded body exceeded its limit",
            Self::DecodedBodyLimit => "web response decoded body exceeded its limit",
            Self::IncompleteBody => "web response body was incomplete",
            Self::UnsupportedEncoding => "web response encoding is unsupported",
            Self::UnsupportedMediaType => "web response media type is unsupported",
            Self::MediaTypeMismatch => "web response media type did not match its bytes",
            Self::UnexpectedPartialResponse => "web response was unexpectedly partial",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ResponseValidationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BodyDisposition {
    Accept(ValidatedWebMediaType),
    Discard(Option<ValidatedWebMediaType>),
}

pub(crate) struct ValidatedResponseMetadata {
    pub(crate) status: u16,
    pub(crate) disposition: BodyDisposition,
    pub(crate) declared_length: Option<usize>,
    pub(crate) max_body_bytes: usize,
    pub(crate) payment_required: Option<WebPaymentRequiredChallenge>,
}

pub(crate) fn validate_response_metadata(
    method: WebMethod,
    status: StatusCode,
    status_line_bytes: usize,
    headers: &HeaderMap,
    max_document_body_bytes: usize,
    max_remote_asset_body_bytes: usize,
) -> Result<ValidatedResponseMetadata, ResponseValidationError> {
    validate_status_and_headers(status_line_bytes, headers)?;
    validate_identity_encoding(headers)?;
    let declared_length = parse_content_length(headers)?;
    let status = status.as_u16();
    let payment_required = parse_payment_required(status, headers);

    if status == StatusCode::PARTIAL_CONTENT.as_u16() {
        return Err(ResponseValidationError::UnexpectedPartialResponse);
    }
    let representation_is_successful = status / 100 == 2 && status != 204;
    if !representation_is_successful {
        return Ok(ValidatedResponseMetadata {
            status,
            disposition: BodyDisposition::Discard(None),
            declared_length: None,
            max_body_bytes: 0,
            payment_required,
        });
    }

    if method == WebMethod::Head {
        return Ok(ValidatedResponseMetadata {
            status,
            disposition: BodyDisposition::Discard(optional_media_type(headers)?),
            declared_length: None,
            max_body_bytes: 0,
            payment_required,
        });
    }
    let media_type = parse_media_type(headers)?;
    let max_body_bytes = if media_type.is_image() {
        max_remote_asset_body_bytes
    } else {
        max_document_body_bytes
    };
    if declared_length.is_some_and(|length| length > max_body_bytes) {
        return Err(ResponseValidationError::EncodedBodyLimit);
    }
    Ok(ValidatedResponseMetadata {
        status,
        disposition: BodyDisposition::Accept(media_type),
        declared_length,
        max_body_bytes,
        payment_required,
    })
}

fn parse_payment_required(status: u16, headers: &HeaderMap) -> Option<WebPaymentRequiredChallenge> {
    if status != StatusCode::PAYMENT_REQUIRED.as_u16() {
        return None;
    }
    let mut values = headers.get_all(&PAYMENT_REQUIRED_HEADER).iter();
    let encoded = values.next()?.as_bytes();
    if values.next().is_some() || encoded.len() > MAX_PAYMENT_REQUIRED_CHALLENGE_BYTES {
        return None;
    }
    let decoded = BASE64_STANDARD.decode(encoded).ok()?;
    if decoded.is_empty() || decoded.len() > MAX_PAYMENT_REQUIRED_CHALLENGE_BYTES {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(&decoded).ok()?;
    let object = value.as_object()?;
    let version = object.get("x402Version")?.as_u64()?;
    let requirements = object.get("accepts")?.as_array()?;
    if version != 2 || requirements.is_empty() || requirements.len() > MAX_PAYMENT_REQUIREMENTS {
        return None;
    }
    let error = object
        .get("error")
        .and_then(serde_json::Value::as_str)
        .filter(|error| {
            !error.is_empty()
                && error.len() <= MAX_PAYMENT_ERROR_BYTES
                && !error.chars().any(char::is_control)
        })
        .map(str::to_owned);
    Some(WebPaymentRequiredChallenge {
        version,
        requirement_count: requirements.len(),
        challenge_sha256: format!("{:x}", Sha256::digest(&decoded)),
        error,
    })
}

fn validate_status_and_headers(
    status_line_bytes: usize,
    headers: &HeaderMap,
) -> Result<(), ResponseValidationError> {
    if status_line_bytes > MAX_RESPONSE_STATUS_LINE_BYTES {
        return Err(ResponseValidationError::StatusLineLimit);
    }
    if headers.len() > MAX_RESPONSE_HEADER_FIELDS {
        return Err(ResponseValidationError::HeaderCountLimit);
    }
    let mut aggregate = 2usize;
    for (name, value) in headers {
        let field_bytes = name
            .as_str()
            .len()
            .checked_add(value.as_bytes().len())
            .ok_or(ResponseValidationError::HeaderFieldLimit)?;
        if field_bytes > MAX_RESPONSE_HEADER_FIELD_BYTES {
            return Err(ResponseValidationError::HeaderFieldLimit);
        }
        aggregate = aggregate
            .checked_add(field_bytes)
            .and_then(|size| size.checked_add(4))
            .ok_or(ResponseValidationError::HeaderAggregateLimit)?;
        if aggregate > MAX_RESPONSE_HEADER_BYTES {
            return Err(ResponseValidationError::HeaderAggregateLimit);
        }
    }
    Ok(())
}

fn validate_identity_encoding(headers: &HeaderMap) -> Result<(), ResponseValidationError> {
    if headers
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .all(|value| value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        Ok(())
    } else {
        Err(ResponseValidationError::UnsupportedEncoding)
    }
}

fn parse_content_length(headers: &HeaderMap) -> Result<Option<usize>, ResponseValidationError> {
    let mut parsed = None;
    for value in headers.get_all(CONTENT_LENGTH) {
        let value = value
            .to_str()
            .map_err(|_| ResponseValidationError::InvalidContentLength)?;
        for component in value.split(',') {
            let component = component.trim();
            if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ResponseValidationError::InvalidContentLength);
            }
            let length = component
                .parse::<u64>()
                .ok()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(ResponseValidationError::InvalidContentLength)?;
            if parsed.replace(length).is_some_and(|prior| prior != length) {
                return Err(ResponseValidationError::InvalidContentLength);
            }
        }
    }
    Ok(parsed)
}

fn parse_media_type(headers: &HeaderMap) -> Result<ValidatedWebMediaType, ResponseValidationError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let value = values
        .next()
        .ok_or(ResponseValidationError::UnsupportedMediaType)?;
    if values.next().is_some() {
        return Err(ResponseValidationError::UnsupportedMediaType);
    }
    let value = value
        .to_str()
        .map_err(|_| ResponseValidationError::UnsupportedMediaType)?;
    let mut components = value.split(';');
    let essence = components
        .next()
        .map(str::trim)
        .ok_or(ResponseValidationError::UnsupportedMediaType)?;
    let media_type = if essence.eq_ignore_ascii_case("text/html") {
        ValidatedWebMediaType::Html
    } else if essence.eq_ignore_ascii_case("application/json") {
        ValidatedWebMediaType::Json
    } else if essence.eq_ignore_ascii_case("text/plain") {
        ValidatedWebMediaType::PlainText
    } else if essence.eq_ignore_ascii_case("text/markdown") {
        ValidatedWebMediaType::Markdown
    } else if essence.eq_ignore_ascii_case("image/jpeg") {
        ValidatedWebMediaType::Jpeg
    } else if essence.eq_ignore_ascii_case("image/png") {
        ValidatedWebMediaType::Png
    } else if essence.eq_ignore_ascii_case("image/webp") {
        ValidatedWebMediaType::Webp
    } else {
        return Err(ResponseValidationError::UnsupportedMediaType);
    };
    validate_media_parameters(media_type, components)?;
    Ok(media_type)
}

fn optional_media_type(
    headers: &HeaderMap,
) -> Result<Option<ValidatedWebMediaType>, ResponseValidationError> {
    if headers.get_all(CONTENT_TYPE).iter().next().is_none() {
        Ok(None)
    } else {
        parse_media_type(headers).map(Some)
    }
}

fn validate_media_parameters<'a>(
    media_type: ValidatedWebMediaType,
    components: impl Iterator<Item = &'a str>,
) -> Result<(), ResponseValidationError> {
    let mut charset = None;
    for component in components {
        let (name, value) = component
            .split_once('=')
            .ok_or(ResponseValidationError::UnsupportedMediaType)?;
        if !media_type.is_textual() || !name.trim().eq_ignore_ascii_case("charset") {
            return Err(ResponseValidationError::UnsupportedMediaType);
        }
        let raw_value = value.trim();
        let value = if raw_value.starts_with('"') || raw_value.ends_with('"') {
            raw_value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .filter(|value| !value.contains('"'))
                .ok_or(ResponseValidationError::UnsupportedMediaType)?
        } else {
            raw_value
        };
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || charset.replace(value).is_some()
            || !matches!(value.to_ascii_lowercase().as_str(), "utf-8" | "utf8")
        {
            return Err(ResponseValidationError::UnsupportedMediaType);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
