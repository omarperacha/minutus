use reqwest::{
    StatusCode,
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};

use super::*;

fn text_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers
}

#[test]
fn status_and_header_limits_are_exact_and_overflow_safe() -> Result<(), Box<dyn std::error::Error>>
{
    let headers = text_headers();
    assert!(validate_status_and_headers(MAX_RESPONSE_STATUS_LINE_BYTES, &headers).is_ok());
    assert_eq!(
        validate_status_and_headers(MAX_RESPONSE_STATUS_LINE_BYTES + 1, &headers).err(),
        Some(ResponseValidationError::StatusLineLimit)
    );

    let mut too_many = HeaderMap::new();
    for index in 0..=MAX_RESPONSE_HEADER_FIELDS {
        let name = HeaderName::from_bytes(format!("x-{index}").as_bytes())?;
        too_many.insert(name, HeaderValue::from_static("a"));
    }
    assert_eq!(
        validate_status_and_headers(16, &too_many).err(),
        Some(ResponseValidationError::HeaderCountLimit)
    );

    let mut large_field = HeaderMap::new();
    large_field.insert("x-large", HeaderValue::from_bytes(&vec![b'a'; 8_185])?);
    assert!(validate_status_and_headers(16, &large_field).is_ok());
    large_field.insert("x-large", HeaderValue::from_bytes(&vec![b'a'; 8_186])?);
    assert_eq!(
        validate_status_and_headers(16, &large_field).err(),
        Some(ResponseValidationError::HeaderFieldLimit)
    );

    let mut aggregate = HeaderMap::new();
    for index in 0..5 {
        let name = HeaderName::from_bytes(format!("x-aggregate-{index}").as_bytes())?;
        aggregate.insert(name, HeaderValue::from_bytes(&vec![b'a'; 7_000])?);
    }
    assert_eq!(
        validate_status_and_headers(16, &aggregate).err(),
        Some(ResponseValidationError::HeaderAggregateLimit)
    );
    Ok(())
}

#[test]
fn content_length_cannot_raise_or_misstate_observed_limits()
-> Result<(), Box<dyn std::error::Error>> {
    let mut headers = text_headers();
    headers.append(CONTENT_LENGTH, HeaderValue::from_static("3"));
    headers.append(CONTENT_LENGTH, HeaderValue::from_static("4"));
    assert_eq!(
        parse_content_length(&headers).err(),
        Some(ResponseValidationError::InvalidContentLength)
    );

    let mut oversized = text_headers();
    oversized.insert(CONTENT_LENGTH, HeaderValue::from_static("11"));
    assert_eq!(
        validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &oversized, 10, 10).err(),
        Some(ResponseValidationError::EncodedBodyLimit)
    );

    for declared in ["2", "4"] {
        let mut accumulator = BodyAccumulator::new(
            ValidatedWebMediaType::PlainText,
            Some(declared.parse()?),
            10,
            10,
        );
        accumulator.push_identity_chunk(b"abc")?;
        assert_eq!(
            accumulator.finish().err(),
            Some(ResponseValidationError::IncompleteBody)
        );
    }
    Ok(())
}

#[test]
fn encoded_and_decoded_stream_ceilings_are_independent() -> Result<(), Box<dyn std::error::Error>> {
    let mut encoded = BodyAccumulator::new(ValidatedWebMediaType::PlainText, None, 3, 4);
    encoded.push_identity_chunk(b"abc")?;
    assert_eq!(
        encoded.push_identity_chunk(b"d").err(),
        Some(ResponseValidationError::EncodedBodyLimit)
    );

    let mut decoded = BodyAccumulator::new(ValidatedWebMediaType::PlainText, None, 4, 3);
    assert_eq!(
        decoded.push_identity_chunk(b"abcd").err(),
        Some(ResponseValidationError::DecodedBodyLimit)
    );
    Ok(())
}

