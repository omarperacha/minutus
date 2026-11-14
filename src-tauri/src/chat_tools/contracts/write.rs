//! Versioned declarations reach only their exact native qualification controller.
use crate::evm_approval::contracts::{
    ContractWorkflowRequest, DeclaredWorkflowRequest, available, execute, execute_declared,
    generic_available,
};
use ipc_types::{
    Conversation, IpcError, IpcErrorCode, TaskActivityStatus, ToolOutcome, ToolResultPart,
};
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
pub(in crate::chat_tools) struct WorkflowToolResult {
    pub(in crate::chat_tools) outcome: ToolOutcome,
    pub(in crate::chat_tools) status: TaskActivityStatus,
    pub(in crate::chat_tools) detail: &'static str,
}
pub(in crate::chat_tools) fn execute_write(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancelled: &Arc<AtomicBool>,
) -> Result<WorkflowToolResult, IpcError> {
    let result = match request(call)? {
        WorkflowRequest::Profile(request) => execute(app, conversation, &request, cancelled)?,
        WorkflowRequest::Declared(request) => {
            execute_declared(app, conversation, &request, cancelled)?
        }
    };
    project_result(&result)
}
fn project_result(result: &serde_json::Value) -> Result<WorkflowToolResult, IpcError> {
    let (status, detail) = match result["state"].as_str() {
        Some("verified") => (
            TaskActivityStatus::Completed,
            "Workflow execution and declared goals verified",
        ),
        Some("submittedVerificationUnavailable") => (
            TaskActivityStatus::Partial,
            "Transaction submitted or possibly sent; verification unavailable · do not resubmit",
        ),
        Some("executionConfirmed") => (
            TaskActivityStatus::Partial,
            "Execution confirmed; declared goals are unsatisfied or undeclared",
        ),
        _ => {
            return Ok(WorkflowToolResult {
                outcome: ToolOutcome::Failed {
                    error: IpcError {
                        code: IpcErrorCode::Unsupported,
                        message: result.to_string(),
                        retryable: false,
                    },
                },
                status: TaskActivityStatus::Failed,
                detail: "Workflow stopped · transaction execution is a separate lifecycle",
            });
        }
    };
    Ok(WorkflowToolResult {
        status,
        detail,
        outcome: ToolOutcome::Completed {
            content: vec![ToolResultPart::Json {
                value: super::super::filesystem::ipc_json_value(result)?,
            }],
        },
    })
}
pub(super) fn definition() -> PromptToolDefinition {
    if generic_available() {
        return super::workflow_schema::definition();
    }
    PromptToolDefinition {name:"contract.request_workflow".into(),description:"Request one bounded immutable ordered contract workflow, including a single write. Advertised only in qualification builds passing packaged isolated-worker admission; release availability remains disabled pending native interaction and final security review. The host currently supports only exact approve(address,uint256), transfer(address,uint256), and transferFrom(address,address,uint256) on the bundled oz570-solc0838-cancun-erc20-v1 compiled token runtime, with zero native value. Unknown source/proxies/functions, swaps/bridges and nested app calls reject. Every target/deployment and declared function is admitted before the first write. Requires a semantic externalWrite plan, native outbound scope review before RPC, then independent exact native approval and fresh password for EACH write. An existing selected account is required. If locked, the host prompts native unlock in chat before outbound scope consent; cancelling ends this attempt. Unlocking grants no transaction approval. Each write still requires its own fresh password; later relocking/account changes face the normal readiness and approval checks. Network and the selected wallet account are fixed for the workflow; no model-selected signing account. Each next step is freshly prepared after all prior receipts and current qualified predicates are proved. All earlier predicates must remain true before a dependent write: workflows that consume an earlier allowance before a third step may stop conservatively. Provide exact raw token units and each token's original deployment transaction hash; the host acquires/verifies public transaction and receipt trie proofs. An allowance may remain if a later step is denied, fails, reverts or is uncertain. No automatic cleanup/retry, wallet creation/import/funding or arbitrary raw-body signing. Report partial facts and remaining authority honestly; acknowledgement or receipt status alone is not task success. Report only explicit host lifecycle facts for the same attempt and step. Initial unlock and outbound-scope consent do not establish exact transaction approval or fresh signing authentication. An error code alone does not establish which prompt, authentication, signature or submission occurred; retain unknown states and do not invent denial, skipped passwords, source changes or a known-unsent outcome. A missing returned signature does not prove signing never began. Earlier reads describe their listed blocks and do not prove current or unchanged state. An executed workflow failure remains stopped without automatic retry or cleanup.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"network":{"type":"string","enum":["baseMainnet","baseSepolia","ethereumMainnet","ethereumSepolia"]},"steps":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","additionalProperties":false,"properties":{"target":{"type":"string","pattern":"^0x[0-9a-fA-F]{40}$"},"function":{"type":"string","enum":["approve(address,uint256)","transfer(address,uint256)","transferFrom(address,address,uint256)"]},"arguments":{"type":"array","maxItems":3,"items":{"type":"string","maxLength":78}},"valueWei":{"type":"string","const":"0"},"deploymentTransaction":{"type":"string","pattern":"^0x[0-9a-fA-F]{64}$"}},"required":["target","function","arguments","valueWei","deploymentTransaction"]}}},"required":["network","steps"]})}
}

#[cfg(test)]
mod result_tests {
    use super::*;
    #[test]
    fn acknowledged_or_possible_submission_is_partial_without_workflow_goal_credit()
    -> Result<(), IpcError> {
        for state in ["submittedVerificationUnavailable", "executionConfirmed"] {
            let original = json!({"state":state,"transactionHash":format!("0x{}","a".repeat(64)),"retryAutomatically":false});
            let result = project_result(&original)?;
            assert_eq!(result.status, TaskActivityStatus::Partial);
            let ToolOutcome::Completed { content } = result.outcome else {
                return Err(IpcError::internal());
            };
            let [ToolResultPart::Json { value }] = content.as_slice() else {
                return Err(IpcError::internal());
            };
            assert_eq!(
                *value,
                super::super::super::filesystem::ipc_json_value(&original)?
            );
        }
        Ok(())
    }
    #[test]
    fn only_verified_result_gets_completed_workflow_status() -> Result<(), IpcError> {
        assert_eq!(
            project_result(&json!({"state":"verified"}))?.status,
            TaskActivityStatus::Completed
        );
        for state in [
            "stopped",
            "deniedOrDismissed",
            "alreadyAttempted",
            "unknown",
        ] {
            let result = project_result(&json!({"state":state}))?;
            assert_eq!(result.status, TaskActivityStatus::Failed);
            assert!(matches!(result.outcome,ToolOutcome::Failed{error} if !error.retryable));
        }
        Ok(())
    }
}
