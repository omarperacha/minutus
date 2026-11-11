//! Generic source acquisition is part of explicit native outbound consent.
use crate::evm_approval::{execution::show_native_review, presentation::ApprovalDisplay};
use app_core::ContractWorkflowDeclaration;
use chain_evm::{EvmRpcUrl, contract::DeclaredContractCall};
use ipc_types::{EvmApprovalDecision, IpcError};
use serde_json::{Value, json};
pub(super) async fn review(
    app: &tauri::AppHandle,
    declaration: &ContractWorkflowDeclaration,
    endpoint: &EvmRpcUrl,
) -> Result<EvmApprovalDecision, IpcError> {
    show_native_review(app, display(declaration, endpoint)?).await
}
fn display(
    declaration: &ContractWorkflowDeclaration,
    endpoint: &EvmRpcUrl,
) -> Result<ApprovalDisplay, IpcError> {
    let intent = declaration.intent();
    let calls=intent.steps.iter().map(|step|{
        let target=intent.contracts.iter().find(|contract|contract.id==step.contract).ok_or_else(IpcError::internal)?;
        let call=DeclaredContractCall::new(&target.address,&step.function,&step.arguments,&[]).map_err(|_|IpcError::invalid_request("Invalid declared workflow ABI"))?;
        Ok(json!({"step":step.id,"target":target.address,"function":step.function,"arguments":step.arguments,"calldata":call.calldata(),"valueWei":step.value_wei,"dependsOn":step.depends_on,"prerequisites":step.prerequisites,"completion":step.completion}))
    }).collect::<Result<Vec<_>,IpcError>>()?;
    let body = format!(
        "RPC provider origin: {}\nNetwork: {}\nSelected account: {}\nPublic source service: https://sourcify.dev\nOfficial compiler downloads: https://binaries.soliditylang.org\nContracts: {}\nExact ordered calls and declared conditions: {}\nFinal goals: {}\nScope commitment: {}\n\nThe source service sees public chain/address lookups for the listed business contracts and proof-derived code-only dependencies of exact minimal proxies. Official compiler downloads reveal compiler versions. Neither service receives transaction calldata, signatures or wallet secrets. The RPC provider sees addresses, queried state keys and public transaction hashes; these queries may reveal intent. Source is compiled locally with an independently pinned official compiler, then matched against proof-checked current runtime. Execution remains local. New transaction calldata and signatures reach the RPC provider only after each exact native approval, fresh password and signed validation.\n\nThis permission covers the listed fixed business addresses, exact ordered calls, predicates, proof-derived minimal-proxy code dependencies and bounded proof/status refreshes during this request. Every external business call target must be listed before any source or RPC acquisition; unknown targets stop the request. Code-only implementation discovery grants no permission to call that implementation as another business target. Source/ABI and predicate feasibility are checked before the first write; later execution can still fail. Every write requires its own review and password. Earlier effects may remain if a later step stops. No automatic cleanup, replacement or retry. This outbound permission authorises no transaction.",
        endpoint.public_origin(),
        intent.network,
        intent.account,
        crate::web_approval::review_json(
            &serde_json::to_value(&intent.contracts).map_err(|_| IpcError::internal())?
        ),
        crate::web_approval::review_json(&Value::Array(calls)),
        crate::web_approval::review_json(
            &serde_json::to_value(&intent.goals).map_err(|_| IpcError::internal())?
        ),
        declaration.commitment()
    );
    if body.len() > 64 * 1024 {
        return Err(IpcError::invalid_request(
            "Complete outbound workflow scope exceeds the native review limit",
        ));
    }
    Ok(ApprovalDisplay {
        title: "Allow generic contract workflow data to leave this Mac?".into(),
        body,
        has_warnings: true,
        approval_allowed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outbound_scope_distinguishes_business_targets_from_code_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let request: super::super::request::DeclaredWorkflowRequest =
            serde_json::from_value(json!({
                "contractVersion":2,"network":"ethereumSepolia",
                "contracts":[
                    {"id":"router","address":"0x2222222222222222222222222222222222222222"},
                    {"id":"callee","address":"0x3333333333333333333333333333333333333333"}
                ],
                "steps":[{"id":"route","contract":"router","function":"route(address,uint256)",
                    "arguments":["0x3333333333333333333333333333333333333333","7"],
                    "valueWei":"0","dependsOn":[],"prerequisites":[],"completion":[]}],
                "goals":[]
            }))?;
        let declaration = request
            .declaration("0x1111111111111111111111111111111111111111")
            .map_err(|error| error.message)?;
        let endpoint =
            EvmRpcUrl::new("https://user:secret@provider.example/private?token=credential".into())?;
        let view = display(&declaration, &endpoint).map_err(|error| error.message)?;
        for address in [
            "0x2222222222222222222222222222222222222222",
            "0x3333333333333333333333333333333333333333",
        ] {
            assert!(view.body.contains(address));
        }
        assert!(
            view.body
                .contains("Every external business call target must be listed")
        );
        assert!(view.body.contains("proof-derived code-only dependencies"));
        assert!(
            view.body
                .contains("grants no permission to call that implementation")
        );
        assert!(view.body.contains("authorises no transaction"));
        for secret in ["user:secret", "private?token", "credential"] {
            assert!(!view.body.contains(secret));
        }
        Ok(())
    }
}
