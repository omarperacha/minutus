use super::*;

#[test]
fn duplicate_tool_detection_is_name_only_and_content_free() {
    assert!(!has_duplicate_tool_names(&[
        prompt_tool("filesystem.find_paths"),
        prompt_tool("filesystem.list"),
    ]));
    assert!(has_duplicate_tool_names(&[
        prompt_tool("filesystem.find_paths"),
        prompt_tool("filesystem.list"),
        prompt_tool("filesystem.find_paths"),
    ]));
}

#[test]
fn duplicate_tool_failure_has_a_precise_closed_subtype() {
    let error = AppCoreError::UnsupportedConversationContent;
    assert_eq!(
        prompt_preparation_failure_subtype(&error, true),
        "duplicate_tool_definition"
    );
    assert_eq!(
        prompt_preparation_failure_subtype(&error, false),
        "unsupported_conversation_content"
    );
}

#[test]
fn forecast_control_reaches_its_dedicated_dispatch_before_semantic_validation() {
    let forecast = ParsedToolCall {
        call_id: "forecast".to_owned(),
        recipient: FORECAST_TOOL.to_owned(),
        name: FORECAST_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    };
    assert!(uses_dedicated_forecast_dispatch(std::slice::from_ref(
        &forecast
    )));
    let operation = ParsedToolCall {
        name: "filesystem.find_paths".to_owned(),
        recipient: "filesystem.find_paths".to_owned(),
        ..forecast
    };
    assert!(!uses_dedicated_forecast_dispatch(&[operation]));
}

#[test]
fn stage_completion_control_reaches_its_dedicated_dispatch() {
    let completion = ParsedToolCall {
        call_id: "complete".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    };
    assert!(uses_dedicated_stage_completion_dispatch(
        std::slice::from_ref(&completion)
    ));
    let operation = ParsedToolCall {
        name: "filesystem.read_text".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        ..completion.clone()
    };
    assert!(!uses_dedicated_stage_completion_dispatch(
        std::slice::from_ref(&operation)
    ));
    assert!(uses_dedicated_stage_completion_dispatch(&[
        operation.clone(),
        completion.clone()
    ]));
    assert!(uses_dedicated_stage_completion_dispatch(&[
        operation, completion,
    ]));
}

#[test]
fn stage_completion_position_structurally_segments_one_batch() {
    let task = ParsedToolCall {
        call_id: "task".to_owned(),
        recipient: "filesystem.read_text".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: BTreeMap::new(),
    };
    let completion = ParsedToolCall {
        call_id: "complete".to_owned(),
        recipient: COMPLETE_STAGE_TOOL.to_owned(),
        name: COMPLETE_STAGE_TOOL.to_owned(),
        arguments: BTreeMap::new(),
    };
    let calls = [task.clone(), completion.clone(), task];
    let (index, boundary) = stage_completion_boundary(&calls).unwrap_or((usize::MAX, &calls[0]));
    assert_eq!(index, 1);
    assert_eq!(boundary.call_id, "complete");
    assert!(stage_completion_boundary(&[completion.clone(), completion]).is_err());
}

#[test]
fn development_stream_rejection_trace_is_closed_and_content_free()
-> Result<(), Box<dyn std::error::Error>> {
    let conversation = Conversation {
        id: ipc_types::ConversationId("conversation".to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "title".to_owned(),
        messages: Vec::new(),
        created_at: "2026-08-19T00:00:00Z".to_owned(),
        updated_at: "2026-08-19T00:00:00Z".to_owned(),
    };
    let value = model_stream_rejection_trace_data(
        &RequestId("request".to_owned()),
        &conversation,
        2,
        ModelOutputFailureKind::MalformedToolCall,
    );
    let Some(object) = value.as_object() else {
        return Err(std::io::Error::other("trace event must be an object").into());
    };
    assert_eq!(
        object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["conversationId", "event", "requestId", "round", "subtype"])
    );
    assert_eq!(value["event"], "model_stream_rejected");
    assert_eq!(value["subtype"], "malformed_tool_call");
    assert!(!value.to_string().contains("output"));
    Ok(())
}

