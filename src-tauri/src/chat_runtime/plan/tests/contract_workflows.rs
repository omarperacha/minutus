//! Scripted semantic admission only; no source/approval/signing authority is minted.
use super::*;
fn workflow_plan(class: &str) -> ParsedToolCall {
    ParsedToolCall {call_id:"plan-workflow".into(),recipient:PLAN_TOOL.into(),name:PLAN_TOOL.into(),arguments:serde_json::from_value(serde_json::json!({"contractVersion":crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION,"mode":"initial","outcome":"other","target":"Request exact source-qualified token operations","constraints":["No automatic retry or cleanup"],"assumptions":[],"successCriteria":["Report verified outcomes or exact partial failure and remaining authority"],"steps":[{"id":"workflow","actionClass":class,"dependsOn":[],"objective":"Request the immutable ordered token workflow once","doneWhen":"The single workflow attempt and residual allowance facts are reported","suggestedTools":["contract.request_workflow"]}]})).unwrap_or_default()}
}
#[test]
fn qualified_workflow_requires_external_write_plan_and_denial_ends_attempt() -> Result<(), IpcError>
{
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("contract.request_workflow"),
    ];
    let mut policy = PlanPolicy::default();
    let call = operation("workflow", "contract.request_workflow");
    assert!(
        !policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "contract.request_workflow")
    );
    assert!(policy.initial_plan_required(std::slice::from_ref(&call)));
    assert!(policy.validate_calls(std::slice::from_ref(&call)).is_err());
    policy.accept(&workflow_plan("external_write"), &all, 8)?;
    policy.validate_calls(std::slice::from_ref(&call))?;
    let mut denied = completed_operation("workflow", ToolName::ContractRequestWorkflow);
    denied.result.outcome = ToolOutcome::Failed {
        error: IpcError::invalid_request(
            "second step denied; prior allowance remains at last verified block",
        ),
    };
    policy.record_results(&[call], &[Ok(denied)]);
    assert!(policy.final_response_ready());
    assert!(policy.tools(&all).is_empty());
    Ok(())
}
#[test]
fn a_read_stage_cannot_admit_a_profile_qualified_write() {
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("contract.request_workflow"),
    ];
    let mut policy = PlanPolicy::default();
    if policy.accept(&workflow_plan("read"), &all, 8).is_ok() {
        assert!(
            policy
                .validate_calls(&[operation("workflow", "contract.request_workflow")])
                .is_err()
        );
    }
}
