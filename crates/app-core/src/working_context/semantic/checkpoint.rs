//! Checkpoint identity, reuse validation, and source-state digests.

use agent_core::{ModelLedgerProjection, SemanticCompactionCheckpoint};
use ipc_types::Conversation;
use sha2::{Digest, Sha256};

use crate::{AppCore, AppCoreError};

use super::SUMMARY_POLICY_VERSION;

// A narrative is a historical claim, never a reusable live observation. The
// independent rich-evidence checkpoint still compares source_state_digest;
// retained payload rehydration validates exact source bindings at its owner.
pub(super) fn checkpoint_reusable(
    checkpoint: &SemanticCompactionCheckpoint,
    conversation: &Conversation,
    target_count: usize,
    binding_sha256: &str,
) -> bool {
    let Ok(covered_count) = usize::try_from(checkpoint.covered_message_count()) else {
        return false;
    };
    checkpoint.conversation_id() == conversation.id.0
        && checkpoint.project_id() == conversation.project_id.0
        && checkpoint.summary_policy_version() == SUMMARY_POLICY_VERSION
        && checkpoint.binding_sha256() == binding_sha256
        && covered_count <= target_count
        && covered_count > 0
        && conversation
            .messages
            .get(covered_count - 1)
            .is_some_and(|message| message.id.0 == checkpoint.covered_message_id())
        && history_digest(conversation, covered_count)
            .is_ok_and(|digest| digest == checkpoint.history_sha256())
}

pub(crate) fn source_state_digest(
    core: &AppCore,
    conversation: &Conversation,
    ledger: Option<&ModelLedgerProjection>,
) -> Result<String, AppCoreError> {
    let workspaces = core.project_workspaces(&conversation.project_id)?;
    let encoded = serde_json::to_vec(&(workspaces, ledger))
        .map_err(|_| agent_core::WorkingContextError::InvalidCheckpoint)?;
    Ok(hex_digest(&encoded))
}

pub(crate) fn history_digest(
    conversation: &Conversation,
    message_count: usize,
) -> Result<String, AppCoreError> {
    let messages = conversation
        .messages
        .get(..message_count)
        .ok_or(agent_core::WorkingContextError::InvalidCheckpoint)?;
    let encoded = serde_json::to_vec(messages)
        .map_err(|_| agent_core::WorkingContextError::InvalidCheckpoint)?;
    Ok(hex_digest(&encoded))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut digest, byte| {
            digest.push(char::from(HEX[usize::from(byte >> 4)]));
            digest.push(char::from(HEX[usize::from(byte & 0x0f)]));
            digest
        })
}

// Persist deterministic fallback too, avoiding repeated auxiliary inference for
// the same historical span. Its incomplete marker remains part of the summary.
pub(super) fn save_narrative(
    core: &AppCore,
    conversation: &Conversation,
    count: u32,
    source_digest: &str,
    profile: &model_runtime::ActiveInferenceProfile,
    summary: &agent_core::UntrustedWorkingContextSummary,
) -> Result<(), AppCoreError> {
    let count_usize =
        usize::try_from(count).map_err(|_| agent_core::WorkingContextError::InvalidCheckpoint)?;
    let last = count_usize
        .checked_sub(1)
        .and_then(|index| conversation.messages.get(index))
        .ok_or(agent_core::WorkingContextError::InvalidCheckpoint)?;
    let updated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| agent_core::WorkingContextError::InvalidCheckpoint)?;
    let checkpoint = SemanticCompactionCheckpoint::new(
        conversation.id.0.clone(),
        conversation.project_id.0.clone(),
        count,
        last.id.0.clone(),
        history_digest(conversation, count_usize)?,
        source_digest,
        SUMMARY_POLICY_VERSION,
        profile.binding().identity_sha256(),
        summary.clone(),
        updated_at,
    )?;
    core.with_storage(|storage| storage.save_semantic_compaction_checkpoint(&checkpoint))
}
