//! Consuming handoff from validated network responses to managed media ingestion.

use std::fmt;

use crate::{
    ValidatedWebMediaType, WebBrokerResponse, WebMethod, WebResponseKind, WebResponseProvenance,
};

/// Failure to narrow a validated web response into a remote static image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteImageHandoffError {
    /// Only complete successful GET responses may carry an image body.
    IncompleteResponse,
    /// The response is not one of the closed supported static image types.
    UnsupportedMediaType,
}

impl fmt::Display for RemoteImageHandoffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IncompleteResponse => "remote image response was incomplete",
            Self::UnsupportedMediaType => "remote image media type is unsupported",
        })
    }
}

impl std::error::Error for RemoteImageHandoffError {}

/// Host-only image bytes admitted to the existing managed-asset validator.
///
/// This value has no URL-to-inference operation, serialization or debug
/// projection. Consuming a [`WebBrokerResponse`] is the only public constructor.
pub struct RemoteImageResponse {
    provenance: WebResponseProvenance,
    encoded_bytes: Vec<u8>,
}

impl RemoteImageResponse {
    /// Returns trusted retrieval provenance for the accepted image bytes.
    #[must_use]
    pub const fn provenance(&self) -> &WebResponseProvenance {
        &self.provenance
    }

    /// Returns untrusted encoded bytes for full media validation and normalization.
    #[must_use]
    pub fn untrusted_encoded_bytes(&self) -> &[u8] {
        &self.encoded_bytes
    }
}

impl TryFrom<WebBrokerResponse> for RemoteImageResponse {
    type Error = RemoteImageHandoffError;

    fn try_from(response: WebBrokerResponse) -> Result<Self, Self::Error> {
        if response.kind() != WebResponseKind::Completed
            || response.provenance().method() != WebMethod::Get
            || response.provenance().truncated()
            || response.untrusted_body().is_empty()
        {
            return Err(RemoteImageHandoffError::IncompleteResponse);
        }
        let Some(media_type) = response.provenance().media_type() else {
            return Err(RemoteImageHandoffError::UnsupportedMediaType);
        };
        if !matches!(
            media_type,
            ValidatedWebMediaType::Jpeg | ValidatedWebMediaType::Png | ValidatedWebMediaType::Webp
        ) {
            return Err(RemoteImageHandoffError::UnsupportedMediaType);
        }
        let (provenance, body) = response.into_parts();
        Ok(Self {
            provenance,
            encoded_bytes: body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CanonicalWebUrl;

    fn response(
        media_type: ValidatedWebMediaType,
        body: &[u8],
    ) -> Result<WebBrokerResponse, crate::UrlValidationError> {
        Ok(WebBrokerResponse::test_document(
            CanonicalWebUrl::parse("https://images.example.com/diagram.png?token=secret")?,
            media_type,
            body.to_vec(),
        ))
    }

    #[test]
    fn consuming_handoff_admits_only_completed_static_images()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = RemoteImageResponse::try_from(response(
            ValidatedWebMediaType::Png,
            b"\x89PNG\r\n\x1a\nawaiting-full-validation",
        )?)?;
        assert!(image.untrusted_encoded_bytes().starts_with(b"\x89PNG"));
        assert_eq!(
            image.provenance().final_origin(),
            "https://images.example.com"
        );

        assert_eq!(
            RemoteImageResponse::try_from(response(ValidatedWebMediaType::Html, b"<p>x</p>")?)
                .err(),
            Some(RemoteImageHandoffError::UnsupportedMediaType)
        );
        Ok(())
    }
}
