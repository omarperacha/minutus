//! Controlled response fixtures for downstream adversarial tests.
//!
//! This module is absent from production builds. It enters below DNS and socket
//! admission and reuses the production metadata, body, media and size policy.

use std::time::Duration;

use reqwest::{StatusCode, header::HeaderMap};

use crate::{
    CanonicalWebUrl, MAX_DECODED_BODY_BYTES, MAX_ENCODED_BODY_BYTES,
    MAX_REMOTE_ASSET_DECODED_BODY_BYTES, MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    ResponseValidationError, WebBrokerResponse, WebMethod, WebResponseProvenance,
    response::{
        BodyAccumulator, BodyDisposition, ValidatedResponseMetadata, validate_response_metadata,
    },
};

/// Builds one controlled response through the production response policy.
///
/// No listener, DNS resolution or production target exception is created.
///
/// # Errors
///
/// Returns the same bounded metadata, framing, media or body failure as the
/// production response path.
pub fn validated_response(
    method: WebMethod,
    canonical_url: CanonicalWebUrl,
    status: StatusCode,
    status_line_bytes: usize,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<WebBrokerResponse, ResponseValidationError> {
    let ValidatedResponseMetadata {
        status,
        disposition,
        declared_length,
        max_body_bytes,
        payment_required,
    } = validate_response_metadata(
        method,
        status,
        status_line_bytes,
        headers,
        MAX_ENCODED_BODY_BYTES,
        MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    )?;
    let (media_type, body) = match disposition {
        BodyDisposition::Discard(media_type) => (media_type, Vec::new()),
        BodyDisposition::Accept(media_type) => {
            let mut accumulator = BodyAccumulator::new(
                media_type,
                declared_length,
                max_body_bytes,
                if media_type.is_image() {
                    MAX_REMOTE_ASSET_DECODED_BODY_BYTES
                } else {
                    MAX_DECODED_BODY_BYTES
                },
            );
            accumulator.push_identity_chunk(body)?;
            (Some(media_type), accumulator.finish()?)
        }
    };
    let provenance = WebResponseProvenance::new(
        canonical_url,
        method,
        status,
        media_type,
        &body,
        1,
        Duration::ZERO,
    );
    Ok(WebBrokerResponse::from_validated_parts(
        provenance,
        body,
        payment_required,
    ))
}
