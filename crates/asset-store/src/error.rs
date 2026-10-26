//! Sanitized asset-ingestion failures.

use std::{error::Error, fmt};

/// Failure while validating, normalizing or managing a local asset.
#[derive(Debug)]
pub enum AssetStoreError {
    /// Input exceeds a fixed media-ingestion limit.
    LimitExceeded,
    /// Input is not a supported static image.
    UnsupportedMedia,
    /// Image bytes or structure are malformed.
    InvalidMedia,
    /// Managed asset storage could not complete an operation.
    Storage(std::io::Error),
    /// An image codec rejected an otherwise bounded operation.
    Codec(image::ImageError),
    /// Persisted metadata contains an invalid Rust-owned storage key.
    InvalidStorageKey,
}

impl fmt::Display for AssetStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded => formatter.write_str("image exceeds a supported limit"),
            Self::UnsupportedMedia => formatter.write_str("image format is not supported"),
            Self::InvalidMedia => formatter.write_str("image is malformed or unsupported"),
            Self::Storage(_) => formatter.write_str("local asset storage failed"),
            Self::Codec(_) => formatter.write_str("image could not be processed safely"),
            Self::InvalidStorageKey => formatter.write_str("stored asset reference is invalid"),
        }
    }
}

impl Error for AssetStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Codec(error) => Some(error),
            Self::LimitExceeded
            | Self::UnsupportedMedia
            | Self::InvalidMedia
            | Self::InvalidStorageKey => None,
        }
    }
}

impl From<std::io::Error> for AssetStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<image::ImageError> for AssetStoreError {
    fn from(error: image::ImageError) -> Self {
        Self::Codec(error)
    }
}
