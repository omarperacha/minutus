use super::intent::{contract_error, request};
use chain_evm::contract::{
    observe_contract_code_with_admission, read_declared_contract_with_admission,
};
use ipc_types::{IpcError, IpcErrorCode, ToolOutcome, ToolResultPart};
use model_runtime::ParsedToolCall;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

pub(in crate::chat_tools) fn execute(
    app: &tauri::AppHandle,
    call: &ParsedToolCall,
    cancelled: &AtomicBool,
) -> Result<ToolOutcome, IpcError> {
    let request = request(call)?;
    check_cancelled(cancelled)?;
    let endpoint = crate::evm_rpc_settings::resolved(request.network)?;
    check_cancelled(cancelled)?;
    if !super::review::review(app, &request, &endpoint)? {
        return Err(IpcError::permission_denied(
            "The unsigned contract observation was not approved; do not retry automatically",
        ));
    }
    check_cancelled(cancelled)?;
    let admit = || !cancelled.load(Ordering::Acquire);
    let result = if let Some(call) = request.call {
        serde_json::to_value(
            read_declared_contract_with_admission(
                request.network,
                &endpoint,
                &call,
                request.caller.as_deref(),
                &request.block,
                &admit,
            )
            .map_err(contract_error)?,
        )
    } else {
        serde_json::to_value(
            observe_contract_code_with_admission(
                request.network,
                &endpoint,
                &request.target,
                &request.block,
                &admit,
            )
            .map_err(contract_error)?,
        )
    }
    .map_err(|_| IpcError::internal())?;
    check_cancelled(cancelled)?;
    project_observation(&result)
}
fn project_observation(result: &Value) -> Result<ToolOutcome, IpcError> {
    let value = json!({"observation":result,
        "trust":"Untrusted RPC observation and caller-declared ABI; no verified source/proxy/effect semantics, account control, signing authority or task postcondition.",
        "freshness":"Observation at the returned block only; latest and canonicality must be fetched again when needed."});
    if value["observation"]["outcome"]["status"] == "reverted" {
        // Retain exact revert/context evidence while preventing generic legacy
        // Completed-result accounting from crediting a successful value read.
        return Ok(ToolOutcome::Failed {
            error: IpcError {
                code: IpcErrorCode::Unsupported,
                message: value.to_string(),
                retryable: false,
            },
        });
    }
    Ok(ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: super::super::filesystem::ipc_json_value(&value)?,
        }],
    })
}
pub(super) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), IpcError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(IpcError {
            code: IpcErrorCode::Cancelled,
            message: "Contract observation cancelled".into(),
            retryable: false,
        });
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_declared_read_retains_all_raw_and_decoded_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        use chain_evm::contract::{
            ContractCodeObservation, ContractReadObservation, ContractReadOutcome,
            DeclaredContractCall,
        };
        use std::fmt::Write as _;
        let types = vec!["string".to_owned(); 7];
        let arguments = vec![json!("x".repeat(1024)); 7];
        let call = DeclaredContractCall::new(
            "0x1111111111111111111111111111111111111111",
            &format!("f({})", types.join(",")),
            &arguments,
            &types,
        )?;
        let mut data = "0x".to_owned();
        for index in 0..7 {
            write!(data, "{:064x}", 7 * 32 + index * (32 + 1024))?;
        }
        for _ in 0..7 {
            write!(data, "{:064x}{}", 1024, "00".repeat(1024))?;
        }
        let values = call.decode_result(&data)?;
        assert_eq!(values, vec![json!("\0".repeat(1024)); 7]);
        let observation = ContractReadObservation {
            context: ContractCodeObservation {
                chain_id: 84532,
                target: call.target().to_owned(),
                block_number: u64::MAX.to_string(),
                block_hash: format!("0x{}", "a".repeat(64)),
                runtime_code_hash: format!("0x{}", "b".repeat(64)),
                runtime_code_bytes: 1764,
                source_qualified: false,
                writes_available: false,
                canonical_at_observation: true,
            },
            caller: "0x2222222222222222222222222222222222222222".to_owned(),
            function: format!("f({})", types.join(",")),
            arguments: arguments.clone(),
            calldata: call.calldata().to_owned(),
            output_types: types,
            value_wei: "0".to_owned(),
            gas_limit: 500_000,
            outcome: ContractReadOutcome::Returned {
                values,
                data: data.clone(),
            },
        };
        let value = serde_json::to_value(observation)?;
        let projected = project_observation(&value).map_err(|error| error.message)?;
        // Both independently bounded ABI buffers and their decoded strings are
        // retained; the scheduler's 64KiB reservation is not a truncation limit.
        assert!(serde_json::to_vec(&projected)?.len() > 64 * 1024);
        let ToolOutcome::Completed { content } = projected else {
            return Err("bounded read was not retained".into());
        };
        let [
            ToolResultPart::Json {
                value: projected_value,
            },
        ] = content.as_slice()
        else {
            return Err("missing exact observation projection".into());
        };
        let ipc_types::JsonValue::Object(fields) = projected_value else {
            return Err("missing exact observation object".into());
        };
        let expected =
            crate::chat_tools::filesystem::ipc_json_value(&value).map_err(|error| error.message)?;
        assert_eq!(fields.get("observation"), Some(&expected));
        assert!(
            matches!(fields.get("trust"), Some(ipc_types::JsonValue::String(text)) if text.starts_with("Untrusted"))
        );
        Ok(())
    }
    #[test]
    fn reverted_read_retains_context_without_successful_value_credit() -> Result<(), IpcError> {
        let result = project_observation(
            &json!({"context":{"blockHash":"block"},"outcome":{"status":"reverted","code":3,"data":"0x1234"}}),
        )?;
        let ToolOutcome::Failed { error } = result else {
            return Err(IpcError::internal());
        };
        assert!(error.message.contains("blockHash"));
        assert!(error.message.contains("0x1234"));
        assert!(!error.retryable);
        Ok(())
    }
}
