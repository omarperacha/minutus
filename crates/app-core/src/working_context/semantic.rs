//! Tools-disabled semantic summarization and checkpoint reuse.

use agent_core::{
    ContextAdmissionController, ContextAdmissionPolicy, ContextTokenEstimate, ExecutionLedger,
    ModelLedgerProjection, SemanticCompactionCheckpoint, UntrustedWorkingContextSummary,
};
use ipc_types::Conversation;
use model_runtime::{
    ActiveInferenceProfile, GenerationCompletion, GenerationEvent, InferenceError, ModelAdapterKind,
};

mod checkpoint;
mod input;

use checkpoint::checkpoint_reusable;
pub(crate) use checkpoint::{history_digest, source_state_digest};
#[cfg(test)]
use input::safe_message_value;
#[cfg(test)]
use input::semantic_prompt;
use input::semantic_prompt_request;

use super::WorkingConversation;
use crate::{AppCore, AppCoreError};

pub(super) const SUMMARY_POLICY_VERSION: u16 = 3;
const SUMMARY_REASONING_TOKENS: u64 = 256;
const SUMMARY_CONTENT_TOKENS: u64 = 768;
const SUMMARY_GENERATION_TOKENS: u32 = 1_024;

pub(crate) enum SemanticCompactionDisposition {
    NotNeeded,
    Checkpoint(UntrustedWorkingContextSummary),
    Generated(UntrustedWorkingContextSummary),
    Fallback(UntrustedWorkingContextSummary),
}

impl SemanticCompactionDisposition {
    pub(crate) const fn diagnostic_name(&self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::Checkpoint(_) => "reused",
            Self::Generated(_) => "generated",
            Self::Fallback(_) => "incomplete_fallback",
        }
    }

    pub(crate) fn summary(&self) -> Option<&UntrustedWorkingContextSummary> {
        match self {
            Self::NotNeeded => None,
            Self::Checkpoint(summary) | Self::Generated(summary) | Self::Fallback(summary) => {
                Some(summary)
            }
        }
    }
}

pub(crate) fn semantic_summary(
    core: &AppCore,
    generation_id: &str,
    conversation: &Conversation,
    working: &WorkingConversation,
    profile: &ActiveInferenceProfile,
) -> Result<SemanticCompactionDisposition, AppCoreError> {
    let Some(compacted_range) = working.compacted_message_range() else {
        return Ok(SemanticCompactionDisposition::NotNeeded);
    };
    let fallback = working
        .fallback_summary()
        .ok_or(agent_core::WorkingContextError::InvalidSummary)?
        .clone();
    let ledger = fresh_ledger(core, conversation)?;
    let ledger_projection = ledger.as_ref().map(ExecutionLedger::model_projection);
    let source_digest = source_state_digest(core, conversation, ledger_projection.as_ref())?;
    let target_count = compacted_range.end;
    let target_count_u32 = u32::try_from(target_count)
        .map_err(|_| agent_core::WorkingContextError::InvalidCheckpoint)?;

    let persist = |summary: &UntrustedWorkingContextSummary| {
        checkpoint::save_narrative(
            core,
            conversation,
            target_count_u32,
            &source_digest,
            profile,
            summary,
        )
    };
    let stored = core.with_storage(|storage| {
        storage.semantic_compaction_checkpoint(&conversation.id.0, &conversation.project_id.0)
    })?;
    let reusable = stored.as_ref().filter(|checkpoint| {
        checkpoint_reusable(
            checkpoint,
            conversation,
            target_count,
            &profile.binding().identity_sha256(),
        )
    });
    if stored.is_some() && reusable.is_none() {
        core.with_storage(|storage| {
            storage.delete_semantic_compaction_checkpoint(
                &conversation.id.0,
                &conversation.project_id.0,
            )
        })?;
    }
    if reusable.is_some_and(|checkpoint| checkpoint.covered_message_count() == target_count_u32) {
        return Ok(SemanticCompactionDisposition::Checkpoint(
            reusable
                .map(|checkpoint| checkpoint.summary().clone())
                .ok_or(agent_core::WorkingContextError::InvalidCheckpoint)?,
        ));
    }

    let start = reusable.map_or(0, |checkpoint| checkpoint.covered_message_count() as usize);
    let prior = reusable.map(SemanticCompactionCheckpoint::summary);
    let prompt = semantic_prompt_request(
        conversation,
        start..target_count,
        prior,
        &fallback,
        profile.binding().adapter(),
    )?;
    let prompt_tokens = semantic_prompt_tokens(core, profile, &prompt)?;
    let policy = ContextAdmissionPolicy::new(
        u64::from(profile.context_tokens()),
        SUMMARY_REASONING_TOKENS,
    )
    .map_err(|_| agent_core::WorkingContextError::InvalidPolicy)?;
    let admission = ContextAdmissionController::new(policy);
    if admission
        .admit_model_pass(
            u64::from(profile.context_tokens()),
            ContextTokenEstimate {
                conversation_history: prompt_tokens,
                ..ContextTokenEstimate::default()
            },
            SUMMARY_CONTENT_TOKENS,
        )
        .is_err()
    {
        persist(&fallback)?;
        return Ok(SemanticCompactionDisposition::Fallback(fallback));
    }

    let generated = match generate_summary(core, generation_id, prompt, profile) {
        Ok(value) => value,
        Err(AppCoreError::Inference(InferenceError::Cancelled)) => {
            return Err(InferenceError::Cancelled.into());
        }
        Err(_) => {
            persist(&fallback)?;
            return Ok(SemanticCompactionDisposition::Fallback(fallback));
        }
    };
    let Ok(summary) = UntrustedWorkingContextSummary::from_model_json(&generated) else {
        persist(&fallback)?;
        return Ok(SemanticCompactionDisposition::Fallback(fallback));
    };
    persist(&summary)?;
    Ok(SemanticCompactionDisposition::Generated(summary))
}

