//! Exact output-only admission for a bounded report already composed by the host.
use super::{
    AppCore, AppCoreError, Conversation, ExecutionProfileDecision, GenerationBinding,
    product_output_tokens, selected_round_budget, working_context,
};

impl AppCore {
    /// Checks a host report against the ordinary selected final-answer budget.
    ///
    /// This neither assembles a prompt nor starts inference. A missing/stale
    /// model or tokenizer rejects admission so the caller can use normal chat.
    /// It does not establish that the reported task succeeded.
    ///
    /// # Errors
    /// Returns an error when the binding, ledger or exact tokenizer is unavailable.
    pub fn admit_host_final_report(
        &self,
        conversation: &Conversation,
        binding: &GenerationBinding,
        text: &str,
        decision: ExecutionProfileDecision,
    ) -> Result<bool, AppCoreError> {
        if decision.work != agent_core::ExecutionWorkKind::FinalResponse {
            return Ok(false);
        }
        let selection = self.conversation_model(&conversation.id)?;
        if !selection.is_some_and(|selection| {
            selection.model_id == binding.model_id()
                && selection.model_version == binding.model_version()
        }) {
            return Ok(false);
        }
        let model = self.model_for_binding(binding)?;
        let ledger = working_context::semantic::fresh_ledger(self, conversation)?;
        let (budget, _) =
            selected_round_budget(ledger.as_ref(), decision, product_output_tokens(model));
        let tokens = self.exact_generated_text_tokens(binding, text)?;
        Ok(tokens <= u64::from(budget.output_tokens))
    }
}
