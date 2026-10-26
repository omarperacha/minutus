use super::*;
use ipc_types::{IpcError, ToolOutcome, WorkspaceGrantId, WorkspacePermissionLevel};
use model_runtime::ParsedToolCall;

#[test]
fn filesystem_tool_registry_uses_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let workspaces = vec![
        ChatToolWorkspace {
            alias: "primary".to_owned(),
            display_name: "zeta-dev".to_owned(),
            grant_id: WorkspaceGrantId("secret-grant-id".to_owned()),
            permission_level: WorkspacePermissionLevel::ReadAndWrite,
        },
        ChatToolWorkspace {
            alias: "additional-1".to_owned(),
            display_name: "docs".to_owned(),
            grant_id: WorkspaceGrantId("another-secret-id".to_owned()),
            permission_level: WorkspacePermissionLevel::ReadOnly,
        },
    ];
    let definitions =
        chat_tool_definitions(&workspaces).map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(definitions.len(), 22);
    assert_eq!(definitions[0].name, "computation.run");
    let encoded = format!("{definitions:?}");
    assert!(encoded.contains("primary"));
    assert!(encoded.contains("additional-1"));
    assert!(!encoded.contains("zeta-dev"));
    assert!(!encoded.contains("docs"));
    assert!(!encoded.contains("secret-grant-id"));
    assert!(!encoded.contains("another-secret-id"));

    let rendered = model_runtime::render_model_prompt(
        model_runtime::ModelAdapterKind::GlimmerAtem,
        &model_runtime::PromptRenderRequest {
            messages: vec![model_runtime::PromptMessage::User {
                content: vec![model_runtime::PromptContent::Text(
                    "Inspect the project".to_owned(),
                )],
            }],
            tools: definitions,
            historical_tool_names: Vec::new(),
            current_date: "2026-08-16".to_owned(),
            reasoning_strength: model_runtime::ReasoningStrength::High,
            add_generation_prompt: true,
        },
    )?;
    assert!(rendered.contains("filesystem.read_text"));
    assert!(rendered.contains("filesystem.scan"));
    assert!(rendered.contains("filesystem.find_paths"));
    assert!(rendered.contains("filesystem.collect_evidence"));
    assert!(rendered.contains("filesystem.write_text"));
    assert!(rendered.contains("filesystem.replace_text"));
    assert!(rendered.contains("filesystem.create_directory"));
    assert!(rendered.contains("filesystem.write_delimited_table"));
    assert!(!rendered.contains("orchestration.transform_artifact"));
    assert!(rendered.contains("orchestration.submit_plan"));
    assert!(rendered.contains("orchestration.complete_stage"));
    assert!(!rendered.contains("orchestration.refine_stage"));
    assert!(rendered.contains("A successful verified result is authoritative"));
    Ok(())
}

#[test]
fn delimited_table_writer_is_flat_strict_and_keeps_evidence_host_owned() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    let writer = definitions
        .iter()
        .find(|definition| definition.name == "filesystem.write_delimited_table")
        .ok_or_else(IpcError::internal)?;
    let properties = writer.parameters["properties"]
        .as_object()
        .ok_or_else(IpcError::internal)?;
    assert!(!properties.contains_key("evidence"));
    assert!(!properties.contains_key("representation"));
    assert_eq!(writer.parameters["additionalProperties"], false);
    assert!(
        writer.parameters["required"]
            .as_array()
            .is_some_and(|required| [
                "workspace",
                "path",
                "expectedSha256",
                "delimiter",
                "columns",
                "rows"
            ]
            .iter()
            .all(|field| required.iter().any(|required| required == field)))
    );
    assert!(writer.description.contains("host binds"));
    assert!(writer.description.contains("Do not use it for JSON"));
    assert!(writer.description.contains("Canonical image-to-CSV"));
    assert!(
        writer
            .description
            .contains("after filesystem.inspect_image")
    );
    assert!(
        writer
            .description
            .contains("top-level native JSON `columns` and `rows` arrays")
    );
    assert!(writer.description.contains("outputs/example.csv"));
    Ok(())
}

#[test]
fn discovery_and_exact_path_tools_advertise_request_local_references() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    let discovery = definitions
        .iter()
        .find(|tool| tool.name == "filesystem.find_paths")
        .ok_or_else(IpcError::internal)?;
    assert!(
        discovery
            .description
            .contains("conversation-scoped `pathRef`")
    );

    let reader = definitions
        .iter()
        .find(|tool| tool.name == "filesystem.inspect_image")
        .ok_or_else(IpcError::internal)?;
    assert!(
        reader.parameters["properties"]["path"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("`pathRef` value"))
    );
    Ok(())
}

