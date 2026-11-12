//! Versioned declarations reach only their exact native qualification controller.
use crate::evm_approval::contracts::{
    ContractWorkflowRequest, DeclaredWorkflowRequest, available, execute, execute_declared,
    generic_available,
};
use ipc_types::{Conversation, IpcError, IpcErrorCode, ToolOutcome, ToolResultPart};
use model_runtime::{ParsedToolCall, PromptToolDefinition};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};
enum WorkflowRequest {
    Profile(ContractWorkflowRequest),
    Declared(DeclaredWorkflowRequest),
}
fn request(call: &ParsedToolCall) -> Result<WorkflowRequest, IpcError> {
    if call.name != "contract.request_workflow" || !available() {
        return Err(IpcError::permission_denied(
            "Source-qualified contract writes are unavailable in this host",
        ));
    }
    if let Some(version) = call.arguments.get("contractVersion") {
        if version != &json!(2) {
            return Err(IpcError::invalid_request(
                "Unsupported contract workflow version",
            ));
        }
        if !generic_available() {
            return Err(IpcError::permission_denied(
                "Packaged generic source qualification is unavailable",
            ));
        }
        let request: DeclaredWorkflowRequest = serde_json::from_value(json!(call.arguments))
            .map_err(|_| {
                IpcError::invalid_request("Use the exact version 2 declared-workflow fields")
            })?;
        request.validate()?;
        return Ok(WorkflowRequest::Declared(request));
    }
    let request: ContractWorkflowRequest = serde_json::from_value(json!(call.arguments))
        .map_err(|_| IpcError::invalid_request("Use the exact fixed-workflow fields"))?;
    request.validate()?;
    Ok(WorkflowRequest::Profile(request))
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
    let result = match request(call)? {
        WorkflowRequest::Profile(request) => execute(app, conversation, &request, cancelled)?,
        WorkflowRequest::Declared(request) => {
            execute_declared(app, conversation, &request, cancelled)?
        }
    };
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
    if generic_available() {
        return super::workflow_schema::definition();
    }
    PromptToolDefinition {name:"contract.request_workflow".into(),description:"Request one bounded immutable ordered contract workflow, including a single write. Advertised only in qualification builds passing packaged isolated-worker admission; release availability remains disabled pending native interaction and final security review. The host currently supports only exact approve(address,uint256), transfer(address,uint256), and transferFrom(address,address,uint256) on the bundled oz570-solc0838-cancun-erc20-v1 compiled token runtime, with zero native value. Unknown source/proxies/functions, swaps/bridges and nested app calls reject. Every target/deployment and declared function is admitted before the first write. Requires a semantic externalWrite plan, native outbound scope review before RPC, then independent exact native approval and fresh password for EACH write. An existing selected account is required. If locked, the host prompts native unlock in chat before outbound scope consent; cancelling ends this attempt. Unlocking grants no transaction approval. Each write still requires its own fresh password; later relocking/account changes face the normal readiness and approval checks. Network and the selected wallet account are fixed for the workflow; no model-selected signing account. Each next step is freshly prepared after all prior receipts and current qualified predicates are proved. All earlier predicates must remain true before a dependent write: workflows that consume an earlier allowance before a third step may stop conservatively. Provide exact raw token units and each token's original deployment transaction hash; the host acquires/verifies public transaction and receipt trie proofs. An allowance may remain if a later step is denied, fails, reverts or is uncertain. No automatic cleanup/retry, wallet creation/import/funding or arbitrary raw-body signing. Report partial facts and remaining authority honestly; acknowledgement or receipt status alone is not task success. Report only explicit host lifecycle facts for the same attempt and step. Initial unlock and outbound-scope consent do not establish exact transaction approval or fresh signing authentication. An error code alone does not establish which prompt, authentication, signature or submission occurred; retain unknown states and do not invent denial, skipped passwords, source changes or a known-unsent outcome. A missing returned signature does not prove signing never began. Earlier reads describe their listed blocks and do not prove current or unchanged state. An executed workflow failure remains stopped without automatic retry or cleanup.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"network":{"type":"string","enum":["baseMainnet","baseSepolia","ethereumMainnet","ethereumSepolia"]},"steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"target":{"type":"string","pattern":"^0x[0-9a-fA-F]{40}$"},"function":{"type":"string","enum":["approve(address,uint256)","transfer(address,uint256)","transferFrom(address,address,uint256)"]},"arguments":{"type":"array","maxItems":3,"items":{"type":"string","maxLength":78}},"valueWei":{"type":"string","const":"0"},"deploymentTransaction":{"type":"string","pattern":"^0x[0-9a-fA-F]{64}$"}},"required":["target","function","arguments","valueWei","deploymentTransaction"]}}},"required":["network","steps"]})}
}
