use super::*;

#[test]
fn natural_four_stage_plan_is_accepted() -> Result<(), IpcError> {
    let all = [tool(PLAN_TOOL), tool("filesystem.read_text")];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&plan_with_step_count(4), &all, 18)?;

    assert_eq!(accepted["estimatedCalls"], 4);
    assert!(policy.active());
    Ok(())
}

#[test]
fn semantic_contract_is_versioned_and_category_projection_keeps_safe_alternatives()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.find_paths"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&semantic_plan_call("initial"), &all, 8)?;
    assert_eq!(
        accepted["contractVersion"],
        crate::chat_tools::ROLELESS_SEMANTIC_PLAN_CONTRACT_VERSION
    );
    assert_eq!(
        policy
            .tools(&all)
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [FORECAST_TOOL]
    );
    policy.accept_forecast(&semantic_forecast_call("initial"), &all)?;
    let available = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(available.contains("filesystem.inspect_image"));
    assert!(available.contains("filesystem.read_text"));
    assert!(!available.contains("filesystem.write_text"));
    assert!(available.contains(PLAN_TOOL));
    assert!(available.contains(FORECAST_TOOL));
    Ok(())
}

#[test]
fn semantic_v3_keeps_typed_roles_independent_and_gates_the_outcome_operation()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v3_plan_call("initial"), &all, 12)?;
    policy.accept_forecast(&semantic_v3_forecast_call("initial"), &all)?;

    let text = operation("text-receipt", "filesystem.read_text");
    policy.validate_calls(std::slice::from_ref(&text))?;
    policy.record_results(
        std::slice::from_ref(&text),
        &[Ok(completed_operation(
            "text-receipt",
            ToolName::FilesystemReadText,
        ))],
    );

    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "convention")
            .is_some_and(ForecastStep::complete)
    );
    assert!(
        policy
            .forecast_steps
            .iter()
            .find(|step| step.id == "source")
            .is_some_and(ForecastStep::unfinished)
    );
    assert!(
        !policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.write_text")
    );
    Ok(())
}

#[test]
fn semantic_v4_plan_and_forecast_preserve_the_clean_validation_boundary() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&semantic_v4_plan_call("initial"), &all, 12)?;
    assert_eq!(
        accepted["contractVersion"],
        crate::chat_tools::OUTCOME_SEMANTIC_PLAN_CONTRACT_VERSION
    );
    assert_eq!(
        policy
            .tools(&all)
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [FORECAST_TOOL]
    );
    let forecast = semantic_v4_forecast_call("initial");
    policy.validate_calls(std::slice::from_ref(&forecast))?;
    assert!(!policy.authorize_safe_read_recovery(
        &[operation("premature-read", "filesystem.read_text")],
        &all,
    ));
    policy.accept_forecast(&forecast, &all)?;
    assert!(!policy.forecast_required());
    Ok(())
}

