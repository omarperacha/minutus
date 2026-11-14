//! Scripted real host bookkeeping; no model execution or new chain authority.
use super::*;
use crate::chat_runtime::generation::state::GenerationToolState;
use ipc_types::{ToolName, ToolOutcome};
use model_runtime::PromptToolDefinition;
use serde_json::json;
use std::collections::BTreeMap;

fn recorded(
    encoded: &str,
) -> Result<(ParsedToolCall, ChatToolExchange), Box<dyn std::error::Error>> {
    let (mut call, mut exchange) = fixtures()?.remove(1);
    let value: Value = serde_json::from_str(encoded)?;
    exchange.request = serde_json::from_value(value["request"].clone())?;
    exchange.result = serde_json::from_value(value["result"].clone())?;
    call.call_id = exchange.request.call_id.0.clone();
    call.name = value["request"]["name"].as_str().ok_or("tool name")?.into();
    call.recipient = call.name.clone();
    call.arguments = serde_json::from_value(value["request"]["arguments"].clone())?;
    Ok((call, exchange))
}
fn control(name: &str, template: &ChatToolExchange) -> (ParsedToolCall, ChatToolExchange) {
    let mut exchange = template.clone();
    let call = ParsedToolCall {
        call_id: name.into(),
        recipient: name.into(),
        name: name.into(),
        arguments: BTreeMap::default(),
    };
    exchange.request.call_id.0 = name.into();
    exchange.result.call_id.0 = name.into();
    exchange.request.arguments = BTreeMap::default();
    exchange.request.name = if name == "orchestration.submit_plan" {
        ToolName::OrchestrationSubmitPlan
    } else {
        ToolName::OrchestrationCompleteStage
    };
    exchange.result.outcome = ToolOutcome::Completed { content: vec![] };
    (call, exchange)
}

#[test]
fn initial_inspection_then_plan_read_workflow_read_and_completion_retains_all_observations()
-> TestResult {
    let mut state = GenerationToolState::default();
    let (inspect, inspection) = recorded(include_str!("initial_source_inspect.json"))?;
    state.record_results(&[inspect], &[Ok(inspection.clone())]);
    let tools = [
        "orchestration.submit_plan",
        "orchestration.complete_stage",
        "contract.inspect",
        "contract.read",
        "contract.request_workflow",
    ]
    .map(|name| PromptToolDefinition {
        name: name.into(),
        description: String::new(),
        parameters: json!({}),
    });
    let (mut plan, mut plan_result) = control("orchestration.submit_plan", &inspection);
    plan.arguments = serde_json::from_value(
        json!({"contractVersion":crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION,"mode":"initial","outcome":"other","target":"Report the exact one-workflow outcome and separate public observations","constraints":[],"assumptions":[],"successCriteria":["Report proved and unknown facts without retry"],"steps":[{"id":"workflow","actionClass":"external_write","dependsOn":[],"objective":"Request one workflow","doneWhen":"Its outcome is reported","suggestedTools":["contract.request_workflow"]}]}),
    )?;
    plan_result.request.arguments = serde_json::from_value(json!(plan.arguments))?;
    state
        .plan_policy
        .accept(&plan, &tools, 12)
        .map_err(|error| error.message)?;
    state.record_results(&[plan], &[Ok(plan_result)]);
    let (read, initial_read) = recorded(include_str!("preread1.json"))?;
    state.record_results(&[read], &[Ok(initial_read)]);
    for (call, exchange) in fixtures()? {
        state.record_results(&[call], &[Ok(exchange)]);
    }
    let (complete, exchange) = control("orchestration.complete_stage", &inspection);
    if !state.plan_policy.final_response_ready() {
        state
            .plan_policy
            .complete_advisory_stage(&complete)
            .map_err(|error| error.message)?;
    }
    state.record_results(&[complete], &[Ok(exchange)]);
    assert!(state.plan_policy.contract_report_ready());
    let output = state
        .contract_report
        .render(state.plan_policy.revision())
        .ok_or("initial metadata must not exclude final report")?;
    assert!(output.len() < MAX_REPORT_BYTES);
    assert_eq!(output, include_str!("discovery_report.md"));
    assert_eq!(
        output
            .matches("Preparatory separate unsigned observation")
            .count(),
        2
    );
    assert_eq!(
        output
            .matches("Later separate unsigned observation")
            .count(),
        1
    );
    assert!(output.contains("\"values\":[\"1\"]"));
    assert!(output.contains("\"values\":[\"42\"]"));
    assert!(state.contract_report.render(2).is_none());
    Ok(())
}

#[test]
fn initial_revision_exception_never_accepts_preplan_reads_or_workflows() -> TestResult {
    for index in [0, 1] {
        let mut state = ContractReportState::default();
        let (call, exchange) = fixtures()?.remove(index);
        state.record(0, &[call], &[Ok(exchange)]);
        for (call, exchange) in fixtures()? {
            state.record(1, &[call], &[Ok(exchange)]);
        }
        assert!(state.render(1).is_none());
    }
    Ok(())
}
