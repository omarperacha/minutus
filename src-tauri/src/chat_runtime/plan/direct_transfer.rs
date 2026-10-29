//! Admission of one standalone native-reviewed transfer without semantic stages.

use model_runtime::ParsedToolCall;

pub(in crate::chat_runtime) fn is_standalone_transfer(calls: &[ParsedToolCall]) -> bool {
    matches!(calls, [call] if call.name == "wallet.request_transfer")
}
