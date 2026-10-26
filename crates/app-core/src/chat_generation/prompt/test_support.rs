//! Feature-gated cross-crate trajectory inspection using production assembly.

use super::{
    AppCore, AppCoreError, Conversation, ModelAdapterKind, PromptRenderRequest,
    PromptToolDefinition, ReasoningStrength, RichInputPresentationPolicy, ToolOrchestrationContext,
    WorkingPromptAdapterContext, WorkingPromptContext, working_context,
};

impl AppCore {
    /// Assembles the real provider-neutral request without model loading or provider I/O.
    ///
    /// This test-support entry point lets the desktop's authoritative state
    /// transition tests inspect the same selected history and host tail passed
    /// to the native adapter. It does not simulate model behavior or admission.
    ///
    /// # Errors
    /// Returns ordinary history, presentation or prompt assembly errors.
    pub fn assemble_trajectory_test_prompt(
        &self,
        conversation: &Conversation,
        tools: Vec<PromptToolDefinition>,
        orchestration: ToolOrchestrationContext,
    ) -> Result<PromptRenderRequest, AppCoreError> {
        let working = working_context::continuity::preview(self, conversation)?;
        Self::build_working_chat_request(
            conversation,
            WorkingPromptAdapterContext {
                current_date: "2026-09-11",
                tools,
                adapter: ModelAdapterKind::QwenNative,
                reasoning_strength: ReasoningStrength::Low,
            },
            WorkingPromptContext {
                working: &working,
                summary: None,
                ledger: None,
                orchestration: Some(orchestration),
                presentation: &RichInputPresentationPolicy::default().resolve(self, "")?,
                rich_evidence: None,
                compact_completed_exchanges: false,
                output_token_limit: None,
            },
        )
        .map(|(request, _)| request)
    }
}
