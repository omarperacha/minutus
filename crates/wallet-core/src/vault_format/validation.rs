use std::{error::Error, fmt};

/// Failure to construct or decode a versioned vault document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultFormatError {
    /// The serialized document exceeds its host-owned byte limit.
    DocumentTooLarge {
        /// Observed serialized byte length.
        actual: usize,
        /// Maximum accepted serialized byte length.
        maximum: usize,
    },
    /// The document is not valid versioned vault JSON.
    MalformedDocument(String),
    /// The document declares a version this build cannot safely interpret.
    UnsupportedVersion {
        /// Version declared by the document.
        found: u32,
        /// Version understood by this build.
        current: u32,
    },
    /// A closed algorithm or derivation parameter does not match the version.
    UnsupportedParameter {
        /// Stable field path containing the unsupported value.
        field: &'static str,
    },
    /// A base64 field is malformed or has a non-canonical representation.
    InvalidEncoding {
        /// Stable field path containing invalid base64.
        field: &'static str,
    },
    /// A fixed-width binary field has the wrong decoded length.
    InvalidLength {
        /// Stable field path containing the invalid value.
        field: &'static str,
        /// Observed decoded byte length.
        actual: usize,
        /// Required decoded byte length.
        expected: usize,
    },
    /// The encrypted payload is empty or exceeds its byte limit.
    InvalidPayloadLength {
        /// Observed encrypted-payload byte length.
        actual: usize,
        /// Maximum accepted encrypted-payload byte length.
        maximum: usize,
    },
    /// Account metadata violates a bounded format invariant.
    InvalidAccountMetadata {
        /// Stable field path that violates the account schema.
        field: &'static str,
    },
    /// Two account records use the same stable account identifier.
    DuplicateAccountId,
}

impl fmt::Display for VaultFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DocumentTooLarge { actual, maximum } => write!(
                formatter,
                "vault document is {actual} bytes; the maximum is {maximum} bytes"
            ),
            Self::MalformedDocument(error) => {
                write!(formatter, "malformed vault document: {error}")
            }
            Self::UnsupportedVersion { found, current } => write!(
                formatter,
                "vault version {found} is unsupported; this build reads version {current}"
            ),
            Self::UnsupportedParameter { field } => {
                write!(formatter, "vault parameter `{field}` is unsupported")
            }
            Self::InvalidEncoding { field } => {
                write!(formatter, "vault field `{field}` is not canonical base64")
            }
            Self::InvalidLength {
                field,
                actual,
                expected,
            } => write!(
                formatter,
                "vault field `{field}` is {actual} bytes; expected {expected} bytes"
            ),
            Self::InvalidPayloadLength { actual, maximum } => write!(
                formatter,
                "encrypted vault payload is {actual} bytes; expected 1 to {maximum} bytes"
            ),
            Self::InvalidAccountMetadata { field } => {
                write!(formatter, "vault account field `{field}` is invalid")
            }
            Self::DuplicateAccountId => {
                formatter.write_str("vault contains a duplicate account identifier")
            }
        }
    }
}

impl Error for VaultFormatError {}
