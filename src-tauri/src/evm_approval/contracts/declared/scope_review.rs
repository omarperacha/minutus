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
        "RPC provider origin: {}\nNetwork: {}\nSelected account: {}\nPublic source service: https://sourcify.dev\nOfficial compiler downloads: https://binaries.soliditylang.org\nContracts: {}\nExact ordered calls and declared conditions: {}\nFinal goals: {}\nScope commitment: {}\n\nThe source service sees public chain/address lookups. Official compiler downloads reveal compiler versions. Neither service receives transaction calldata, signatures or wallet secrets. The RPC provider sees addresses, queried state keys and public transaction hashes; these queries may reveal intent. Source is compiled locally with an independently pinned official compiler, then matched against proof-checked current runtime. Execution remains local. New transaction calldata and signatures reach the RPC provider only after each exact native approval, fresh password and signed validation.\n\nThis permission covers only these fixed addresses, calls, predicates and bounded proof/status refreshes during this request. Source/ABI and predicate feasibility are checked before the first write; later execution can still fail. Every write requires its own review and password. Earlier effects may remain if a later step stops. No automatic cleanup, replacement or retry. This outbound permission authorises no transaction.",
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