#[test]
fn model_visible_tool_contracts_do_not_embed_golden_fixture_names() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    let rendered = definitions
        .iter()
        .map(|definition| {
            format!(
                "{}\n{}\n{}",
                definition.name, definition.description, definition.parameters
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    for fixture_name in [
        "monthly cashflow",
        "annual cashflow",
        "outputs/reconstructed_tables",
    ] {
        assert!(
            !rendered.contains(fixture_name),
            "model-visible contract leaked golden fixture name `{fixture_name}`"
        );
    }
    Ok(())
}

#[test]
fn read_only_catalogue_leads_with_discovery_but_omits_mutation() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "read-only".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadOnly,
    }])?;
    assert_eq!(definitions.len(), 18);
    assert_eq!(definitions[0].name, "computation.run");
    assert!(definitions.iter().all(|tool| {
        !matches!(
            tool.name.as_str(),
            "filesystem.create_directory"
                | "filesystem.write_text"
                | "filesystem.replace_text"
                | "filesystem.write_delimited_table"
        )
    }));
    Ok(())
}

#[test]
fn projects_without_folders_receive_pure_computation_web_and_planning() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[])?;
    assert_eq!(
        definitions
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [
            "computation.run",
            "web.retrieve",
            "x402.request",
            "web.search",
            "orchestration.submit_plan",
            "orchestration.complete_stage",
            "orchestration.search_tools",
            "orchestration.recall",
            "orchestration.submit_forecast"
        ]
    );

    let plan = definitions
        .iter()
        .find(|definition| definition.name == "orchestration.submit_plan")
        .ok_or_else(IpcError::internal)?;
    assert!(
        plan.parameters["properties"]["steps"]["items"]["properties"]
            .get("preferredTools")
            .is_none()
    );
    let forecast = definitions
        .iter()
        .find(|definition| definition.name == "orchestration.submit_forecast")
        .ok_or_else(IpcError::internal)?;
    let forecastable =
            forecast.parameters["properties"]["entries"]["items"]["properties"]["routes"]["items"]
                ["oneOf"]
                .as_array()
                .ok_or_else(IpcError::internal)?
                .iter()
                .filter_map(|route| route.pointer("/properties/operation/const"))
                .cloned()
                .collect::<Vec<_>>();
    assert_eq!(
        forecastable,
        [
            serde_json::json!("computation.run"),
            serde_json::json!("web.retrieve"),
            serde_json::json!("x402.request"),
            serde_json::json!("web.search")
        ]
    );
    assert!(plan.description.contains("semantic_execution_v6 plan"));
    assert!(forecast.description.contains("advisory forecast"));
    assert!(
        plan.description
            .contains("accepted plan grants no permissions")
    );
    Ok(())
}

#[test]
fn forecast_routes_expose_operation_specific_argument_names() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadOnly,
    }])?;
    let forecast = definitions
        .iter()
        .find(|definition| definition.name == "orchestration.submit_forecast")
        .ok_or_else(IpcError::internal)?;
    let routes = forecast.parameters["properties"]["entries"]["items"]["properties"]["routes"]
            ["items"]["oneOf"]
            .as_array()
            .ok_or_else(IpcError::internal)?;
    let find_paths = routes
        .iter()
        .find(|route| {
            route.pointer("/properties/operation/const")
                == Some(&serde_json::json!("filesystem.find_paths"))
        })
        .ok_or_else(IpcError::internal)?;
    let arguments = find_paths["properties"]["unresolvedArguments"]["items"]["enum"]
        .as_array()
        .ok_or_else(IpcError::internal)?;
    assert_eq!(
        arguments,
        &[serde_json::json!("queries"), serde_json::json!("workspace")]
    );
    assert!(!arguments.iter().any(|argument| argument == "query"));
    assert!(!arguments.iter().any(|argument| argument == "pattern"));
    Ok(())
}

#[test]
fn image_contract_bounds_transcription_recheck_and_preserves_source_values() -> Result<(), IpcError>
{
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    let image = definitions
        .iter()
        .find(|definition| definition.name == "filesystem.inspect_image")
        .ok_or_else(IpcError::internal)?;
    assert!(
        image
            .description
            .contains("recheck only disputed cells once")
    );
    assert!(
        image
            .description
            .contains("preserve a clear inconsistent source value")
    );
    assert!(image.description.contains("structured consumer arguments"));
    assert!(
        image
            .description
            .contains("filesystem.write_delimited_table")
    );
    assert!(
        image
            .description
            .contains("top-level native JSON `columns` and `rows`")
    );
    assert!(image.description.contains("use filesystem.write_text"));
    Ok(())
}

#[test]
fn image_to_table_search_returns_the_actionable_canonical_route() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    let search = ParsedToolCall {
        call_id: "search-image-to-table".to_owned(),
        recipient: "orchestration.search_tools".to_owned(),
        name: "orchestration.search_tools".to_owned(),
        arguments: std::collections::BTreeMap::from([(
            "queries".to_owned(),
            serde_json::json!(["screenshot to table"]),
        )]),
    };

    let outcome = crate::chat_tools::search_tool_definitions(&search, &definitions)?;

    assert_eq!(
        outcome.loaded_names,
        [
            "filesystem.write_delimited_table",
            "filesystem.inspect_image"
        ]
    );
    let matches = outcome.value["matches"]
        .as_array()
        .ok_or_else(IpcError::internal)?;
    assert!(
        matches[0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("columns") && summary.contains("rows"))
    );
    assert!(
        matches[1]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("filesystem.write_delimited_table"))
    );
    Ok(())
}

