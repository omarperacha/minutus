//! Pre-execution stage/task correction retains the live plan and effect fences.
use super::{
    ChatTerminal, GenerationToolState, IpcError, IpcErrorCode, MAX_ARGUMENT_CORRECTIONS,
    ParsedToolCall, PromptToolDefinition, RecoveryKind, argument_correction_fingerprint,
    record_argument_correction,
};
use crate::chat_tools::{RejectedStageOperationPair, stage_operation_argument_corrections};

pub(super) fn prepare(
    state: &mut GenerationToolState,
    calls: &[ParsedToolCall],
    invalid: &[(usize, IpcError)],
    catalogue: &[PromptToolDefinition],
    error: &IpcError,
) -> Result<Option<RejectedStageOperationPair>, ChatTerminal> {
    if !matches!(invalid, [(1, _)])
        || state.argument_corrections >= MAX_ARGUMENT_CORRECTIONS
        || error.code != IpcErrorCode::InvalidRequest
        || calls.len() != 2
        || !calls
            .iter()
            .all(|call| catalogue.iter().any(|tool| tool.name == call.name))
        || state
            .plan_policy
            .validate_advisory_stage_completion(&calls[0])
            .is_err()
    {
        return Ok(None);
    }
    let Ok(pair) = stage_operation_argument_corrections(calls, error) else {
        return Ok(None);
    };
    state.request_recovery(
        RecoveryKind::InvalidArguments,
        state.plan_policy.correction_scope(),
        argument_correction_fingerprint(calls, error),
        format!(
            "Neither the stage completion nor `{}` was executed. Its arguments failed validation: {} Retain the same stage, plan and prior evidence. Use the advertised schema to express the intended condition; do not guess or silently weaken unsupported conditions. Submit corrected calls for ordinary admission. This correction grants no effect permission or new write attempt.",
            calls[1].name, error.message
        ),
    )?;
    state.record_calls(calls);
    record_argument_correction(state, calls, error);
    Ok(Some(pair))
}

#[cfg(debug_assertions)]
mod qualification;
#[cfg(debug_assertions)]
pub(crate) use qualification::run as qualify_stage_argument_recovery;

#[cfg(test)]
mod tests;