#[test]
fn remote_image_profile_cannot_raise_static_document_limits()
-> Result<(), Box<dyn std::error::Error>> {
    let over_document = MAX_ENCODED_BODY_BYTES + 1;
    let mut text = text_headers();
    text.insert(CONTENT_LENGTH, HeaderValue::from(over_document));
    assert_eq!(
        validate_response_metadata(
            WebMethod::Get,
            StatusCode::OK,
            16,
            &text,
            MAX_ENCODED_BODY_BYTES,
            MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
        )
        .err(),
        Some(ResponseValidationError::EncodedBodyLimit)
    );

    let mut image = HeaderMap::new();
    image.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
    image.insert(CONTENT_LENGTH, HeaderValue::from(over_document));
    let admitted = validate_response_metadata(
        WebMethod::Get,
        StatusCode::OK,
        16,
        &image,
        MAX_ENCODED_BODY_BYTES,
        MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    )?;
    assert_eq!(admitted.max_body_bytes, MAX_REMOTE_ASSET_ENCODED_BODY_BYTES);

    image.insert(
        CONTENT_LENGTH,
        HeaderValue::from(MAX_REMOTE_ASSET_ENCODED_BODY_BYTES + 1),
    );
    assert_eq!(
        validate_response_metadata(
            WebMethod::Get,
            StatusCode::OK,
            16,
            &image,
            MAX_ENCODED_BODY_BYTES,
            MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
        )
        .err(),
        Some(ResponseValidationError::EncodedBodyLimit)
    );
    Ok(())
}

#[test]
fn encoding_media_and_partial_response_policy_fail_closed() {
    let mut encoded = text_headers();
    encoded.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    assert_eq!(
        validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &encoded, 1024, 1024).err(),
        Some(ResponseValidationError::UnsupportedEncoding)
    );

    let mut unsupported = HeaderMap::new();
    unsupported.insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    assert_eq!(
        validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &unsupported, 1024, 1024,)
            .err(),
        Some(ResponseValidationError::UnsupportedMediaType)
    );
    assert_eq!(
        validate_response_metadata(
            WebMethod::Get,
            StatusCode::PARTIAL_CONTENT,
            16,
            &text_headers(),
            1024,
            1024,
        )
        .err(),
        Some(ResponseValidationError::UnexpectedPartialResponse)
    );

    let mut utf8 = HeaderMap::new();
    utf8.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=\"UTF-8\""),
    );
    assert!(
        validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &utf8, 1024, 1024).is_ok()
    );
    for declaration in [
        "text/html; charset=windows-1252",
        "text/html; charset=\"utf-8",
        "text/html; charset=utf-8\"",
    ] {
        let mut unsupported_charset = HeaderMap::new();
        unsupported_charset.insert(
            CONTENT_TYPE,
            HeaderValue::try_from(declaration)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
        );
        assert_eq!(
            validate_response_metadata(
                WebMethod::Get,
                StatusCode::OK,
                16,
                &unsupported_charset,
                1024,
                1024,
            )
            .err(),
            Some(ResponseValidationError::UnsupportedMediaType)
        );
    }
}

#[test]
fn declared_type_can_only_be_narrowed_or_rejected_by_bytes() {
    for (media_type, body) in [
        (
            ValidatedWebMediaType::Html,
            b" \r\nPK\x03\x04archive".as_slice(),
        ),
        (
            ValidatedWebMediaType::Html,
            b"\xef\xbb\xbf<SvG></sVg>".as_slice(),
        ),
        (ValidatedWebMediaType::Json, b"<html>not json".as_slice()),
        (ValidatedWebMediaType::PlainText, b"MZexecutable".as_slice()),
        (ValidatedWebMediaType::Png, b"GIF89a".as_slice()),
    ] {
        assert_eq!(
            validate_media_signature(media_type, body).err(),
            Some(ResponseValidationError::MediaTypeMismatch)
        );
    }
    assert!(validate_media_signature(ValidatedWebMediaType::Html, b"<!doctype html>").is_ok());
    assert!(validate_media_signature(ValidatedWebMediaType::Json, b" {\"ok\":true}").is_ok());
    assert!(validate_media_signature(ValidatedWebMediaType::Png, b"\x89PNG\r\n\x1a\nrest").is_ok());
}

