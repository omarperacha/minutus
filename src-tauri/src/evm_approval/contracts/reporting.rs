//! Non-authority projection of already verified host observations. No formatter
//! creates a proof, credits a journal step or performs an additional RPC read.
use ipc_types::IpcError;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_OBSERVATION: AtomicU64 = AtomicU64::new(1);
#[derive(Clone)]
pub(super) struct ObservedFacts {
    pub(super) sequence: u64,
    pub(super) facts: Value,
}
pub(super) fn capture(facts: Value) -> Result<ObservedFacts, IpcError> {
    let sequence = NEXT_OBSERVATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| IpcError::internal())?;
    Ok(ObservedFacts { sequence, facts })
}

pub(super) struct ProofReport {
    pub(super) postconditions: Value,
    pub(super) residual: Value,
}
pub(super) fn complete(facts: Vec<ObservedFacts>) -> Result<ProofReport, IpcError> {
    if facts.is_empty()
        || facts.iter().any(|observed| {
            let fact = &observed.facts;
            [
                "transactionHash",
                "network",
                "receiptBlockNumber",
                "receiptBlockHash",
                "stateBlockNumber",
                "stateBlockHash",
            ]
            .iter()
            .any(|field| fact[*field].as_str().is_none())
                || fact["predicates"].as_array().is_none_or(|predicates| {
                    predicates.is_empty()
                        || predicates
                            .iter()
                            .any(|predicate| predicate_key(fact, predicate).is_none())
                })
        })
    {
        return Err(IpcError::invalid_request(
            "Verified goal observation context is unavailable; receipt status alone does not establish workflow postconditions",
        ));
    }
    Ok(project(facts))
}
pub(super) fn partial(facts: Vec<ObservedFacts>) -> Value {
    let report = project(facts);
    let mut residual = report.residual;
    residual["lastVerifiedPredicates"] = report.postconditions;
    residual
}
fn project(mut facts: Vec<ObservedFacts>) -> ProofReport {
    // Snapshot sequence records actual refresh order. Step order is only the
    // report layout; neither a later step nor a higher block is necessarily the
    // newest observation after a partial refresh or reorganisation.
    let mut last = BTreeMap::new();
    for (index, observed) in facts.iter().enumerate() {
        let fact = &observed.facts;
        if let Some(predicates) = fact["predicates"].as_array() {
            for (position, predicate) in predicates.iter().enumerate() {
                if let Some(key) = predicate_key(fact, predicate) {
                    let candidate = (observed.sequence, index, position);
                    last.entry(key)
                        .and_modify(|current: &mut (u64, usize, usize)| {
                            if candidate.0 > current.0
                                || (candidate.0 == current.0
                                    && candidate.1 == current.1
                                    && candidate.2 > current.2)
                            {
                                *current = candidate;
                            }
                        })
                        .or_insert(candidate);
                }
            }
        }
    }
    let step_indices = facts
        .iter()
        .map(|observed| observed.facts["stepIndex"].clone())
        .collect::<Vec<_>>();
    let mut allowances = Vec::new();
    for (index, observed) in facts.iter_mut().enumerate() {
        let fact = &mut observed.facts;
        let snapshot = fact.clone();
        if let Some(predicates) = fact["predicates"].as_array_mut() {
            for (position, predicate) in predicates.iter_mut().enumerate() {
                let latest = predicate_key(&snapshot, predicate).and_then(|key| last.get(&key));
                let is_last = latest == Some(&(observed.sequence, index, position));
                predicate["observationStatus"] = json!(if is_last {
                    "lastObservedAtListedBlock"
                } else {
                    "historicalSupersededByLaterObservation"
                });
                predicate["supersededByStepIndex"] = latest
                    .filter(|_| !is_last)
                    .map_or(Value::Null, |(_, later, _)| step_indices[*later].clone());
                if is_last && predicate["kind"] == "allowance" {
                    allowances.push(json!({"network":snapshot["network"],"token":predicate["token"],"owner":predicate["owner"],"spender":predicate["spender"],"value":predicate["value"],"asOfStateBlockNumber":snapshot["stateBlockNumber"],"asOfStateBlockHash":snapshot["stateBlockHash"],"transactionHash":snapshot["transactionHash"],"stepIndex":snapshot["stepIndex"]}));
                }
            }
        }
        fact["temporalScope"] = json!(
            "Historical verified predicates at the listed state block; current state after observation is unknown"
        );
    }
    ProofReport {
        postconditions: json!(
            facts
                .into_iter()
                .map(|observed| observed.facts)
                .collect::<Vec<_>>()
        ),
        residual: json!({"currentResidualAllowance":"unknown after the listed observations","lastObservedRemainingAllowances":allowances,"coverage":"Only the listed allowance keys were observed; unobserved allowances remain unknown","automaticCleanup":false,"retryAutomatically":false}),
    }
}
fn predicate_key(fact: &Value, predicate: &Value) -> Option<Vec<String>> {
    let fields: &[&str] = match predicate["kind"].as_str()? {
        "allowance" => &["token", "owner", "spender"],
        "tokenBalance" => &["token", "account"],
        _ => return None,
    };
    predicate["value"].as_str()?;
    let mut key = vec![
        fact["network"].as_str()?.into(),
        predicate["kind"].as_str()?.into(),
    ];
    for field in fields {
        key.push(predicate[*field].as_str()?.into());
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation(step: u8, block: &str, amount: &str) -> ObservedFacts {
        ObservedFacts {
            sequence: u64::from(step) + 1,
            facts: json!({"stepIndex":step,"transactionHash":format!("tx-{step}"),"network":"eip155:11155111","receiptBlockNumber":block,"receiptBlockHash":format!("receipt-{step}"),"stateBlockNumber":block,"stateBlockHash":format!("state-{step}"),"predicates":[{"kind":"allowance","token":"token","owner":"owner","spender":"spender","value":amount}]}),
        }
    }
    #[test]
    fn completed_pull_reports_zero_allowance_and_supersedes_the_approval_observation()
    -> Result<(), IpcError> {
        let report = complete(vec![observation(0, "45", "7"), observation(1, "44", "0")])?;
        assert_eq!(
            report.postconditions[0]["predicates"][0]["observationStatus"],
            "historicalSupersededByLaterObservation"
        );
        assert_eq!(
            report.postconditions[0]["predicates"][0]["supersededByStepIndex"],
            1
        );
        assert_eq!(report.postconditions[0]["receiptBlockNumber"], "45");
        assert_eq!(report.postconditions[1]["predicates"][0]["value"], "0");
        assert_eq!(
            report.residual["lastObservedRemainingAllowances"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            report.residual["lastObservedRemainingAllowances"][0]["value"],
            "0"
        );
        assert_eq!(
            report.residual["lastObservedRemainingAllowances"][0]["asOfStateBlockNumber"],
            "44"
        );
        assert_eq!(
            report.residual["currentResidualAllowance"],
            "unknown after the listed observations"
        );
        assert_eq!(report.residual["automaticCleanup"], false);
        assert_eq!(report.residual["retryAutomatically"], false);
        Ok(())
    }
    #[test]
    fn partial_refresh_uses_new_step_zero_snapshot_over_retained_old_step_one() {
        let mut refreshed = observation(0, "43", "7");
        refreshed.sequence = 3;
        let retained = observation(1, "45", "0");
        let report = partial(vec![refreshed, retained]);
        assert_eq!(report["lastObservedRemainingAllowances"][0]["value"], "7");
        assert_eq!(report["lastObservedRemainingAllowances"][0]["stepIndex"], 0);
        assert_eq!(
            report["lastObservedRemainingAllowances"][0]["asOfStateBlockNumber"],
            "43"
        );
        assert_eq!(
            report["lastVerifiedPredicates"][1]["predicates"][0]["observationStatus"],
            "historicalSupersededByLaterObservation"
        );
        assert_eq!(
            report["lastVerifiedPredicates"][1]["predicates"][0]["supersededByStepIndex"],
            0
        );
        assert_eq!(report["lastVerifiedPredicates"][0]["stepIndex"], 0);
        assert!(
            report["lastVerifiedPredicates"][0]
                .get("sequence")
                .is_none()
        );
    }
    #[test]
    fn receipt_without_goal_observations_cannot_produce_a_completed_proof_report() {
        let mut receipt = observation(0, "44", "7");
        receipt.facts["predicates"] = json!([]);
        assert!(complete(vec![receipt]).is_err());
        assert!(complete(vec![]).is_err());
        let partial = partial(vec![observation(0, "44", "7")]);
        assert_eq!(partial["lastObservedRemainingAllowances"][0]["value"], "7");
        assert!(
            partial["currentResidualAllowance"]
                .as_str()
                .is_some_and(|text| text.starts_with("unknown"))
        );
    }

    #[test]
    fn sixteen_goal_report_preserves_every_predicate_and_residual_within_reservation()
    -> Result<(), IpcError> {
        // Synthetic already-observed facts exercise projection size only; this
        // formatter does not create receipt, source or state-proof authority.
        let hash = format!("0x{}", "a".repeat(64));
        let address = |index: u8| format!("0x{index:040x}");
        let amount =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        let observations = (0_u8..16)
            .map(|step| ObservedFacts {
                sequence: u64::from(step) + 1,
                facts: json!({"stepIndex":step,"transactionHash":hash,"network":"eip155:11155111","receiptBlockNumber":u64::MAX.to_string(),"receiptBlockHash":hash,"stateBlockNumber":u64::MAX.to_string(),"stateBlockHash":hash,"receiptCommitment":hash,"proofCommitment":hash,"profileId":"oz570-solc0838-cancun-erc20-v1","profileCommitment":hash,"predicates":[
                    {"kind":"allowance","token":address(step+1),"owner":address(40),"spender":address(41),"value":amount},
                    {"kind":"tokenBalance","token":address(step+1),"account":address(40),"value":amount},
                    {"kind":"tokenBalance","token":address(step+1),"account":address(41),"value":amount}
                ]}),
            })
            .collect::<Vec<_>>();
        let report = complete(observations)?;
        assert_eq!(report.postconditions.as_array().map(Vec::len), Some(16));
        assert!(
            report
                .postconditions
                .as_array()
                .is_some_and(|facts| facts.iter().all(|fact| fact["predicates"]
                    .as_array()
                    .is_some_and(|predicates| predicates.len() == 3)))
        );
        assert_eq!(
            report.residual["lastObservedRemainingAllowances"]
                .as_array()
                .map(Vec::len),
            Some(16)
        );
        let projection = json!({"state":"verified","transactions":vec![hash.clone();16],"scopeCommitment":hash,"verifiedPostconditions":report.postconditions,"residualAuthority":report.residual,"retryAutomatically":false});
        assert!(
            serde_json::to_vec(&projection)
                .map_err(|_| IpcError::internal())?
                .len()
                < 64 * 1024
        );
        Ok(())
    }
}
