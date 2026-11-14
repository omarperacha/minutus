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

#[test]
fn acknowledged_partial_report_retains_hash_without_verified_or_automatic_plan_credit()
-> Result<(), Box<dyn std::error::Error>> {
    let all = vec![
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool("contract.request_workflow"),
    ];
    let mut policy = PlanPolicy::default();
    policy
        .accept(&workflow_plan("external_write"), &all, 8)
        .map_err(|error| error.message)?;
    let call = operation("workflow-partial", "contract.request_workflow");
    policy
        .validate_calls(std::slice::from_ref(&call))
        .map_err(|error| error.message)?;
    let hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    // Same native result shape as project_result: the tool returned a bounded
    // report, but the activity is Partial and task verification is unavailable.
    let report = serde_json::json!({
        "state":"submittedVerificationUnavailable", "completedSteps":0,
        "verificationError":{"code":"unsupported","message":"Receipt observation unavailable","retryable":false},
        "partialFacts":{"historicalCompletions":[]},
        "hostOutcomeSummary":"Submission was acknowledged; receipt observation is unavailable. Task completion is not established.",
        "latestObservedStage":{"stepIndex":0,"stage":"receiptUnavailable"},
        "currentWalletState":"notObserved",
        "lifecycle":{"observationScope":"thisInvocationOnly","observations":{"steps":[{
            "stepIndex":0,"transactionHash":hash,"submission":"acknowledged",
            "postSubmission":{"receipt":"unavailable","inclusionProof":"notStarted","completionProof":"notStarted"}
        }]}},
        "retryAutomatically":false,"automaticCleanup":false
    });
    let mut exchange = completed_operation(&call.call_id, ToolName::ContractRequestWorkflow);
    exchange.activity.status = TaskActivityStatus::Partial;
    exchange.workspace_grant_id = None;
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ToolResultPart::Json {
            value: serde_json::from_value(report.clone())?,
        }],
    };
    policy.record_results(std::slice::from_ref(&call), &[Ok(exchange.clone())]);
    assert_eq!(policy.operations_used, 1); // No refunded or fresh attempt credit.
    assert_eq!(policy.revisions_used, 0);
    assert!(policy.verified_outcome_call_ids.is_empty());
    assert!(!policy.outcome_satisfied);
    assert!(!policy.forecast_steps[0].semantic_complete);
    assert!(!policy.forecast_steps[0].semantic_completion_declared);
    assert!(!policy.final_response_ready());
    assert_eq!(
        policy.forecast_steps[0].semantic_completed_call_ids,
        [call.call_id]
    );
    let ToolOutcome::Completed { content } = exchange.result.outcome else {
        return Err("partial report lost".into());
    };
    let [ToolResultPart::Json { value }] = content.as_slice() else {
        return Err("public JSON report lost".into());
    };
    let retained = serde_json::to_value(value)?;
    assert_eq!(*value, serde_json::from_value::<JsonValue>(report)?);
    assert_eq!(
        retained["lifecycle"]["observations"]["steps"][0]["transactionHash"],
        hash
    );
    assert_eq!(retained["retryAutomatically"], false);
    Ok(())
}
