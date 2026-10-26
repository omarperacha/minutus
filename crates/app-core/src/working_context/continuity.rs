//! Conversation-scoped selection independent of request and plan lifetimes.

use super::{WorkingConversation, semantic::history_digest, summarize};
use crate::{AppCore, AppCoreError};
use agent_core::{DialogueContinuityCheckpoint, WorkingContextError};
use ipc_types::{Conversation, MessageRole};

/// Preview uses the stored boundary, but never creates state for an unsaved chat.
pub(crate) fn preview(
    core: &AppCore,
    conversation: &Conversation,
) -> Result<WorkingConversation, AppCoreError> {
    match core.with_storage(|storage| {
        storage.dialogue_continuity_checkpoint(&conversation.id.0, &conversation.project_id.0)
    })? {
        Some(checkpoint) => restore(conversation, &checkpoint),
        None => full_dialogue(conversation),
    }
}

/// Pin before model work, including the initial planning/clarification round.
pub(crate) fn prepare(
    core: &AppCore,
    conversation: &Conversation,
) -> Result<WorkingConversation, AppCoreError> {
    if let Some(checkpoint) = core.with_storage(|storage| {
        storage.dialogue_continuity_checkpoint(&conversation.id.0, &conversation.project_id.0)
    })? {
        return restore(conversation, &checkpoint);
    }
    let working = full_dialogue(conversation)?;
    let start = working.message_range().start;
    let message = conversation
        .messages
        .get(start)
        .ok_or(WorkingContextError::InvalidCheckpoint)?;
    let checkpoint = DialogueContinuityCheckpoint::new(
        conversation.id.0.clone(),
        conversation.project_id.0.clone(),
        u32::try_from(start).map_err(|_| WorkingContextError::InvalidCheckpoint)?,
        message.id.0.clone(),
        history_digest(conversation, start + 1)?,
    )?;
    let installed = core
        .with_storage(|storage| {
            storage.initialise_dialogue_continuity(&checkpoint)?;
            storage.dialogue_continuity_checkpoint(&conversation.id.0, &conversation.project_id.0)
        })?
        .ok_or(WorkingContextError::InvalidCheckpoint)?;
    restore(conversation, &installed)
}

fn full_dialogue(conversation: &Conversation) -> Result<WorkingConversation, AppCoreError> {
    if !conversation
        .messages
        .first()
        .is_some_and(|message| message.role == MessageRole::User)
    {
        return Err(WorkingContextError::InvalidRound.into());
    }
    Ok(WorkingConversation {
        message_range: 0..conversation.messages.len(),
        compacted_message_range: None,
        fallback_summary: None,
    })
}

fn restore(
    conversation: &Conversation,
    checkpoint: &DialogueContinuityCheckpoint,
) -> Result<WorkingConversation, AppCoreError> {
    checkpoint.validate()?;
    let start = usize::try_from(checkpoint.retained_start())
        .map_err(|_| WorkingContextError::InvalidCheckpoint)?;
    if checkpoint.conversation_id() != conversation.id.0
        || checkpoint.project_id() != conversation.project_id.0
        || !conversation.messages.get(start).is_some_and(|message| {
            message.id.0 == checkpoint.retained_message_id() && message.role == MessageRole::User
        })
        || history_digest(conversation, start + 1)? != checkpoint.history_sha256()
    {
        return Err(WorkingContextError::InvalidCheckpoint.into());
    }
    // Ordinary progression never advances this boundary. Only an exactly
    // admitted pressure checkpoint can replace it with compare-and-swap.
    Ok(WorkingConversation {
        message_range: start..conversation.messages.len(),
        compacted_message_range: (start > 0).then_some(0..start),
        fallback_summary: if start > 0 {
            Some(summarize(conversation, 0..start)?)
        } else {
            None
        },
    })
}

/// Proposes one older complete span after exact admission fails. No storage effects.
/// The latest user round and every native call/result group stay indivisible.
pub(crate) fn pressure_candidate(
    conversation: &Conversation,
    working: &WorkingConversation,
) -> Result<WorkingConversation, AppCoreError> {
    let start = conversation
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .filter(|start| *start > working.message_range().start)
        .ok_or(WorkingContextError::ContextCapacityGap)?;
    let mut pending = std::collections::BTreeSet::new();
    for message in &conversation.messages[..start] {
        for part in &message.parts {
            match part {
                ipc_types::MessagePart::ToolCall { request } => {
                    pending.insert(&request.call_id.0);
                }
                ipc_types::MessagePart::ToolResult { result } => {
                    pending.remove(&result.call_id.0);
                }
                _ => {}
            }
        }
    }
    if !pending.is_empty() {
        return Err(WorkingContextError::ContextCapacityGap.into());
    }
    Ok(WorkingConversation {
        message_range: start..conversation.messages.len(),
        compacted_message_range: Some(0..start),
        fallback_summary: Some(summarize(conversation, 0..start)?),
    })
}

/// Installs only after the candidate's complete native prompt passes exact admission.
pub(crate) fn commit_pressure(
    core: &AppCore,
    conversation: &Conversation,
    previous: &WorkingConversation,
    next: &WorkingConversation,
) -> Result<(), AppCoreError> {
    let checkpoint = |start: usize| -> Result<DialogueContinuityCheckpoint, AppCoreError> {
        let message = conversation
            .messages
            .get(start)
            .ok_or(WorkingContextError::InvalidCheckpoint)?;
        Ok(DialogueContinuityCheckpoint::new(
            conversation.id.0.clone(),
            conversation.project_id.0.clone(),
            u32::try_from(start).map_err(|_| WorkingContextError::InvalidCheckpoint)?,
            message.id.0.clone(),
            history_digest(conversation, start + 1)?,
        )?)
    };
    let previous = checkpoint(previous.message_range().start)?;
    let next = checkpoint(next.message_range().start)?;
    if !core.with_storage(|storage| storage.advance_dialogue_continuity(&previous, &next))? {
        return Err(WorkingContextError::InvalidCheckpoint.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
