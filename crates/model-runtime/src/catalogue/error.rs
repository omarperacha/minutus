use super::{ArtifactKey, Error, ModelKey, fmt};

/// Failure while parsing or validating model catalogue data.
#[derive(Debug)]
pub enum CatalogueError {
    /// The JSON document exceeds the Rust-owned input limit.
    TooLarge,
    /// The document is not valid strict catalogue JSON.
    Json(serde_json::Error),
    /// The catalogue uses a schema this build does not understand.
    UnsupportedSchema {
        /// Schema found in the document.
        found: u32,
        /// Schema supported by this build.
        supported: u32,
    },
    /// The catalogue contains no model entries.
    NoModels,
    /// The catalogue exceeds the model-count limit.
    TooManyModels,
    /// One model field violated a semantic invariant.
    InvalidField {
        /// Zero-based entry position.
        model_index: usize,
        /// Stable field path.
        field: &'static str,
        /// Stable validation reason.
        reason: &'static str,
    },
    /// The same model release appears more than once.
    DuplicateModel(ModelKey),
    /// The same artifact identity appears more than once.
    DuplicateArtifact(ArtifactKey),
}

impl fmt::Display for CatalogueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("model catalogue exceeds its size limit"),
            Self::Json(error) => write!(formatter, "invalid model catalogue JSON: {error}"),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "unsupported model catalogue schema {found}; this build supports {supported}"
            ),
            Self::NoModels => formatter.write_str("model catalogue contains no models"),
            Self::TooManyModels => formatter.write_str("model catalogue contains too many models"),
            Self::InvalidField {
                model_index,
                field,
                reason,
            } => write!(
                formatter,
                "invalid model catalogue entry {model_index} field {field}: {reason}"
            ),
            Self::DuplicateModel(key) => write!(formatter, "duplicate model release {key}"),
            Self::DuplicateArtifact(key) => write!(formatter, "duplicate model artifact {key}"),
        }
    }
}

impl Error for CatalogueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}
