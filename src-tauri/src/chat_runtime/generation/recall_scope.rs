//! Startup recall eligibility depends on retained history, never inferred intent.

use super::{Conversation, GenerationToolState, MessageRole};

#[derive(Clone, Copy, Default)]
pub(super) enum RecallScope {
    /// Conservative default for unknown history, including restored conversations.
    #[default]
    Historical,
    CurrentInputOnly,
}

impl RecallScope {
    pub(super) fn for_conversation(conversation: &Conversation) -> Self {
        match conversation.messages.as_slice() {
            [message] if message.role == MessageRole::User => Self::CurrentInputOnly,
            _ => Self::Historical,
        }
    }
}

impl GenerationToolState {
    pub(super) fn recall_available(&self) -> bool {
        !self.plan_policy.final_response_ready()
            && (matches!(self.recall_scope, RecallScope::Historical)
                || self.plan_policy.active()
                || !self.preplan_result_call_ids.is_empty())
    }
}
