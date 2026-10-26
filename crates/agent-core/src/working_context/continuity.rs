//! Non-authoritative boundary for a conversation's retained dialogue.

use serde::{Deserialize, Serialize};

use super::WorkingContextError;

/// Persisted context selection identity, containing no execution authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DialogueContinuityCheckpoint {
    version: u16,
    conversation_id: String,
    project_id: String,
    retained_start: u32,
    retained_message_id: String,
    history_sha256: String,
}

impl DialogueContinuityCheckpoint {
    /// Binds the retained start to the digest of history through that message.
    ///
    /// # Errors
    /// Returns an error for invalid scope, message identity or digest.
    pub fn new(
        conversation_id: String,
        project_id: String,
        retained_start: u32,
        retained_message_id: String,
        history_sha256: String,
    ) -> Result<Self, WorkingContextError> {
        let value = Self {
            version: 1,
            conversation_id,
            project_id,
            retained_start,
            retained_message_id,
            history_sha256,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validates persisted policy version and bounded identifiers.
    ///
    /// # Errors
    /// Rejects unsupported versions or malformed checkpoint values.
    pub fn validate(&self) -> Result<(), WorkingContextError> {
        if self.version != 1
            || [
                &self.conversation_id,
                &self.project_id,
                &self.retained_message_id,
            ]
            .iter()
            .any(|id| {
                id.is_empty()
                    || id.len() > 160
                    || id.chars().any(|c| c.is_whitespace() || c.is_control())
            })
            || self.history_sha256.len() != 64
            || !self
                .history_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(WorkingContextError::InvalidCheckpoint);
        }
        Ok(())
    }

    /// Owning conversation.
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    /// Owning project.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    /// Number of messages before the retained dialogue.
    #[must_use]
    pub const fn retained_start(&self) -> u32 {
        self.retained_start
    }
    /// First retained user message.
    #[must_use]
    pub fn retained_message_id(&self) -> &str {
        &self.retained_message_id
    }
    /// Digest of history including the first retained message.
    #[must_use]
    pub fn history_sha256(&self) -> &str {
        &self.history_sha256
    }
}