#[test]
fn category_metadata_drives_provider_neutral_index_projection_and_complete_schemas()
-> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[ChatToolWorkspace {
        alias: "primary".to_owned(),
        display_name: "workspace".to_owned(),
        grant_id: WorkspaceGrantId("grant".to_owned()),
        permission_level: WorkspacePermissionLevel::ReadAndWrite,
    }])?;
    assert!(definitions.iter().all(|definition| {
        let Some(category) = tool_category(&definition.name) else {
            return false;
        };
        match definition
            .name
            .split_once('.')
            .map(|(namespace, _)| namespace)
        {
            Some("filesystem") => matches!(
                category,
                ToolCategory::WorkspaceDiscovery
                    | ToolCategory::WorkspaceRead
                    | ToolCategory::WorkspaceWrite
            ),
            Some("computation") => category == ToolCategory::Computation,
            Some("web") => category == ToolCategory::Web,
            Some("x402") => category == ToolCategory::Payments,
            Some("orchestration") => category == ToolCategory::Orchestration,
            _ => false,
        }
    }));
    assert!(definitions.iter().all(|definition| {
        !is_forecastable_chat_tool(&definition.name)
            || tool_action_class(&definition.name).is_some()
    }));
    assert_eq!(
        tool_action_class("x402.request"),
        Some(ToolActionClass::ExternalWrite)
    );
    let index = tool_category_index(&definitions);
    assert!(index.iter().all(|entry| !entry.responsibility().is_empty()));
    assert!(index.iter().any(|entry| {
        entry.name() == "workspace_read"
            && entry
                .operations()
                .iter()
                .any(|operation| operation == "filesystem.inspect_image")
    }));
    let forecast = definitions
        .iter()
        .find(|definition| definition.name == "orchestration.submit_forecast")
        .ok_or_else(IpcError::internal)?;
    assert_eq!(
        forecast.parameters["properties"]["entries"]["items"]["properties"]["category"]["enum"],
        serde_json::json!(forecastable_category_names(&definitions))
    );

    let read_projection = project_tool_definitions(
        &definitions,
        &std::collections::BTreeSet::from([ToolCategory::WorkspaceRead]),
        &std::collections::BTreeSet::new(),
    );
    let projected_names = read_projection
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(projected_names.contains("filesystem.read_text"));
    assert!(projected_names.contains("filesystem.inspect_image"));
    assert!(projected_names.contains("orchestration.submit_plan"));
    assert!(projected_names.contains("orchestration.submit_forecast"));
    assert!(!projected_names.contains("filesystem.write_text"));
    for projected in &read_projection {
        let original = definitions
            .iter()
            .find(|definition| definition.name == projected.name)
            .ok_or_else(IpcError::internal)?;
        assert_eq!(projected, original);
    }

    let write_projection = project_tool_definitions(
        &definitions,
        &std::collections::BTreeSet::from([ToolCategory::WorkspaceWrite]),
        &std::collections::BTreeSet::new(),
    );
    assert!(
        write_projection
            .iter()
            .any(|definition| definition.name == "filesystem.write_text")
    );
    assert!(
        !write_projection
            .iter()
            .any(|definition| definition.name == "filesystem.read_text")
    );

    let full_size = definitions
        .iter()
        .map(|definition| definition.description.len() + definition.parameters.to_string().len())
        .sum::<usize>();
    let projected_size = read_projection
        .iter()
        .map(|definition| definition.description.len() + definition.parameters.to_string().len())
        .sum::<usize>();
    assert!(projected_size.saturating_mul(4) < full_size.saturating_mul(3));
    Ok(())
}

#[test]
fn only_x402_purchase_is_a_terminal_one_shot_stage_result() {
    let arguments = |operation| {
        std::collections::BTreeMap::from([("operation".to_owned(), serde_json::json!(operation))])
    };
    assert!(!terminal_result_completes_stage(
        "x402.request",
        &arguments("discover"),
        &ToolOutcome::Completed {
            content: Vec::new()
        },
    ));
    assert!(terminal_result_completes_stage(
        "x402.request",
        &arguments("purchase"),
        &ToolOutcome::Completed {
            content: Vec::new()
        },
    ));
    let mut repairable = IpcError::invalid_request("fix resourceRef");
    repairable.retryable = true;
    assert!(!terminal_result_completes_stage(
        "x402.request",
        &arguments("purchase"),
        &ToolOutcome::Failed { error: repairable },
    ));
}

#[test]
fn x402_is_advertised_without_workspaces_including_release_builds() -> Result<(), IpcError> {
    let definitions = chat_tool_definitions(&[])?;
    let tool = definitions
        .iter()
        .find(|tool| tool.name == "x402.request")
        .ok_or_else(IpcError::internal)?;
    assert!(tool.description.contains("Base mainnet"));
    assert_eq!(tool.parameters["additionalProperties"], false);
    assert_eq!(
        tool.parameters["properties"]["operation"]["enum"],
        serde_json::json!(["discover", "purchase", "purchase_url"])
    );
    Ok(())
}
