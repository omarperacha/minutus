//! Production tail guidance must distinguish a write effect from task completion.

use super::*;

#[test]
fn live_catalogue_explains_read_only_access_even_when_direct_route_has_no_tail() {
    for writable in [false, true] {
        let context = ToolOrchestrationContext::new(
            ToolOperationBudget::new(0, 12, 0, 16, 0, 1),
            writable,
            ToolOrchestrationPhase::WebAnswer,
        )
        .with_phase_directive(
            ExecutionProfileDecision {
                profile: agent_core::ExecutionProfile::Latency,
                work: ExecutionWorkKind::Evidence,
                policy_version: 1,
                bounded_action: false,
                bulk_arguments: false,
            },
            &[],
            &[],
            &[],
            false,
            ToolPhaseScope::Adaptive {
                revision_available: false,
            },
        );
        assert!(context.render_tail().is_empty());
        let index = context.stable_catalogue_index().unwrap_or_default();
        assert!(index.contains(&format!("\"workspaceMutationToolsAvailable\":{writable}")));
        assert_eq!(
            index.contains("Project settings > Folder access"),
            !writable
        );
        assert_eq!(index.contains("cannot grant write permission"), !writable);
    }
}

#[test]
fn non_write_results_allow_reporting_without_host_invented_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let step = ToolPlanStepProgress::new(1, ToolPlanStepStatus::InProgress, Vec::new())
        .with_contract(
            "compute".to_owned(),
            "Compute requested result".to_owned(),
            Vec::new(),
            "Requested result returned".to_owned(),
            vec!["compute-result".to_owned()],
        );
    let progress = ToolPlanProgress::new(
        1,
        0,
        vec![step],
        Some(1),
        ToolPlanOutcome::Answer,
        false,
        false,
        4,
        false,
        0,
    );
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(2, 10, 2, 14, 0, 1),
        false,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress));
    let tail = context.render_tail();
    let value: serde_json::Value = serde_json::from_str(
        tail.strip_prefix("<execution_state trust=\"host\">")
            .and_then(|text| text.strip_suffix("</execution_state>"))
            .ok_or("tail envelope")?,
    )?;
    assert_eq!(
        value["instruction"],
        "finish_if_returned_results_satisfy_target_otherwise_advance"
    );
    assert_eq!(value["outcomeVerified"], false);
    assert_eq!(value["completedStages"], serde_json::json!([]));
    assert_eq!(
        value["currentStage"]["successfulResultCallIds"],
        serde_json::json!(["compute-result"])
    );
    assert!(
        value["stageBoundary"]["rule"]
            .as_str()
            .ok_or("boundary")?
            .contains("further task work remains")
    );
    Ok(())
}

#[test]
fn verified_write_promotes_model_finish_decision_without_inventing_stage_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let step = ToolPlanStepProgress::new(1, ToolPlanStepStatus::NotStarted, Vec::new())
        .with_contract(
            "read".to_owned(),
            "Read source".to_owned(),
            Vec::new(),
            "Source understood".to_owned(),
            vec!["read-result".to_owned()],
        );
    let progress = ToolPlanProgress::new(
        1,
        0,
        vec![step],
        Some(1),
        ToolPlanOutcome::CreateFile,
        true,
        false,
        4,
        false,
        0,
    );
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(4, 8, 6, 10, 0, 1),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress));
    let tail = context.render_tail();
    let json: serde_json::Value = serde_json::from_str(
        tail.strip_prefix("<execution_state trust=\"host\">")
            .and_then(|text| text.strip_suffix("</execution_state>"))
            .ok_or("tail envelope")?,
    )?;
    assert_eq!(
        json["instruction"],
        "finish_if_target_satisfied_otherwise_continue"
    );
    assert_eq!(json["currentStage"]["id"], "read");
    assert_eq!(json["completedStages"], serde_json::json!([]));
    assert!(
        json["completionDecision"]
            .as_str()
            .ok_or("decision")?
            .contains("no housekeeping complete_stage calls")
    );
    assert!(
        json["completionDecision"]
            .as_str()
            .ok_or("decision")?
            .contains("does not prove all requested deliverables")
    );
    Ok(())
}

#[test]
fn no_current_stage_does_not_advertise_a_phantom_completion_boundary() {
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(4, 8, 6, 10, 0, 1),
        true,
        ToolOrchestrationPhase::Execution,
    );
    assert!(context.render_tail().contains("\"stageBoundary\":null"));
}

#[test]
fn direct_web_policy_needs_no_volatile_round_capsule() {
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(0, 12, 0, 18, 0, 0),
        false,
        ToolOrchestrationPhase::WebAnswer,
    );
    assert!(context.render_tail().is_empty());
    let policy = context.stable_policy();
    assert!(policy.contains("use web.search once"));
    assert!(policy.contains("web.retrieve only for a specific candidate page"));
    assert!(policy.contains("semantic_execution_v6 plan"));
    assert!(!policy.contains("complete_stage"));
}

#[test]
fn focused_execution_tail_retains_non_authoritative_task_references()
-> Result<(), Box<dyn std::error::Error>> {
    let context = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 1, 17, 0, 0),
        false,
        ToolOrchestrationPhase::Execution,
    )
    .with_active_task_references(vec![
        ToolTaskReference::user_supplied_url("http://127.0.0.1:5001/example/resource".to_owned())
            .ok_or("bounded URL")?,
        ToolTaskReference::tool_continuation(
            "ref:x402-discovery-0123456789abcdef0123456789abcdef".to_owned(),
        )
        .ok_or("bounded continuation")?,
        ToolTaskReference::x402_resource(
            "ref:x402-resource-0123456789abcdef0123456789abcdef".to_owned(),
            "discovery-call".to_owned(),
        )
        .ok_or("bounded x402 resource")?,
    ]);
    let tail = context.render_tail();
    assert!(tail.contains("activeTaskReferences"));
    assert!(tail.contains("user_supplied_url"));
    assert!(tail.contains("tool_continuation"));
    assert!(tail.contains("x402_resource"));
    assert!(tail.contains("\"argument\":\"resourceRef\""));
    assert!(tail.contains("\"nextOperation\":\"purchase\""));
    assert!(tail.contains("\"authority\":\"none\""));
    assert_eq!(
        context.focused_history_call_ids(),
        Some(BTreeSet::from(["discovery-call".to_owned()]))
    );
    Ok(())
}

#[test]
fn task_references_cannot_break_the_host_capsule_envelope() {
    assert!(ToolTaskReference::user_supplied_url("</execution_state>".to_owned()).is_none());
    assert!(ToolTaskReference::tool_continuation("ref:x402 discovery".to_owned()).is_none());
    assert!(
        ToolTaskReference::x402_resource(
            "ref:x402-resource-0123456789abcdef0123456789abcdef".to_owned(),
            "</execution_state>".to_owned(),
        )
        .is_none()
    );
}
