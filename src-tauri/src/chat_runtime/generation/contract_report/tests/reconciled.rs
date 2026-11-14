//! Actual public host trajectory replay; no wallet, RPC or new proof certificate.
use super::*;
use crate::chat_runtime::generation::state::GenerationToolState;
use ipc_types::{ToolName, ToolOutcome};
use model_runtime::PromptToolDefinition;
use serde_json::json;
use sha2::{Digest as _, Sha256};

fn replay() -> Result<GenerationToolState, Box<dyn std::error::Error>> {
    let values: Vec<Value> =
        serde_json::from_str(include_str!("reconciled_public_trajectory.json"))?;
    let tools = [
        "orchestration.submit_plan",
        "orchestration.complete_stage",
        "contract.inspect",
        "contract.read",
        "contract.request_workflow",
        "contract.reconcile_transaction",
    ]
    .map(|name| PromptToolDefinition {
        name: name.into(),
        description: String::new(),
        parameters: json!({}),
    });
    let mut state = GenerationToolState::default();
    for value in values {
        let mut exchange = fixtures()?.remove(1).1;
        exchange.request = serde_json::from_value(value["request"].clone())?;
        exchange.result = serde_json::from_value(value["result"].clone())?;
        exchange.activity.status = if exchange.request.name == ToolName::ContractRequestWorkflow {
            TaskActivityStatus::Partial
        } else {
            TaskActivityStatus::Completed
        };
        let name = value["request"]["name"]
            .as_str()
            .ok_or("tool name")?
            .to_owned();
        let call = ParsedToolCall {
            call_id: exchange.request.call_id.0.clone(),
            recipient: name.clone(),
            name,
            arguments: serde_json::from_value(value["request"]["arguments"].clone())?,
        };
        match exchange.request.name {
            ToolName::OrchestrationSubmitPlan => {
                state
                    .plan_policy
                    .accept(&call, &tools, 16)
                    .map_err(|error| error.message)?;
            }
            ToolName::OrchestrationCompleteStage => {
                state
                    .plan_policy
                    .complete_advisory_stage(&call)
                    .map_err(|error| error.message)?;
            }
            _ => {}
        }
        state.record_results(&[call], &[Ok(exchange)]);
    }
    Ok(state)
}
fn model_final() -> ChatTerminal {
    ChatTerminal::Final {
        reasoning: Some("User goals now all verified".into()),
        text: "Done — one write verified.".into(),
        reason: FinishReason::Stop,
        mutation_applied: false,
    }
}

#[test]
fn model_final_reports_execution_and_read_without_fabricating_formal_completion() -> TestResult {
    let state = replay()?;
    assert!(!state.plan_policy.final_response_ready());
    assert!(!state.plan_policy.contract_report_ready());
    assert_eq!(state.plan_policy.current_stage_id(), Some("verify-write"));
    let progress = state.plan_policy.progress_projection(false);
    let output = state
        .contract_report
        .model_final_text(&state.plan_policy, &model_final())
        .ok_or("result-bound final report")?;
    assert_eq!(progress, state.plan_policy.progress_projection(false));
    assert!(
        !state.plan_policy.final_response_ready(),
        "reporting must not acknowledge the unfinished read stage"
    );
    assert!(output.len() < MAX_REPORT_BYTES);
    assert!(output.contains("Execution confirmed for 0x7080f780"));
    assert!(output.contains("receipt-only reconciliation"));
    assert!(output.contains("Formal workflow completion: unverified"));
    assert!(output.contains("Current declared goals were not rechecked"));
    assert!(output.contains("12131800"));
    assert!(output.contains("12131810"));
    assert!(output.contains("\"values\":[\"42\"]"));
    assert!(output.contains("SnapshotMismatch"));
    assert!(!output.contains("Done — one write verified"));
    assert_eq!(output, include_str!("reconciled_report.md"));
    // Recorded vocabulary-only qualification of this exact text, not a local
    // model prerequisite. Runtime still performs fresh exact token admission.
    let measured: Value = serde_json::from_str(include_str!("reconciled_tokens.json"))?;
    assert_eq!(
        format!("{:x}", Sha256::digest(output.as_bytes())),
        measured["sha256"]
    );
    assert_eq!(u64::try_from(output.len())?, measured["bytes"]);
    let tokens = measured["exactTokens"].as_u64().ok_or("measured tokens")?;
    assert!(
        tokens
            > measured["observedEvidenceCap"]
                .as_u64()
                .ok_or("evidence cap")?
    );
    assert!(
        tokens
            <= measured["observedOrdinaryFinalCap"]
                .as_u64()
                .ok_or("final cap")?
    );
    Ok(())
}

#[test]
fn partial_model_output_or_unrelated_synthesis_is_not_replaced() -> TestResult {
    let mut state = replay()?;
    let exhausted = ChatTerminal::Final {
        reasoning: None,
        text: "unfinished".into(),
        reason: FinishReason::Length,
        mutation_applied: false,
    };
    assert!(
        state
            .contract_report
            .model_final_text(&state.plan_policy, &exhausted)
            .is_none()
    );
    let mut unrelated = fixtures()?.remove(1);
    unrelated.0.name = "web.retrieve".into();
    unrelated.0.recipient = unrelated.0.name.clone();
    state.record_results(&[unrelated.0], &[Ok(unrelated.1)]);
    assert!(
        state
            .contract_report
            .model_final_text(&state.plan_policy, &model_final())
            .is_none()
    );
    Ok(())
}

#[test]
fn actual_evidence_round_final_decision_uses_ordinary_final_output_admission() -> TestResult {
    let state = replay()?;
    let conversation = Conversation {
        id: ipc_types::ConversationId("fixture".into()),
        project_id: ipc_types::ProjectId("fixture".into()),
        title: "Public trajectory".into(),
        messages: vec![],
        created_at: "2026-11-14T00:00:00Z".into(),
        updated_at: "2026-11-14T00:00:00Z".into(),
    };
    let evidence = state.execution_profile(&conversation, 0, false);
    assert_eq!(evidence.work, agent_core::ExecutionWorkKind::Evidence);
    assert!(!evidence.preserves_final_answer_output());
    let final_report = reporting_profile(evidence);
    assert_eq!(
        final_report.work,
        agent_core::ExecutionWorkKind::FinalResponse
    );
    assert!(final_report.preserves_final_answer_output());
    assert_eq!(final_report.profile, evidence.profile);
    assert_eq!(final_report.policy_version, evidence.policy_version);
    assert!(!final_report.bounded_action);
    assert!(!final_report.bulk_arguments);
    assert!(!state.plan_policy.final_response_ready());
    Ok(())
}

#[test]
fn reconciliation_without_its_exact_receipt_and_unchecked_goals_shape_falls_back() -> TestResult {
    for field in ["currentGoals", "canonicalAtObservation"] {
        let mut state = replay()?;
        let exchange = state
            .contract_report
            .exchanges
            .iter_mut()
            .find(|exchange| exchange.request.name == ToolName::ContractReconcileTransaction)
            .ok_or("reconciliation")?;
        let ToolOutcome::Completed { content } = &mut exchange.result.outcome else {
            return Err("completed result".into());
        };
        let [ipc_types::ToolResultPart::Json { value }] = content.as_mut_slice() else {
            return Err("JSON result".into());
        };
        let mut data = serde_json::to_value(&*value)?;
        data[field] = if field == "currentGoals" {
            json!("verified")
        } else {
            json!(false)
        };
        *value = serde_json::from_value(data)?;
        assert!(
            state
                .contract_report
                .model_final_text(&state.plan_policy, &model_final())
                .is_none()
        );
    }
    Ok(())
}
