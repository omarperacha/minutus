//! Before-write capacity admission for complete historical and residual reports.
//! Synthetic values here are size bounds only; they never become proof facts.
use app_core::{ContractWorkflowDeclaration, StoredWorkflowCondition, StoredWorkflowRead};
use chain_evm::contract::generic::{GenericCondition, GenericError};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const REPORT_LIMIT: usize = 64 * 1024;
const HASH: &str = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const HEIGHT: &str = "18446744073709551615";

pub(super) fn admit(
    declaration: &ContractWorkflowDeclaration,
    types: &[(GenericCondition, String)],
) -> Result<(), GenericError> {
    if maximum_bytes(declaration, types)? > REPORT_LIMIT {
        return Err(GenericError::Limit);
    }
    Ok(())
}

fn maximum_bytes(
    declaration: &ContractWorkflowDeclaration,
    types: &[(GenericCondition, String)],
) -> Result<usize, GenericError> {
    let types = types
        .iter()
        .map(|(condition, ty)| Ok((key(condition)?, ty.as_str())))
        .collect::<Result<BTreeMap<_, _>, GenericError>>()?;
    let mut historical = Vec::new();
    let mut residual = Vec::new();
    for (index, step) in declaration.intent().steps.iter().enumerate() {
        let completion = observations(declaration, &step.completion, &types)?;
        historical.push(json!({"stepIndex":index,"transactionHash":HASH,
            "candidateDigest":HASH,"receiptBlockNumber":HEIGHT,"receiptBlockHash":HASH,
            "inclusionCommitment":HASH,"completionCoverage":"declaredStateConditions",
            "observedCompletion":facts(&completion),
            "temporalMeaning":"historical observation; later steps may consume or overwrite these values; no transaction-local causation claimed"}));
        residual.extend(completion);
        residual.extend(observations(declaration, &step.prerequisites, &types)?);
    }
    let goals = observations(declaration, &declaration.intent().goals, &types)?;
    residual.extend(goals.clone());
    // Repeated keys are intentionally not deduplicated: this bounds every
    // possible chronology, including earlier prerequisite-only observations.
    let residual = residual
        .into_iter()
        .map(|observation| {
            json!({"network":declaration.intent().network,
            "stateBlockNumber":HEIGHT,"stateBlockHash":HASH,"observation":observation,
            "currentValueAfterObservation":"unknown"})
        })
        .collect::<Vec<_>>();
    let report = json!({"state":"executionConfirmed","scopeCommitment":HASH,
        "completedSteps":declaration.intent().steps.len(),"goalsDeclared":true,
        "goalStatus":"unsatisfied at proof-checked observation",
        "partialFacts":{"historicalCompletions":historical,
            "residualState":{"lastObservedFields":residual},
            "currentResidualAuthority":"unknown after the listed observations; generic field names do not classify token or spending authority",
            "retryAutomatically":false,"automaticCleanup":false},
        // Actual final evidence has fewer fields than a condition-proof cohort
        // plus these declarations. Charge both to cover every result variant.
        "verifiedCurrentGoals":{"kind":"goals","scopeCommitment":HASH,
            "preparationCommitment":null,"receipt":null,"stateBlockNumber":HEIGHT,
            "stateBlockHash":HASH,"proofCommitment":HASH,"results":goals,
            "declaredConditions":declaration.intent().goals},
        "qualification":"Exact source/ABI, complete structural execution effects, canonical successful inclusions and separately declared state conditions were independently checked. Source labels are not economic semantics; observations are historical and do not establish finality or later current state.",
        "retryAutomatically":false,"automaticCleanup":false});
    // Non-verified results travel inside an IpcError message. Include this second
    // JSON quoting layer, not just the bare successful-result byte count. The
    // margin covers fixed error/status wrappers and their bounded diagnostics.
    serde_json::to_vec(&json!({"code":"unsupported","retryable":false,
        "message":report.to_string()}))
    .map(|bytes| bytes.len().saturating_add(4096))
    .map_err(|_| GenericError::Limit)
}

fn observations(
    declaration: &ContractWorkflowDeclaration,
    conditions: &[StoredWorkflowCondition],
    types: &BTreeMap<String, &str>,
) -> Result<Vec<Value>, GenericError> {
    conditions
        .iter()
        .map(|condition| {
            let target = declaration
                .intent()
                .contracts
                .iter()
                .find(|contract| contract.id == condition.contract)
                .ok_or(GenericError::InvalidAbi)?;
            let StoredWorkflowRead::SourceField { path } = &condition.read else {
                return Err(GenericError::InvalidAbi);
            };
            let condition = json!({"target":target.address,"path":path,
                "comparison":condition.comparison,"expected":condition.expected});
            let ty = types
                .get(&condition.to_string())
                .ok_or(GenericError::InvalidAbi)?;
            Ok(json!({"condition":condition,"solidityType":ty,"value":maximum_value(ty)?}))
        })
        .collect()
}

fn facts(observations: &[Value]) -> Value {
    json!({"network":"eip155:11155111","stateBlockNumber":HEIGHT,"stateBlockHash":HASH,
        "sourceCommitments":vec!["f".repeat(64);32],"proofCommitment":HASH,
        "observations":observations})
}
fn key(condition: &GenericCondition) -> Result<String, GenericError> {
    serde_json::to_value(condition)
        .map(|value| value.to_string())
        .map_err(|_| GenericError::Limit)
}
fn maximum_value(ty: &str) -> Result<Value, GenericError> {
    Ok(match ty {
        "string" => json!("\u{1}".repeat(1024)),
        "bytes" => json!(format!("0x{}", "f".repeat(2048))),
        "address" => json!(format!("0x{}", "f".repeat(40))),
        "bool" => json!(false),
        _ if ty.starts_with("uint") => json!("9".repeat(78)),
        _ if ty.starts_with("int") => json!(format!("-{}", "9".repeat(78))),
        _ => {
            let bytes = ty
                .strip_prefix("bytes")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|bytes| (1..=32).contains(bytes))
                .ok_or(GenericError::InvalidAbi)?;
            json!(format!("0x{}", "f".repeat(bytes * 2)))
        }
    })
}

#[cfg(test)]
mod tests;
