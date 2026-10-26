use super::*;

#[test]
fn restored_focused_phase_keeps_original_instruction_and_dependency_receipt_only() {
    let conversation = workspace_image_tool_conversation(
        AssetId("asset-image".to_owned()),
        "2026-08-15T12:00:00Z",
    );
    let selected = BTreeSet::from(["inspect-image-call".to_owned()]);
    let projected =
        focused_phase_messages(&conversation, &selected, 0..conversation.messages.len());
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[0].role, MessageRole::User);
    assert!(projected[0].parts.iter().all(|part| {
        matches!(part, MessagePart::Text { text } if text == "Describe the diagram")
    }));
    assert!(projected[1].parts.iter().all(|part| {
        matches!(part, MessagePart::ToolCall { request } if request.call_id.0 == "inspect-image-call")
    }));
    assert!(projected[2].parts.iter().all(|part| {
        matches!(part, MessagePart::ToolResult { result } if result.call_id.0 == "inspect-image-call")
    }));
}

#[test]
fn focused_phase_includes_the_latest_unresolved_current_step_receipt() {
    let progress = ToolPlanProgress::new(
        2,
        0,
        vec![ToolPlanStepProgress::new(
            1,
            ToolPlanStepStatus::Blocked,
            vec!["filesystem.collect_evidence".to_owned()],
        )],
        Some(1),
        ToolPlanOutcome::CreateFile,
        false,
        false,
        2,
        true,
        0,
    )
    .with_retained_call_ids(["ambiguous-candidates".to_owned()]);
    let orchestration = ToolOrchestrationContext::new(
        ToolOperationBudget::new(2, 10, 2, 16, 2, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress));

    assert_eq!(
        orchestration.focused_history_call_ids(),
        Some(BTreeSet::from(["ambiguous-candidates".to_owned()]))
    );
}

#[test]
fn focused_phase_selects_exact_dependencies_instead_of_every_prior_receipt() {
    let completed = |ordinal, id: &str, call_id: &str| {
        ToolPlanStepProgress::new(ordinal, ToolPlanStepStatus::ForecastComplete, Vec::new())
            .with_contract(
                id.to_owned(),
                format!("Complete {id}"),
                Vec::new(),
                format!("{id} complete"),
                vec![call_id.to_owned()],
            )
    };
    let convention = completed(2, "convention", "convention-receipt")
        .with_dependencies(vec!["source".to_owned()]);
    let current = ToolPlanStepProgress::new(
        4,
        ToolPlanStepStatus::NotStarted,
        vec!["filesystem.write_text".to_owned()],
    )
    .with_contract(
        "write".to_owned(),
        "Write artifact".to_owned(),
        Vec::new(),
        "Artifact verified".to_owned(),
        Vec::new(),
    )
    .with_dependencies(vec!["convention".to_owned()]);
    let progress = ToolPlanProgress::new(
        1,
        0,
        vec![
            completed(1, "source", "source-receipt"),
            convention,
            completed(3, "unrelated", "unrelated-receipt"),
            current,
        ],
        Some(4),
        ToolPlanOutcome::CreateFile,
        false,
        false,
        2,
        true,
        0,
    )
    .with_retained_call_ids(["latest-refinement".to_owned()]);
    let orchestration = ToolOrchestrationContext::new(
        ToolOperationBudget::new(2, 10, 2, 16, 2, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress));

    assert_eq!(
        orchestration.focused_history_call_ids(),
        Some(BTreeSet::from([
            "latest-refinement".to_owned(),
            "convention-receipt".to_owned(),
            "source-receipt".to_owned(),
        ]))
    );
}

#[test]
fn focused_phase_keeps_completed_work_for_a_ready_independent_stage() {
    let source = ToolPlanStepProgress::new(1, ToolPlanStepStatus::ForecastComplete, Vec::new())
        .with_contract(
            "source".to_owned(),
            "Locate source".to_owned(),
            Vec::new(),
            "Source located".to_owned(),
            vec!["source-receipt".to_owned()],
        );
    let convention = ToolPlanStepProgress::new(
        2,
        ToolPlanStepStatus::NotStarted,
        vec!["filesystem.read_text".to_owned()],
    )
    .with_contract(
        "convention".to_owned(),
        "Read convention".to_owned(),
        Vec::new(),
        "Convention known".to_owned(),
        Vec::new(),
    )
    .with_dependencies(vec!["source".to_owned()]);
    let transcribe = ToolPlanStepProgress::new(
        3,
        ToolPlanStepStatus::InProgress,
        vec!["filesystem.inspect_image".to_owned()],
    )
    .with_contract(
        "transcribe".to_owned(),
        "Read image".to_owned(),
        Vec::new(),
        "Image values retained".to_owned(),
        vec!["image-receipt".to_owned()],
    )
    .with_dependencies(vec!["source".to_owned()]);
    let progress = ToolPlanProgress::new(
        1,
        0,
        vec![source, convention, transcribe],
        Some(2),
        ToolPlanOutcome::CreateFile,
        false,
        false,
        2,
        true,
        0,
    );
    let orchestration = ToolOrchestrationContext::new(
        ToolOperationBudget::new(3, 9, 4, 14, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(progress));

    assert_eq!(
        orchestration.focused_history_call_ids(),
        Some(BTreeSet::from([
            "image-receipt".to_owned(),
            "source-receipt".to_owned(),
        ]))
    );
}
