//! Request-local replay of previously admitted Qwen host capsules.

use ipc_types::AssetId;
use model_runtime::{PromptMessage, PromptRenderRequest};

/// Ephemeral provider history. Never persisted, summarized, recalled or logged.
/// Exact prefix equality and a host request/conversation/runtime binding prevent
/// replay across requests or after selection, image or binding changes.
#[derive(Clone, Default)]
pub struct ChatPromptContinuation {
    binding: String,
    base: Vec<PromptMessage>,
    admitted: Vec<PromptMessage>,
    assets: Vec<AssetId>,
}

impl ChatPromptContinuation {
    pub(super) fn has_history(&self) -> bool {
        !self.base.is_empty()
    }

    pub(super) fn replay(
        &self,
        binding: &str,
        assets: &[AssetId],
        messages: &mut Vec<PromptMessage>,
    ) {
        if self.binding == binding
            && assets.starts_with(&self.assets)
            && !self.base.is_empty()
            && messages.len() > self.base.len()
            && messages.starts_with(&self.base)
        {
            let appended = messages.split_off(self.base.len());
            messages.clone_from(&self.admitted);
            messages.extend(appended);
        }
    }

    pub(super) fn admitted(
        binding: String,
        base: Vec<PromptMessage>,
        request: &PromptRenderRequest,
        assets: Vec<AssetId>,
    ) -> Self {
        Self {
            binding,
            base,
            admitted: request.messages.clone(),
            assets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_runtime::{PromptContent, ReasoningStrength};

    fn user(text: &str) -> PromptMessage {
        PromptMessage::User {
            content: vec![PromptContent::Text(text.into())],
        }
    }

    #[test]
    fn extension_replays_capsules_but_changed_history_and_binding_do_not() {
        let base = vec![user("task")];
        let request = PromptRenderRequest {
            messages: vec![user("task"), user("host capsule")],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-10-14".into(),
            reasoning_strength: ReasoningStrength::Medium,
            add_generation_prompt: true,
        };
        let continuation = ChatPromptContinuation::admitted(
            "binding".into(),
            base,
            &request,
            vec![AssetId("image".into())],
        );
        let mut extension = vec![user("task"), user("next result")];
        continuation.replay("binding", &[AssetId("image".into())], &mut extension);
        assert_eq!(
            extension,
            vec![user("task"), user("host capsule"), user("next result")]
        );
        for (binding, mut messages) in [
            ("other", vec![user("task"), user("next result")]),
            ("binding", vec![user("changed task"), user("next result")]),
            ("binding", vec![user("task")]),
        ] {
            let expected = messages.clone();
            continuation.replay(binding, &[AssetId("image".into())], &mut messages);
            assert_eq!(messages, expected);
        }
        let mut replaced_image = vec![user("task"), user("next result")];
        let expected = replaced_image.clone();
        continuation.replay(
            "binding",
            &[AssetId("other-image".into())],
            &mut replaced_image,
        );
        assert_eq!(replaced_image, expected);
    }
}