#[test]
fn bodyless_statuses_discard_remote_content_and_headers_after_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let head = validate_response_metadata(
        WebMethod::Head,
        StatusCode::OK,
        16,
        &text_headers(),
        MAX_ENCODED_BODY_BYTES,
        MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    )?;
    assert_eq!(
        head.disposition,
        BodyDisposition::Discard(Some(ValidatedWebMediaType::PlainText))
    );

    for (method, status) in [
        (WebMethod::Head, StatusCode::OK),
        (WebMethod::Get, StatusCode::FOUND),
        (WebMethod::Get, StatusCode::PAYMENT_REQUIRED),
        (WebMethod::Get, StatusCode::INTERNAL_SERVER_ERROR),
        (WebMethod::Get, StatusCode::NO_CONTENT),
    ] {
        let metadata = validate_response_metadata(
            method,
            status,
            16,
            &HeaderMap::new(),
            MAX_ENCODED_BODY_BYTES,
            MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
        )?;
        assert_eq!(metadata.disposition, BodyDisposition::Discard(None));
        assert_eq!(metadata.declared_length, None);
    }
    Ok(())
}

#[test]
fn x402_is_projected_as_bounded_untrusted_diagnostic_only() -> Result<(), Box<dyn std::error::Error>>
{
    let decoded = serde_json::to_vec(&serde_json::json!({
        "x402Version": 2,
        "error": "Payment is required by the remote server",
        "accepts": [{"scheme": "exact", "network": "example", "payTo": "secret"}]
    }))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        PAYMENT_REQUIRED_HEADER,
        HeaderValue::from_str(&BASE64_STANDARD.encode(&decoded))?,
    );
    let metadata = validate_response_metadata(
        WebMethod::Get,
        StatusCode::PAYMENT_REQUIRED,
        16,
        &headers,
        MAX_ENCODED_BODY_BYTES,
        MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    )?;
    let challenge = metadata.payment_required.ok_or("missing challenge")?;
    assert_eq!(challenge.version(), 2);
    assert_eq!(challenge.requirement_count(), 1);
    assert_eq!(
        challenge.challenge_sha256(),
        format!("{:x}", Sha256::digest(decoded))
    );
    assert_eq!(
        challenge.untrusted_error(),
        Some("Payment is required by the remote server")
    );
    assert_eq!(metadata.disposition, BodyDisposition::Discard(None));

    headers.append(
        PAYMENT_REQUIRED_HEADER,
        HeaderValue::from_static("duplicate"),
    );
    let duplicate = validate_response_metadata(
        WebMethod::Get,
        StatusCode::PAYMENT_REQUIRED,
        16,
        &headers,
        MAX_ENCODED_BODY_BYTES,
        MAX_REMOTE_ASSET_ENCODED_BODY_BYTES,
    )?;
    assert!(duplicate.payment_required.is_none());
    Ok(())
}

#[test]
fn markdown_retains_textual_limits_and_rejects_binary_or_non_utf8_declarations()
-> Result<(), Box<dyn std::error::Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    let metadata = validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &headers, 4, 4)?;
    assert!(matches!(
        metadata.disposition,
        BodyDisposition::Accept(ValidatedWebMediaType::Markdown)
    ));
    let mut accumulator = BodyAccumulator::new(ValidatedWebMediaType::Markdown, None, 4, 4);
    accumulator.push_identity_chunk(b"# hi")?;
    assert!(accumulator.push_identity_chunk(b"!").is_err());
    assert!(validate_media_signature(ValidatedWebMediaType::Markdown, b"MZexecutable").is_err());
    assert!(
        validate_media_signature(
            ValidatedWebMediaType::Markdown,
            b"# Guide\n<script>untrusted text</script>"
        )
        .is_ok()
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=windows-1252"),
    );
    assert!(
        validate_response_metadata(WebMethod::Get, StatusCode::OK, 16, &headers, 4, 4).is_err()
    );
    Ok(())
}
