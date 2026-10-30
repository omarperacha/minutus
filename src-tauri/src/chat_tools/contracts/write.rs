//! Only the fixed closed-profile native controller can execute this tool.
use crate::evm_approval::contracts::{ContractWorkflowRequest, available, execute};
use ipc_types::{Conversation, IpcError, IpcErrorCode, ToolOutcome, ToolResultPart};
use model_runtime::{ParsedToolCall, PromptToolDefinition};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};
fn request(call: &ParsedToolCall) -> Result<ContractWorkflowRequest, IpcError> {
    if call.name != "contract.request_workflow" || !available() {
        return Err(IpcError::permission_denied(
            "Source-qualified contract writes are unavailable in this host",
        ));
    }
    let request: ContractWorkflowRequest = serde_json::from_value(json!(call.arguments))
        .map_err(|_| IpcError::invalid_request("Use the exact fixed-workflow fields"))?;
    request.validate()?;
    Ok(request)
}
pub(in crate::chat_tools) fn validate_write_call(call: &ParsedToolCall) -> Result<(), IpcError> {
    request(call).map(|_| ())
}
pub(in crate::chat_tools) fn execute_write(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancelled: &Arc<AtomicBool>,
) -> Result<ToolOutcome, IpcError> {
    let result = execute(app, conversation, &request(call)?, cancelled)?;
    if result["state"] != "verified" {
        return Ok(ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Unsupported,
                message: result.to_string(),
                retryable: false,
            },
        });
    }
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: super::super::filesystem::ipc_json_value(&result)?,
        }],
    })
}
pub(super) fn definition() -> PromptToolDefinition {
    PromptToolDefinition {name:"contract.request_workflow".into(),description:"Request one bounded immutable ordered contract workflow, including a single write. Advertised only in qualification builds passing packaged isolated-worker admission; release availability remains disabled pending native interaction and final security review. The host currently supports only exact approve(address,uint256), transfer(address,uint256), and transferFrom(address,address,uint256) on the bundled oz570-solc0838-cancun-erc20-v1 compiled token runtime, with zero native value. Unknown source/proxies/functions, swaps/bridges and nested app calls reject. Every target/deployment and declared function is admitted before the first write. Requires a semantic externalWrite plan, native outbound scope review before RPC, then independent exact native approval and fresh password for EACH write. Start with an existing selected account already unlocked in Wallet; if locked, unlock there and make a fresh request. This tool never unlocks before outbound consent. Each write still requires its own fresh password; later relocking/account changes face the normal readiness and approval checks. Network and the selected wallet account are fixed for the workflow; no model-selected signing account. Each next step is freshly prepared after all prior receipts and current qualified predicates are proved. All earlier predicates must remain true before a dependent write: workflows that consume an earlier allowance before a third step may stop conservatively. Provide exact raw token units and each token's original deployment transaction hash; the host acquires/verifies public transaction and receipt trie proofs. An allowance may remain if a later step is denied, fails, reverts or is uncertain. No automatic cleanup/retry, wallet creation/import/funding or arbitrary raw-body signing. Report partial facts and remaining authority honestly; acknowledgement or receipt status alone is not task success.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"network":{"type":"string","enum":["baseMainnet","baseSepolia","ethereumMainnet","ethereumSepolia"]},"steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"target":{"type":"string","pattern":"^0x[0-9a-fA-F]{40}$"},"function":{"type":"string","enum":["approve(address,uint256)","transfer(address,uint256)","transferFrom(address,address,uint256)"]},"arguments":{"type":"array","maxItems":3,"items":{"type":"string","maxLength":78}},"valueWei":{"type":"string","const":"0"},"deploymentTransaction":{"type":"string","pattern":"^0x[0-9a-fA-F]{64}$"}},"required":["target","function","arguments","valueWei","deploymentTransaction"]}}},"required":["network","steps"]})}
}
