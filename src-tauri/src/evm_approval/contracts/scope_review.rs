//! Native outbound consent freezes the endpoint and every semantic call first.
use super::preparation::ContractWorkflowRequest;
use crate::evm_approval::{execution::show_native_review, presentation::ApprovalDisplay};
use app_core::ContractWorkflowScope;
use chain_evm::{EvmRpcUrl, contract::DeclaredContractCall};
use ipc_types::{EvmApprovalDecision, IpcError};
pub(super) async fn review(
    app: &tauri::AppHandle,
    scope: &ContractWorkflowScope,
    endpoint: &EvmRpcUrl,
    request: &ContractWorkflowRequest,
) -> Result<EvmApprovalDecision, IpcError> {
    show_native_review(app, display(scope, endpoint, request)?).await
}
fn display(
    scope: &ContractWorkflowScope,
    endpoint: &EvmRpcUrl,
    request: &ContractWorkflowRequest,
) -> Result<ApprovalDisplay, IpcError> {
    let calls=scope.intent().steps.iter().zip(&request.steps).map(|(step,source)|{
   let call=DeclaredContractCall::new(&step.target,&step.operation,&step.arguments,&["bool".into()]).map_err(super::preparation::chain_error)?;
   Ok(serde_json::json!({"target":step.target,"function":step.operation,"arguments":step.arguments,"valueWei":step.value_wei,"calldata":call.calldata(),"deploymentTransaction":source.deployment_transaction}))
 }).collect::<Result<Vec<_>,IpcError>>()?;
    Ok(ApprovalDisplay {
        title: "Allow contract workflow data to leave this Mac?".into(),
        body: format!(
            "Provider origin: {}\nAccount: {}\nNetwork: {}\nExact ordered calls: {}\nScope commitment: {}\n\nBefore transaction approval, the provider sees account/target addresses, queried storage keys and public transaction hashes for source/state/receipt proofs; these queries may reveal intent. Execution remains local. The new transaction calldata and signature reach the provider only after that step's exact native approval, fresh password and signed validation. This permission covers only the displayed fixed ordered calls and bounded status/proof refreshes during this request. Every target must match the reviewed compiled token family before any write. Each write requires separate exact native approval and fresh password. This outbound consent authorises no transaction. A prior allowance may remain if a later step is denied, fails or is uncertain; no automatic cleanup or retry.",
            endpoint.public_origin(),
            scope.intent().account,
            scope.intent().network,
            crate::web_approval::review_json(&serde_json::Value::Array(calls)),
            scope.commitment()
        ),
        has_warnings: true,
        approval_allowed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outbound_scope_shows_exact_bytes_without_rpc_credentials()
    -> Result<(), Box<dyn std::error::Error>> {
        let request: ContractWorkflowRequest = serde_json::from_value(
            serde_json::json!({"network":"ethereumSepolia","steps":[{"target":"0x2222222222222222222222222222222222222222","function":"transfer(address,uint256)","arguments":["0x3333333333333333333333333333333333333333","7"],"valueWei":"0","deploymentTransaction":format!("0x{}","a".repeat(64))}]}),
        )?;
        let scope = request
            .scope("0x1111111111111111111111111111111111111111")
            .map_err(|error| error.message)?;
        let endpoint = EvmRpcUrl::new(
            "https://user:secret@provider.example/private-key?token=credential".into(),
        )?;
        let view = display(&scope, &endpoint, &request).map_err(|error| error.message)?;
        assert!(view.body.contains("https://provider.example"));
        assert!(view.body.contains("0xa9059cbb"));
        assert!(view.body.contains(&request.steps[0].deployment_transaction));
        for secret in ["user:secret", "private-key", "credential"] {
            assert!(!view.body.contains(secret));
        }
        assert!(view.body.contains("authorises no transaction"));
        Ok(())
    }
}