fn semantic_prompt_tokens(
    core: &AppCore,
    profile: &ActiveInferenceProfile,
    prompt: &model_runtime::PromptRenderRequest,
) -> Result<u64, AppCoreError> {
    let prepared = match profile.binding().adapter() {
        ModelAdapterKind::GlimmerAtem => crate::PreparedChatPrompt::Local {
            rendered: model_runtime::render_model_prompt(ModelAdapterKind::GlimmerAtem, prompt)?,
            tools: prompt.tools.clone(),
        },
        ModelAdapterKind::QwenNative => crate::PreparedChatPrompt::Qwen(prompt.clone()),
    };
    core.exact_prompt_tokens(&prepared, 0, SUMMARY_GENERATION_TOKENS, profile)
}

pub(crate) fn fresh_ledger_projection(
    core: &AppCore,
    conversation: &Conversation,
) -> Result<Option<ModelLedgerProjection>, AppCoreError> {
    fresh_ledger(core, conversation).map(|ledger| ledger.map(|ledger| ledger.model_projection()))
}

pub(crate) fn fresh_ledger(
    core: &AppCore,
    conversation: &Conversation,
) -> Result<Option<ExecutionLedger>, AppCoreError> {
    core.with_storage(|storage| {
        storage.execution_ledger_for_conversation(&conversation.id.0, &conversation.project_id.0)
    })?
    .map(ExecutionLedger::restore)
    .transpose()
    .map_err(|_| AppCoreError::WorkingContext(agent_core::WorkingContextError::InvalidCheckpoint))
}

fn generate_summary(
    core: &AppCore,
    generation_id: &str,
    prompt: model_runtime::PromptRenderRequest,
    profile: &ActiveInferenceProfile,
) -> Result<String, AppCoreError> {
    let prepared = match profile.binding().adapter() {
        ModelAdapterKind::GlimmerAtem => crate::PreparedChatPrompt::Local {
            rendered: model_runtime::render_model_prompt(ModelAdapterKind::GlimmerAtem, &prompt)?,
            tools: prompt.tools.clone(),
        },
        ModelAdapterKind::QwenNative => crate::PreparedChatPrompt::Qwen(prompt),
    };
    let mut content = String::new();
    let mut completed = false;
    let mut invalid = false;
    core.start_generation_with_assets(
        generation_id,
        prepared,
        profile.context_tokens(),
        SUMMARY_GENERATION_TOKENS,
        profile.binding(),
        &[],
        |event| match event {
            GenerationEvent::FinalContentDelta { text } => content.push_str(&text),
            GenerationEvent::Completed {
                reason: GenerationCompletion::Stop,
            } => completed = true,
            GenerationEvent::OutputStarted
            | GenerationEvent::ReasoningDelta { .. }
            | GenerationEvent::Metrics { .. }
            | GenerationEvent::ContentPreviewDelta { .. }
            | GenerationEvent::ContentPreviewReset => {}
            GenerationEvent::ToolCall { .. }
            | GenerationEvent::Completed {
                reason: GenerationCompletion::Length,
            }
            | GenerationEvent::Cancelled
            | GenerationEvent::Failed { .. } => invalid = true,
        },
    )?;
    if !completed || invalid {
        return Err(agent_core::WorkingContextError::InvalidSummary.into());
    }
    Ok(content)
}

#[cfg(test)]
mod tests;