#[test]
fn semantic_v5_moves_directly_from_plan_to_host_routed_operations() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        collector_tool(),
        tool("filesystem.write_text"),
        tool("computation.run"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool(SEARCH_TOOLS_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&semantic_v5_plan_call("initial"), &all, 12)?;
    assert_eq!(
        accepted["contractVersion"],
        crate::chat_tools::HOST_ROUTED_SEMANTIC_PLAN_CONTRACT_VERSION
    );
    assert!(!policy.forecast_required());
    assert!(!policy.accepts_execution_forecast());
    assert!(policy.requires_plan_only_round());

    let available = policy
        .tools(&all)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    assert!(available.contains("filesystem.inspect_image"));
    assert!(available.contains("filesystem.read_text"));
    assert!(!available.contains("filesystem.write_text"));
    assert!(!available.contains("computation.run"));
    assert!(!available.contains(FORECAST_TOOL));
    assert!(available.contains(PLAN_TOOL));

    let image = operation("v5-image", "filesystem.inspect_image");
    policy.validate_calls(std::slice::from_ref(&image))?;
    policy.record_results(
        std::slice::from_ref(&image),
        &[Ok(completed_operation(
            "v5-image",
            ToolName::FilesystemInspectImage,
        ))],
    );
    let text = operation("v5-text", "filesystem.read_text");
    policy.validate_calls(std::slice::from_ref(&text))?;
    policy.record_results(
        std::slice::from_ref(&text),
        &[Ok(completed_operation(
            "v5-text",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "filesystem.write_text")
    );
    Ok(())
}

#[test]
fn semantic_v5_projects_exact_bindings_and_keeps_revision_recovery() -> Result<(), IpcError> {
    let all = vec![collector_tool(), tool(PLAN_TOOL), tool(FORECAST_TOOL)];
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v5_plan_call("initial"), &all, 12)?;
    let projected = policy.tools(&all);
    let revision = projected
        .iter()
        .find(|tool| tool.name == PLAN_TOOL)
        .ok_or_else(IpcError::internal)?;
    assert!(
        revision
            .description
            .starts_with("Optional recovery control.")
    );
    assert!(revision.description.contains("If called, use mode=revise"));
    assert!(
        !revision
            .description
            .contains("Current host state requires mode=revise")
    );
    let collector = projected
        .iter()
        .find(|tool| tool.name == "filesystem.collect_evidence")
        .ok_or_else(IpcError::internal)?;
    assert_eq!(
        collector
            .parameters
            .pointer("/properties/items/items/properties/id/enum"),
        Some(&serde_json::json!(["convention-format", "source-image"]))
    );
    assert_eq!(
        collector
            .parameters
            .pointer("/properties/items/items/properties/operation/enum"),
        Some(&serde_json::json!([
            "load_exact_image_semantic",
            "load_exact_text_format",
            "load_exact_text_semantic"
        ]))
    );

    let mut invalid = operation("v5-invalid-collector", "filesystem.collect_evidence");
    invalid.arguments.insert(
        "items".to_owned(),
        serde_json::json!([{
            "id": "invented-source",
            "operation": "load_exact_image_semantic"
        }]),
    );
    let error = policy
        .validate_calls(std::slice::from_ref(&invalid))
        .err()
        .ok_or_else(IpcError::internal)?;
    for expected in [
        "invented-source",
        "source-image",
        "convention-format",
        "load_exact_image_semantic",
        "load_exact_text_format",
    ] {
        assert!(error.message.contains(expected));
    }
    assert!(
        policy.tools(&all).iter().any(|tool| tool.name == PLAN_TOOL),
        "plan revision remains available as an execution recovery option"
    );
    Ok(())
}

#[test]
fn semantic_v6_keeps_one_canonical_catalogue_and_promotes_suggestions_in_state()
-> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.inspect_image"),
        tool("filesystem.read_text"),
        tool("filesystem.find_paths"),
        tool("filesystem.write_text"),
        tool("computation.run"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
        tool(SEARCH_TOOLS_TOOL),
        tool(FORECAST_TOOL),
    ];
    let mut policy = PlanPolicy::default();
    let accepted = policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;
    assert_eq!(
        accepted["contractVersion"],
        crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION
    );
    assert!(policy.uses_advisory_stages());
    assert!(!policy.forecast_required());
    assert!(policy.bounded_stage_contract(&all, true).is_none());

    assert_eq!(
        policy.current_step_unfinished_tools(),
        ["filesystem.read_text"]
    );
    assert_eq!(
        policy.post_completion_relevant_tools(&all),
        ["filesystem.write_text"]
    );
    let projected = policy.tools(&all);
    let projected_names = projected
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        projected_names,
        [
            "filesystem.read_text",
            "filesystem.write_text",
            PLAN_TOOL,
            COMPLETE_STAGE_TOOL,
            SEARCH_TOOLS_TOOL,
        ]
    );
    for available in [
        "filesystem.read_text",
        "filesystem.write_text",
        PLAN_TOOL,
        COMPLETE_STAGE_TOOL,
        SEARCH_TOOLS_TOOL,
    ] {
        assert!(projected.iter().any(|tool| tool.name == available));
    }
    assert!(!projected.iter().any(|tool| tool.name == FORECAST_TOOL));

    let read = operation("read-source-result", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&read),
        &[Ok(completed_operation(
            "read-source-result",
            ToolName::FilesystemReadText,
        ))],
    );
    assert!(
        policy
            .current_step_relevant_tools(&all)
            .contains(&COMPLETE_STAGE_TOOL)
    );
    assert_eq!(
        policy.post_completion_relevant_tools(&all),
        ["filesystem.write_text"]
    );
    let after_result = policy.tools(&all);
    assert_eq!(projected, after_result);
    let second_read = operation("second-read-source-result", "filesystem.read_text");
    policy.record_results(
        std::slice::from_ref(&second_read),
        &[Ok(completed_operation(
            "second-read-source-result",
            ToolName::FilesystemReadText,
        ))],
    );
    assert_eq!(after_result, policy.tools(&all));
    Ok(())
}

