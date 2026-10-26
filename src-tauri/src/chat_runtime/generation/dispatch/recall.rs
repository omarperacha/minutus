//! Recall dispatch is inert: no task broker, progress credit, or stored-call execution.

use super::{
    AppCore, ChatGenerationRound, ChatTerminal, Conversation, IpcError, RequestId,
    complete_control_call, persist_unfinished_tool_batch, reject_invalid_tool_batch,
};

pub(in crate::chat_runtime::generation) fn execute(
    app: &tauri::AppHandle,
    core: &AppCore,
    request_id: &RequestId,
    sequence: &mut u32,
    conversation: &mut Conversation,
    generated: ChatGenerationRound,
) -> Option<ChatTerminal> {
    let outcome = match generated.calls.as_slice() {
        [call] => crate::chat_tools::recall::arguments(call).and_then(|(reference, offset, limit)| {
            core.recall_conversation_context(conversation, reference, offset, limit)
                .map_err(|_| IpcError::invalid_request("Context recall failed validation. Re-list the current conversation index; if the exact input remains unavailable, report the context gap."))
        }),
        _ => Err(IpcError::scope_correction("orchestration.recall must be called alone; no calls in this batch were executed.")),
    };
    let exchanges = match outcome {
        Ok(value) => vec![complete_control_call(
            app,
            request_id,
            sequence,
            conversation,
            &generated.calls[0],
            &value,
        )],
        Err(error) => reject_invalid_tool_batch(
            app,
            request_id,
            sequence,
            conversation,
            &generated.calls,
            &error,
        ),
    };
    // Content may contain exact private user inputs. Do not trace payloads.
    persist_unfinished_tool_batch(core, request_id, conversation, generated, exchanges)
}
