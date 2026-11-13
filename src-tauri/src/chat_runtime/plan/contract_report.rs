//! A final report may replace prose synthesis only for one cohesive contract task.
use super::{PlanActionClass, PlanContractVersion, PlanPolicy, SemanticPlanContract};
use std::collections::BTreeSet;

impl PlanPolicy {
    pub(in crate::chat_runtime) fn contract_report_ready(&self) -> bool {
        self.final_response_ready()
            && self.contract_version == Some(PlanContractVersion::SemanticExecutionV6)
            && self.semantic_contract.as_ref().is_some_and(cohesive)
    }
}

fn cohesive(contract: &SemanticPlanContract) -> bool {
    let writes = contract
        .steps
        .iter()
        .filter(|step| step.action_class == PlanActionClass::ExternalWrite)
        .collect::<Vec<_>>();
    let [write] = writes.as_slice() else {
        return false;
    };
    if write.preferred_tools.as_slice() != ["contract.request_workflow"] {
        return false;
    }
    contract.steps.iter().all(|step| {
        if step.id == write.id {
            return true;
        }
        step.action_class == PlanActionClass::Read
            && !step.preferred_tools.is_empty()
            && step.preferred_tools.iter().all(|name| {
                matches!(
                    name.as_str(),
                    "contract.read" | "contract.inspect" | "contract.reconcile_transaction"
                )
            })
            && (depends_on(contract, &step.id, &write.id)
                || depends_on(contract, &write.id, &step.id))
    })
}

fn depends_on(contract: &SemanticPlanContract, from: &str, target: &str) -> bool {
    let mut pending = vec![from];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(step) = contract.steps.iter().find(|step| step.id == id) else {
            return false;
        };
        for dependency in &step.depends_on {
            if dependency == target {
                return true;
            }
            pending.push(dependency);
        }
    }
    false
}

#[cfg(test)]
mod tests;