#[test]
fn semantic_v6_prompt_surface_is_plan_wide_and_search_expandable() -> Result<(), IpcError> {
    let workspaces = [crate::chat_tools::ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "project".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }];
    let all = crate::chat_tools::chat_tool_definitions(&workspaces)?;
    let mut policy = PlanPolicy::default();
    policy.accept(&semantic_v6_plan_call("initial"), &all, 12)?;

    let projected = policy.tools(&all);
    let names = projected
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"filesystem.read_text"));
    assert!(names.contains(&"filesystem.write_text"));
    assert!(names.contains(&SEARCH_TOOLS_TOOL));
    assert!(!names.contains(&"computation.run"));
    assert!(projected.len() < all.len() / 2);

    let full_bytes = tool_contract_bytes(&all);
    let projected_bytes = tool_contract_bytes(&projected);
    assert!(projected_bytes.saturating_mul(2) < full_bytes);

    let plan_revision = policy.revision();
    assert_eq!(policy.tool_surface_revision(), 0);
    policy.load_task_tools(["computation.run".to_owned()]);
    assert_eq!(policy.revision(), plan_revision);
    assert_eq!(policy.tool_surface_revision(), 1);
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == "computation.run")
    );
    Ok(())
}

fn tool_contract_bytes(tools: &[PromptToolDefinition]) -> usize {
    tools.iter().fold(0_usize, |total, tool| {
        total
            .saturating_add(tool.name.len())
            .saturating_add(tool.description.len())
            .saturating_add(tool.parameters.to_string().len())
    })
}

#[test]
fn semantic_v6_deduplicates_shared_suggestions_across_ready_stages() -> Result<(), IpcError> {
    let all = vec![
        tool("filesystem.find_paths"),
        tool("filesystem.list"),
        tool("filesystem.read_text"),
        tool("filesystem.write_text"),
        tool(PLAN_TOOL),
        tool(COMPLETE_STAGE_TOOL),
    ];
    let mut call = semantic_v6_plan_call("initial");
    call.arguments.insert(
        "steps".to_owned(),
        serde_json::json!([
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The source image is located", "id":"locate-png",
                "objective":"Locate the source image",
                "suggestedTools":["filesystem.find_paths", "filesystem.list"]
            },
            {
                "actionClass":"read", "dependsOn":[],
                "doneWhen":"The output conventions are known", "id":"conventions",
                "objective":"Read the output conventions",
                "suggestedTools":["filesystem.find_paths", "filesystem.read_text"]
            },
            {
                "actionClass":"workspace_write", "dependsOn":["locate-png", "conventions"],
                "doneWhen":"The requested output exists", "id":"write-output",
                "objective":"Create the requested output",
                "suggestedTools":["filesystem.write_text"]
            }
        ]),
    );

    let mut policy = PlanPolicy::default();
    policy.accept(&call, &all, 12)?;
    assert_eq!(
        policy.current_step_unfinished_tools(),
        [
            "filesystem.find_paths",
            "filesystem.read_text",
            "filesystem.list"
        ]
    );

    let projected = policy.tools(&all);
    assert_eq!(projected[0].name, "filesystem.find_paths");
    assert_eq!(projected[1].name, "filesystem.list");
    let names = projected
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names.len(),
        names.iter().copied().collect::<BTreeSet<_>>().len()
    );

    // Work completed for a later independent stage remains retained, but its
    // completion control must not conflict with the focused primary stage.
    let primary = policy
        .primary_ready_step_index()
        .unwrap_or(policy.forecast_steps.len());
    assert!(primary < policy.forecast_steps.len());
    let primary_id = policy.forecast_steps[primary].id.clone();
    let (later_tool, later_name) = if primary_id == "locate-png" {
        (ToolName::FilesystemReadText, "filesystem.read_text")
    } else {
        (ToolName::FilesystemList, "filesystem.list")
    };
    let later = operation("later-result", later_name);
    policy.record_results(
        std::slice::from_ref(&later),
        &[Ok(completed_operation("later-result", later_tool))],
    );
    assert!(
        policy.forecast_steps[primary]
            .semantic_completed_call_ids
            .is_empty()
    );
    assert!(
        policy
            .tools(&all)
            .iter()
            .any(|tool| tool.name == COMPLETE_STAGE_TOOL)
    );

    let (primary_tool, primary_name) = if primary_id == "locate-png" {
        (ToolName::FilesystemList, "filesystem.list")
    } else {
        (ToolName::FilesystemReadText, "filesystem.read_text")
    };
    let primary_result = operation("primary-result", primary_name);
    policy.record_results(
        std::slice::from_ref(&primary_result),
        &[Ok(completed_operation("primary-result", primary_tool))],
    );
    assert_eq!(projected, policy.tools(&all));
    Ok(())
}