#[test]
fn model_round_trace_reports_shape_and_limit_without_model_content() {
    let conversation = Conversation {
        id: ipc_types::ConversationId("conversation".to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "title".to_owned(),
        messages: Vec::new(),
        created_at: "2026-08-19T00:00:00Z".to_owned(),
        updated_at: "2026-08-19T00:00:00Z".to_owned(),
    };
    let generated = ChatGenerationRound {
        reasoning: "private reasoning".to_owned(),
        content: "private answer".to_owned(),
        calls: Vec::new(),
        reason: GenerationCompletion::Length,
        image_ids: Vec::new(),
        binding_sha256: "a".repeat(64),
        timing: RoundTiming::default(),
    };
    let admission = RoundAdmissionTelemetry {
        model_id: "qwen-3.8".to_owned(),
        model_version: "1".to_owned(),
        adapter: "qwen_native",
        provider: "ollama",
        context_tokens: 65_536,
        output_tokens: 8_192,
        admitted_prompt_tokens: 1_024,
        eligible_prefix_tokens: Some(768),
        raw_shared_prefix_tokens: Some(768),
        first_user_boundary_tokens: Some(512),
        active_tool_count: 4,
        active_tool_contract_bytes: 2_048,
        image_count: 1,
        execution_profile: "deep",
        execution_work: "evidence",
        policy_version: 1,
        bounded_stage_action: false,
        reasoning_strength: "high",
        profile_output_tokens: 8_192,
        reasoning_reserve_tokens: 2_048,
        final_answer_output_override: false,
        rich_evidence_checkpoint_reused: true,
        selected_history_messages: 5,
        compacted_history_messages: 2,
        dialogue_checkpoint_advances: 0,
        narrative_checkpoint: "reused",
        rich_input_admission: RichInputAdmission::Reinspection,
    };
    let value = model_round_completed_trace_data(
        &RequestId("request".to_owned()),
        &conversation,
        3,
        &generated,
        &admission,
        Some(GenerationMetrics {
            prompt_tokens: Some(1_024),
            cached_prompt_tokens: Some(768),
            cache_write_prompt_tokens: None,
            generated_tokens: Some(300),
            prompt_evaluation_ns: Some(2_000_000_000),
            generation_ns: Some(3_000_000_000),
            total_ns: Some(5_500_000_000),
            load_ns: None,
            peak_memory_bytes: None,
        }),
        125,
        40,
        70,
        15,
        750,
        42,
        Some(200),
        Some(100),
    );

    assert_eq!(value["completionReason"], "length");
    assert_eq!(value["toolCallCount"], 0);
    assert_eq!(value["outputTokenLimit"], 8_192);
    assert_eq!(value["modelAdmissionMs"], 40);
    assert_eq!(value["providerPromptRenderMs"], 70);
    assert_eq!(value["exactTokenizationMs"], 15);
    assert_eq!(value["executionProfile"], "deep");
    assert_eq!(value["executionProfilePolicyVersion"], 1);
    assert_eq!(value["reasoningReserveTokens"], 2_048);
    assert_eq!(value["finalAnswerOutputOverride"], false);
    assert_eq!(value["richEvidenceCheckpointReused"], true);
    assert_eq!(value["richInputReinspection"], true);
    assert_eq!(value["admittedPromptTokens"], 1_024);
    assert_round_prefix_telemetry(&value);
    assert_eq!(value["reasoningTokens"], 200);
    assert_eq!(value["finalOutputTokens"], 100);
    assert_eq!(value["promptEvaluationMs"], 2_000);
    assert_eq!(value["modelGenerationMs"], 3_000);
    assert_eq!(value["providerOverheadMs"], 500);
    assert_eq!(value["outputTokenMeasurementMs"], 42);
    assert_eq!(value["peakMemoryBytes"], serde_json::Value::Null);
    let serialized = value.to_string();
    assert!(!serialized.contains("private reasoning"));
    assert!(!serialized.contains("private answer"));
}

fn assert_round_prefix_telemetry(value: &serde_json::Value) {
    assert_eq!(value["eligiblePrefixTokens"], 768);
    assert_eq!(value["rawSharedPrefixTokens"], 768);
    assert_eq!(value["firstUserBoundaryTokens"], 512);
    assert_eq!(value["providerPromptTokens"], 1_024);
    assert_eq!(value["providerCachedPromptTokens"], 768);
    assert!(value["providerCacheWritePromptTokens"].is_null());
}

#[test]
fn request_telemetry_uses_verified_digest_counts_not_asset_ids() {
    let mut telemetry = RequestExecutionTelemetry::new();
    let round = |image_ids| ChatGenerationRound {
        reasoning: "bounded extraction".to_owned(),
        content: String::new(),
        calls: Vec::new(),
        reason: GenerationCompletion::Stop,
        image_ids,
        binding_sha256: "a".repeat(64),
        timing: RoundTiming::default(),
    };
    telemetry.record_round(&round(vec![ipc_types::AssetId("asset".to_owned())]));
    telemetry.record_round(&round(Vec::new()));
    telemetry.record_image_counts(crate::chat_runtime::rich_input::RichInputCounts {
        unique_sources: 1,
        presentations: 1,
        explicit_inspections: 1,
        retained_presentations: 0,
    });
    assert_eq!(telemetry.submitted_images, 1);
    assert_eq!(telemetry.repeated_rich_inputs, 0);

    telemetry.record_round(&round(vec![ipc_types::AssetId("alias".to_owned())]));
    telemetry.record_image_counts(crate::chat_runtime::rich_input::RichInputCounts {
        unique_sources: 1,
        presentations: 2,
        explicit_inspections: 1,
        retained_presentations: 1,
    });
    assert_eq!(telemetry.submitted_images, 2);
    assert_eq!(telemetry.repeated_rich_inputs, 1);
}

#[test]
fn request_mutation_timing_excludes_directory_creation() {
    let completed = |name: ipc_types::ToolName| {
        Ok(ChatToolExchange {
            request: ipc_types::ToolRequest {
                call_id: ipc_types::ToolCallId("call".to_owned()),
                name,
                arguments: BTreeMap::new(),
            },
            result: ipc_types::ToolResult {
                call_id: ipc_types::ToolCallId("call".to_owned()),
                outcome: ToolOutcome::Completed {
                    content: Vec::new(),
                },
            },
            activity: ipc_types::TaskActivity {
                id: "call".to_owned(),
                kind: ipc_types::TaskActivityKind::FileWrite,
                title: "Apply operation".to_owned(),
                detail: None,
                status: ipc_types::TaskActivityStatus::Completed,
            },
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        })
    };
    let mut telemetry = RequestExecutionTelemetry::new();
    telemetry.record_exchanges(&[completed(ipc_types::ToolName::FilesystemCreateDirectory)]);
    assert_eq!(telemetry.first_mutation_ms, None);

    telemetry.record_exchanges(&[completed(ipc_types::ToolName::FilesystemWriteText)]);
    assert!(telemetry.first_mutation_ms.is_some());
}
