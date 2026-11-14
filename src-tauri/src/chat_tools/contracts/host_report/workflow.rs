use super::{MAX_REPORT_BYTES, display};
use crate::chat_tools::ChatToolExchange;
use chain_evm::{PublicEvmAddress, TransactionHash};
use ipc_types::{ToolName, ToolOutcome, ToolResultPart};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) struct HostContractReport {
    network: String,
    targets: Vec<String>,
    pub(super) hashes: BTreeSet<String>,
    pub(super) workflow_verified: bool,
    pub(super) inclusion_verified: bool,
    rendered: String,
}
impl HostContractReport {
    pub(crate) fn network(&self) -> &str {
        &self.network
    }
    pub(crate) fn targets(&self) -> &[String] {
        &self.targets
    }
    pub(crate) fn render(&self) -> &str {
        &self.rendered
    }
}
pub(crate) fn from_exchange(exchange: &ChatToolExchange) -> Option<HostContractReport> {
    if exchange.request.name != ToolName::ContractRequestWorkflow
        || exchange.request.call_id != exchange.result.call_id
    {
        return None;
    }
    let request = serde_json::to_value(&exchange.request.arguments).ok()?;
    if request["contractVersion"] != json!(2.0) {
        return None;
    }
    let network = request["network"].as_str()?.to_owned();
    network_chain(&network)?;
    let contracts = request["contracts"].as_array()?;
    if contracts.is_empty() || contracts.len() > 16 {
        return None;
    }
    let targets = contracts
        .iter()
        .map(|contract| {
            PublicEvmAddress::new(contract["address"].as_str()?)
                .ok()
                .map(|address| address.as_str().to_owned())
        })
        .collect::<Option<Vec<_>>>()?;
    let value = result_json(&exchange.result.outcome)?;
    let state = value["state"].as_str()?;
    if !matches!(
        state,
        "verified"
            | "executionConfirmed"
            | "submittedVerificationUnavailable"
            | "stopped"
            | "deniedOrDismissed"
            | "scopeDenied"
            | "alreadyAttempted"
    ) {
        return None;
    }
    if value["lifecycle"]["observationScope"] != "thisInvocationOnly"
        || value["retryAutomatically"] != false
        || value
            .get("automaticCleanup")
            .is_some_and(|flag| flag != false)
        || value
            .get("currentWalletState")
            .is_some_and(|state| state != "notObserved")
    {
        return None;
    }
    let summary = value["hostOutcomeSummary"].as_str()?;
    if summary.is_empty() {
        return None;
    }
    let steps = value["lifecycle"]["observations"]["steps"].as_array()?;
    if steps.len() > 16 {
        return None;
    }
    let hashes = steps
        .iter()
        .filter_map(|step| step["transactionHash"].as_str())
        .map(|hash| {
            TransactionHash::new(hash)
                .ok()
                .map(|hash| hash.as_str().to_owned())
        })
        .collect::<Option<BTreeSet<_>>>()?;
    let evidence = material(&value, steps)?;
    let mut declared_steps = request["steps"].as_array()?.clone();
    for step in &mut declared_steps {
        // Presentation prose is not part of the immutable declaration and must
        // not appear beside host facts as a claim of safety or completion.
        step.as_object_mut()?.remove("summary");
    }
    let scope = json!({"network":network,"contracts":request["contracts"],"steps":declared_steps,"goals":request["goals"]});
    let rendered = format!(
        "Contract workflow: {state}.\n\n{summary}\n\nRequested scope and exact calls:\n```json\n{}\n```\n\nRecorded transaction and outcome facts:\n```json\n{}\n```",
        display::json(&scope),
        display::json(&evidence)
    );
    (rendered.len() <= MAX_REPORT_BYTES).then_some(HostContractReport {
        network,
        targets,
        hashes,
        workflow_verified: state == "verified",
        inclusion_verified: steps
            .iter()
            .any(|step| step["postSubmission"]["inclusionProof"] == "verified"),
        rendered,
    })
}
fn material(value: &Value, steps: &[Value]) -> Option<Value> {
    let mut facts = serde_json::Map::new();
    for (key, item) in value.as_object()? {
        match key.as_str() {
            "state"
            | "message"
            | "verificationError"
            | "completedSteps"
            | "partialFacts"
            | "historicalCompletions"
            | "verifiedCurrentGoals"
            | "residualState"
            | "qualification"
            | "scopeCommitment"
            | "goalsDeclared"
            | "goalStatus" => {
                facts.insert(key.clone(), item.clone());
            }
            "hostOutcomeSummary"
            | "lifecycle"
            | "latestObservedStage"
            | "currentWalletState"
            | "retryAutomatically"
            | "automaticCleanup" => {}
            _ => return None,
        }
    }
    facts.insert("transactions".into(),json!(steps.iter().map(|step|json!({"stepIndex":step["stepIndex"],"transactionHash":step["transactionHash"],"submission":step["submission"],"postSubmission":step.get("postSubmission").cloned().unwrap_or_else(||json!("notRecorded"))})).collect::<Vec<_>>()));
    Some(Value::Object(facts))
}
pub(super) fn result_json(outcome: &ToolOutcome) -> Option<Value> {
    match outcome {
        ToolOutcome::Completed { content } => {
            let [ToolResultPart::Json { value }] = content.as_slice() else {
                return None;
            };
            serde_json::to_value(value).ok()
        }
        ToolOutcome::Failed { error } if !error.retryable => {
            let value: Value = serde_json::from_str(&error.message).ok()?;
            if value.get("state").is_some() {
                Some(value)
            } else if value.as_object()?.len() == 2 && value["retryable"] == false {
                serde_json::from_str(value["error"].as_str()?).ok()
            } else {
                None
            }
        }
        ToolOutcome::Failed { .. } => None,
    }
}
pub(super) fn network_chain(network: &str) -> Option<u64> {
    Some(match network {
        "ethereumMainnet" => 1,
        "ethereumSepolia" => 11_155_111,
        "baseMainnet" => 8453,
        "baseSepolia" => 84_532,
        _ => return None,
    })
}
