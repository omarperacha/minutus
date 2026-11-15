//! Model-authored presentation text never enters workflow or signing authority.
use serde::{Deserialize, Deserializer, de::Error as _};

pub(super) struct ModelSummary(String);

impl ModelSummary {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ModelSummary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::evm_approval::presentation::validate_model_summary(&text)
            .map_err(|_| D::Error::custom("Expected a bounded plain model explanation"))?;
        Ok(Self(text))
    }
}
