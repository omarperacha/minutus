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
    if request.include_source && !super::source::available() {
        return Err(IpcError::permission_denied(
            "Packaged local source reproduction is unavailable; plain runtime inspection remains available",
        ));
    }
    let endpoint = crate::evm_rpc_settings::resolved(request.network)?;
    check_cancelled(cancelled)?;
    if !super::review::review(app, &request, &endpoint)? {
        return Err(IpcError::permission_denied(
            "The unsigned contract observation was not approved; do not retry automatically",
        ));
    }
    check_cancelled(cancelled)?;
    if request.include_source {
        return super::source::execute(app, &request, &endpoint, cancelled);
    }
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
    let mut observation = result.clone();
    // Only the host-owned code context carries these legacy flags. Decoded
    // values, arguments and revert data must remain byte/value-exact.
    let context = if observation.get("context").is_some() {
        &mut observation["context"]
    } else {
        &mut observation
    };
    let context = context.as_object_mut().ok_or_else(IpcError::internal)?;
    context.remove("sourceQualified");
    context.remove("writesAvailable");
    let value = json!({"observation":observation,
        "sourceQualification":{"status":"notRequested"},
        "effectsQualified":false,"writeAuthorityGranted":false,
        "trust":"Untrusted RPC observation and caller-declared ABI. This operation did not look up or reproduce source; it does not establish that source is missing. It grants no source/proxy/effect qualification, account control, signing authority or task postcondition.",
        "nextStep":"When source or a reproduced ABI/layout is needed, request contract.inspect with includeSource=true under its separate native outbound consent. Determine workflow availability from the host-advertised tools, never from this observation. Source inspection and workflows can still reject unavailable compiler/provider/proof inputs or unsupported source/effects; this observation authorises no write.",
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

    fn projected_json(result: &Value) -> Result<Value, Box<dyn std::error::Error>> {
        let ToolOutcome::Completed { content } =
            project_observation(result).map_err(|error| error.message)?
        else {
            return Err("expected completed observation".into());
        };
        let [ToolResultPart::Json { value }] = content.as_slice() else {
            return Err("missing observation JSON".into());
        };
        Ok(serde_json::to_value(value)?)
    }

    #[test]
    fn plain_inspection_distinguishes_unrequested_source_from_missing_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = json!({"chainId":11_155_111,"target":"contract","blockHash":"block",
            "runtimeCodeBytes":336,"sourceQualified":false,"writesAvailable":false});
        let projected = projected_json(&data)?;
        assert_eq!(projected["sourceQualification"]["status"], "notRequested");
        assert_eq!(projected["effectsQualified"], false);
        assert_eq!(projected["writeAuthorityGranted"], false);
        assert_eq!(
            projected["observation"],
            json!({"chainId":11_155_111.0,
            "target":"contract","blockHash":"block","runtimeCodeBytes":336.0})
        );
        assert!(projected.get("writesAvailable").is_none());
        assert!(
            projected["nextStep"]
                .as_str()
                .is_some_and(|text| text.contains("includeSource=true")
                    && text.contains("host-advertised tools"))
        );
        Ok(())
    }

    #[test]
    fn read_projection_preserves_matching_keys_in_untrusted_arguments_and_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let untrusted = json!({"sourceQualified":true,"writesAvailable":true,
            "sourceQualification":{"status":"untrusted"},"writeAuthorityGranted":true});
        let data = json!({"context":{"blockHash":"block","runtimeCodeBytes":336,
                "sourceQualified":false,"writesAvailable":false},
            "arguments":[untrusted.clone()],
            "outcome":{"status":"returned","data":"0x1234",
                "values":[untrusted,"writesAvailable sourceQualified"]}});
        let projected = projected_json(&data)?;
        assert_eq!(projected["sourceQualification"]["status"], "notRequested");
        assert_eq!(projected["writeAuthorityGranted"], false);
        assert_eq!(
            projected["observation"]["context"],
            json!({"blockHash":"block","runtimeCodeBytes":336.0})
        );
        assert_eq!(projected["observation"]["arguments"], data["arguments"]);
        assert_eq!(projected["observation"]["outcome"], data["outcome"]);
        Ok(())
    }
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
        let mut expected = value;
        let context = expected["context"]
            .as_object_mut()
            .ok_or("missing code context")?;
        context.remove("sourceQualified");
        context.remove("writesAvailable");
        let expected = crate::chat_tools::filesystem::ipc_json_value(&expected)
            .map_err(|error| error.message)?;
        assert_eq!(fields.get("observation"), Some(&expected));
        assert!(
            matches!(fields.get("trust"), Some(ipc_types::JsonValue::String(text)) if text.starts_with("Untrusted"))
        );
        Ok(())
    }
    #[test]
    fn reverted_read_retains_context_without_successful_value_credit() -> Result<(), IpcError> {
        let result = project_observation(
            &json!({"context":{"blockHash":"block","sourceQualified":false,"writesAvailable":false},"outcome":{"status":"reverted","code":3,"data":"0x1234"}}),
        )?;
        let ToolOutcome::Failed { error } = result else {
            return Err(IpcError::internal());
        };
        assert!(error.message.contains("blockHash"));
        assert!(error.message.contains("0x1234"));
        assert!(!error.retryable);
        let data: Value = serde_json::from_str(&error.message).map_err(|_| IpcError::internal())?;
        assert_eq!(data["sourceQualification"]["status"], "notRequested");
        assert_eq!(data["observation"]["outcome"]["status"], "reverted");
        assert_eq!(data["observation"]["context"], json!({"blockHash":"block"}));
        Ok(())
    }
}
