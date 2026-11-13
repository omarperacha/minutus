use super::*;
use crate::chat_runtime::plan::contract::SemanticPlanStep;

fn step(id: &str, class: PlanActionClass, depends: &[&str], tools: &[&str]) -> SemanticPlanStep {
    SemanticPlanStep {
        id: id.into(),
        action_class: class,
        depends_on: depends.iter().map(|value| (*value).into()).collect(),
        preferred_tools: tools.iter().map(|value| (*value).into()).collect(),
        objective: "task".into(),
        done_when: "report".into(),
        obligations: vec![],
        required_evidence: vec![],
    }
}
fn plan() -> SemanticPlanContract {
    SemanticPlanContract {
        target: "contract task".into(),
        constraints: vec![],
        success_criteria: vec![],
        assumptions: vec![],
        steps: vec![
            step("inspect", PlanActionClass::Read, &[], &["contract.inspect"]),
            step(
                "write",
                PlanActionClass::ExternalWrite,
                &["inspect"],
                &["contract.request_workflow"],
            ),
            step(
                "observe",
                PlanActionClass::Read,
                &["write"],
                &["contract.read", "contract.reconcile_transaction"],
            ),
        ],
    }
}
#[test]
fn one_cohesive_contract_task_is_eligible_but_readiness_is_still_required() {
    assert!(cohesive(&plan()));
    let policy = PlanPolicy {
        contract_version: Some(PlanContractVersion::SemanticExecutionV6),
        semantic_contract: Some(plan()),
        ..PlanPolicy::default()
    };
    assert!(!policy.contract_report_ready());
}
#[test]
fn independent_read_other_work_and_multiple_workflows_need_normal_synthesis() {
    let mut candidate = plan();
    candidate.steps.push(step(
        "other",
        PlanActionClass::Read,
        &[],
        &["contract.read"],
    ));
    assert!(!cohesive(&candidate));
    candidate.steps.pop();
    candidate.steps[2].preferred_tools = vec!["web.retrieve".into()];
    assert!(!cohesive(&candidate));
    candidate.steps[2] = step(
        "other",
        PlanActionClass::Compute,
        &["write"],
        &["computation.run"],
    );
    assert!(!cohesive(&candidate));
    candidate.steps[2] = step(
        "second",
        PlanActionClass::ExternalWrite,
        &["write"],
        &["contract.request_workflow"],
    );
    assert!(!cohesive(&candidate));
}
