//! Atomic correction receipts for a completion boundary and one malformed task.
//! Constructing or emitting these receipts never dispatches either operation.
use super::{
    BatchEventSink, ChatToolExchange, Conversation, IpcError, IpcErrorCode, ParsedToolCall,
    RequestId, TaskActivityStatus, ToolOutcome, emit_finish, emit_request_start,
    prepare_invalid_tool_correction,
};

pub(crate) struct RejectedStageOperationPair {
    exchanges: [ChatToolExchange; 2],
}
impl RejectedStageOperationPair {
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn exchanges(&self) -> &[ChatToolExchange; 2] {
        &self.exchanges
    }
}

pub(crate) fn stage_operation_argument_corrections(
    calls: &[ParsedToolCall],
    error: &IpcError,
) -> Result<RejectedStageOperationPair, IpcError> {
    let [control, operation] = calls else {
        return Err(IpcError::invalid_request(
            "Expected one stage boundary and one task",
        ));
    };
    if control.name != "orchestration.complete_stage"
        || control.call_id == operation.call_id
        || error.code != IpcErrorCode::InvalidRequest
        || super::super::tool_action_class(&operation.name).is_none()
    {
        return Err(IpcError::invalid_request(
            "Unsafe stage argument correction",
        ));
    }
    super::super::validated_chat_tool_request(control, &[])?;
    // Validate both envelopes before producing any receipt. Task arguments are
    // deliberately NOT admitted: only their bounded representation is retained.
    let control = prepare_invalid_tool_correction(control)?;
    let operation = prepare_invalid_tool_correction(operation)?;
    let mut deferred = super::corrections::tool_argument_correction_exchange(&control, error);
    deferred.result.outcome = ToolOutcome::Failed {
        error: IpcError::invalid_request(format!(
            "Stage completion was not executed because `{}` in the same batch has invalid arguments. Neither call was executed; the current stage and prior evidence remain unchanged. Correct only the task arguments using its advertised schema, then submit a new batch through normal admission. Do not report the stage or task as completed.",
            operation.call.name
        )),
    };
    deferred.activity.status = TaskActivityStatus::Partial;
    deferred.activity.detail = Some("Stage completion deferred; no operation executed".into());
    let task = super::corrections::tool_argument_correction_exchange(&operation, error);
    Ok(RejectedStageOperationPair {
        exchanges: [deferred, task],
    })
}

pub(crate) fn emit_stage_operation_argument_corrections(
    app: &tauri::AppHandle,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &Conversation,
    pair: RejectedStageOperationPair,
) -> Vec<Result<ChatToolExchange, IpcError>> {
    let mut events = BatchEventSink {
        app,
        request_id,
        sequence,
        conversation,
    };
    pair.exchanges
        .into_iter()
        .map(|exchange| {
            emit_request_start(&mut events, &exchange.request, &exchange.activity);
            emit_finish(&mut events, &exchange);
            Ok(exchange)
        })
        .collect()
}
