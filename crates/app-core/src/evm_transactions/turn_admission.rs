use crate::{AppCore, AppCoreError};

impl AppCore {
    /// Durably admits at most one transfer per persisted user turn, including retries.
    /// # Errors
    /// Returns a storage failure; no transaction may proceed in that case.
    pub fn claim_chat_transfer(
        &self,
        conversation: &ipc_types::ConversationId,
        message: &ipc_types::MessageId,
    ) -> Result<bool, AppCoreError> {
        self.with_storage(|storage| storage.claim_chat_transfer(conversation, message))
    }
}
