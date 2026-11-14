//! Safe historical projection, ordered by actual observations rather than step height.
use super::{context::Context, observation::CompletedStep};
use crate::evm_approval::contracts::reporting::ObservedFacts;
use chain_evm::contract::generic::VerifiedGenericConditions;
use ipc_types::IpcError;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn conditions(proof: &VerifiedGenericConditions) -> Value {
    json!({"network":proof.network().descriptor().caip2_id(),"stateBlockNumber":proof.block_number().to_string(),"stateBlockHash":proof.block_hash(),"sourceCommitments":proof.source_commitments(),"proofCommitment":proof.proof_commitment(),"observations":proof.observations()})
}
pub(super) fn completions(completed: &[CompletedStep]) -> Value {
    Value::Array(completed.iter().map(|step|{
    let proof=step.completion.proof();json!({"stepIndex":step.step.index(),"transactionHash":proof.transaction_hash(),"candidateDigest":proof.candidate_digest(),"receiptBlockNumber":proof.receipt_block_number().to_string(),"receiptBlockHash":proof.receipt_block_hash(),"inclusionCommitment":proof.inclusion_commitment(),"completionCoverage":if proof.conditions().observations().is_empty(){"executionOnly"}else{"declaredStateConditions"},"observedCompletion":conditions(proof.conditions()),"temporalMeaning":"historical observation; later steps may consume or overwrite these values; no transaction-local causation claimed"})
}).collect())
}
pub(super) fn partial(context: &Context, completed: &[CompletedStep]) -> Value {
    let current = match context.observation_snapshots() {
        Ok(snapshots) => latest(snapshots),
        Err(_) => json!({"lastObservedFields":[],"observationHistory":"unavailable"}),
    };
    json!({"historicalCompletions":completions(completed),"residualState":current,"currentResidualAuthority":"unknown after the listed observations; generic field names do not classify token or spending authority","retryAutomatically":false,"automaticCleanup":false})
}
fn latest(snapshots: Vec<ObservedFacts>) -> Value {
    let mut fields = BTreeMap::<String, (u64, Value)>::new();
    for snapshot in snapshots {
        let Some(observations) = snapshot.facts["observations"].as_array() else {
            continue;
        };
        for observation in observations {
            let key = json!([
                snapshot.facts["network"],
                observation["condition"]["target"],
                observation["condition"]["path"]
            ])
            .to_string();
            if fields
                .get(&key)
                .is_none_or(|(sequence, _)| *sequence < snapshot.sequence)
            {
                fields.insert(key,(snapshot.sequence,json!({"network":snapshot.facts["network"],"stateBlockNumber":snapshot.facts["stateBlockNumber"],"stateBlockHash":snapshot.facts["stateBlockHash"],"observation":observation,"currentValueAfterObservation":"unknown"})));
            }
        }
    }
    json!({"lastObservedFields":fields.into_values().map(|(_,value)|value).collect::<Vec<_>>()})
}
pub(super) fn capture(proof: &VerifiedGenericConditions) -> Result<ObservedFacts, IpcError> {
    crate::evm_approval::contracts::reporting::capture(conditions(proof))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chronological_refresh_survives_lower_height_and_retained_older_step() {
        let facts = |value: &str, block: &str| json!({"network":"eip155:1","stateBlockNumber":block,"stateBlockHash":block,"observations":[{"condition":{"target":"app","path":["remaining"]},"value":value}]});
        let result = latest(vec![
            ObservedFacts {
                sequence: 3,
                facts: facts("7", "90"),
            },
            ObservedFacts {
                sequence: 2,
                facts: facts("0", "100"),
            },
        ]);
        assert_eq!(result["lastObservedFields"][0]["observation"]["value"], "7");
        assert_eq!(result["lastObservedFields"][0]["stateBlockNumber"], "90");
    }
}
