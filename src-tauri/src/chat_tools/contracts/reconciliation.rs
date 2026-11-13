//! The model selects a retained hash, never network, account, endpoint or body.
use ipc_types::{Conversation, IpcError, ToolOutcome, ToolResultPart};
use model_runtime::{ParsedToolCall, PromptToolDefinition};
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    transaction_hash: String,
}
fn request(call: &ParsedToolCall) -> Result<String, IpcError> {
    if call.name != "contract.reconcile_transaction" {
        return Err(IpcError::invalid_request(
            "Expected a read-only transaction reconciliation",
        ));
    }
    let request: Request = serde_json::from_value(json!(call.arguments))
        .map_err(|_| IpcError::invalid_request("Provide only the retained transactionHash"))?;
    chain_evm::TransactionHash::new(&request.transaction_hash)
        .map(|hash| hash.as_str().to_owned())
        .map_err(|_| IpcError::invalid_request("Expected one exact transaction hash"))
}
pub(in crate::chat_tools) fn validate(call: &ParsedToolCall) -> Result<(), IpcError> {
    request(call).map(|_| ())
}
pub(in crate::chat_tools) fn execute(
    app: &tauri::AppHandle,
    conversation: &Conversation,
    call: &ParsedToolCall,
    cancelled: &Arc<AtomicBool>,
) -> Result<ToolOutcome, IpcError> {
    let value = crate::evm_approval::contracts::reconcile_transaction(
        app,
        conversation,
        &request(call)?,
        cancelled,
    )?;
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: super::super::filesystem::ipc_json_value(&value)?,
        }],
    })
}
pub(super) fn definition() -> PromptToolDefinition {
    PromptToolDefinition{name:"contract.reconcile_transaction".into(),description:"Read-only reconciliation of one previously dispatched source-qualified transaction retained in this conversation, including after restart. Supply only its exact transactionHash. The host loads the original network/account/nonce/body digest from its journal; unknown or other-chat hashes reject before RPC. Fresh native outbound consent permits bounded configured-provider status and canonical full block transaction/receipt trie-proof acquisition. No wallet unlock, password, signing, submission, replacement, cleanup, new write attempt or continuation of remaining writes. Pending/missing/provider-unavailable remain explicit, never evidence of revert. Confirmed/reverted requires canonical inclusion bound to the exact stored signed body, signer, nonce and chain. Receipt success does not prove source effects, declared completion conditions, current goals, causation or finality; prior completion evidence remains historical and current goals are not rechecked. A plain contract.read returning the desired value cannot substitute for this transaction check. Reconciliation never retries the write or marks a workflow goal verified. Report the exact execution state and listed block, retaining unknown workflow completion.".into(),parameters:json!({"type":"object","additionalProperties":false,"properties":{"transactionHash":{"type":"string","pattern":"^0x[0-9a-fA-F]{64}$"}},"required":["transactionHash"]})}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_exact_hash_without_privileged_selectors()
    -> Result<(), Box<dyn std::error::Error>> {
        let make = |arguments| ParsedToolCall {
            call_id: "read".into(),
            recipient: "contract.reconcile_transaction".into(),
            name: "contract.reconcile_transaction".into(),
            arguments,
        };
        let value = json!({"transactionHash":format!("0x{}","ab".repeat(32))});
        assert!(validate(&make(serde_json::from_value(value.clone())?)).is_ok());
        for key in [
            "network",
            "account",
            "endpoint",
            "nonce",
            "rawTransaction",
            "resume",
            "retry",
        ] {
            let mut value = value.clone();
            value[key] = json!("forbidden");
            assert!(validate(&make(serde_json::from_value(value)?)).is_err());
        }
        for hash in ["", "0x12", "0xzz"] {
            assert!(
                validate(&make(serde_json::from_value(
                    json!({"transactionHash":hash})
                )?))
                .is_err()
            );
        }
        Ok(())
    }
}
