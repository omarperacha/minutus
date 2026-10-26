use std::collections::{BTreeMap, BTreeSet};

use super::*;
use super::{
    budget::tool_batch_within_limits,
    dispatch::{current_reads_can_survive_rejected_plan, stage_completion_boundary},
    recovery_flow::{
        final_response_tool_batch_repairable, is_direct_discovery_call_batch,
        phase_call_availability, prospective_plan_tools, tool_argument_repair_allowed,
        valid_plan_prefixed_operation_repair_allowed,
    },
    state::{constrain_action_profile, execution_prefix_sha256},
    telemetry::{
        model_round_completed_trace_data, model_stream_rejection_trace_data, tool_capabilities,
    },
};

fn prompt_tool(name: &str) -> PromptToolDefinition {
    PromptToolDefinition {
        name: name.to_owned(),
        description: String::new(),
        parameters: serde_json::json!({}),
    }
}

fn parsed_call(id: &str, name: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: id.to_owned(),
        recipient: name.to_owned(),
        name: name.to_owned(),
        arguments: BTreeMap::new(),
    }
}

fn empty_conversation(id: &str) -> Conversation {
    Conversation {
        id: ipc_types::ConversationId(id.to_owned()),
        project_id: ipc_types::ProjectId("project".to_owned()),
        title: "title".to_owned(),
        messages: Vec::new(),
        created_at: "2026-08-31T00:00:00Z".to_owned(),
        updated_at: "2026-08-31T00:00:00Z".to_owned(),
    }
}

fn v6_file_plan() -> ParsedToolCall {
    ParsedToolCall {
        call_id: "plan".to_owned(),
        recipient: "orchestration.submit_plan".to_owned(),
        name: "orchestration.submit_plan".to_owned(),
        arguments: BTreeMap::from([
            ("assumptions".to_owned(), serde_json::json!([])),
            (
                "constraints".to_owned(),
                serde_json::json!(["Preserve source values"]),
            ),
            (
                "contractVersion".to_owned(),
                serde_json::json!(crate::chat_tools::SEMANTIC_PLAN_CONTRACT_VERSION),
            ),
            ("mode".to_owned(), serde_json::json!("initial")),
            ("outcome".to_owned(), serde_json::json!("create_file")),
            (
                "steps".to_owned(),
                serde_json::json!([
                    {
                        "actionClass":"read", "dependsOn":[],
                        "doneWhen":"The source content is available", "id":"extract",
                        "objective":"Extract the source content",
                        "suggestedTools":["filesystem.inspect_image"]
                    },
                    {
                        "actionClass":"workspace_write", "dependsOn":["extract"],
                        "doneWhen":"The requested file is verified", "id":"write",
                        "objective":"Create the requested file",
                        "suggestedTools":["filesystem.write_text"]
                    }
                ]),
            ),
            (
                "successCriteria".to_owned(),
                serde_json::json!(["The requested file is created"]),
            ),
            ("target".to_owned(), serde_json::json!("result/output.csv")),
        ]),
    }
}

fn completed_tool_exchange(id: &str, name: ipc_types::ToolName) -> ChatToolExchange {
    let call_id = ipc_types::ToolCallId(id.to_owned());
    let content = if name == ipc_types::ToolName::FilesystemWriteText {
        vec![ipc_types::ToolResultPart::Json {
            value: ipc_types::JsonValue::Object(BTreeMap::from([
                (
                    "changes".to_owned(),
                    ipc_types::JsonValue::Array(vec![ipc_types::JsonValue::Object(
                        BTreeMap::from([(
                            "postWriteVerified".to_owned(),
                            ipc_types::JsonValue::Bool(true),
                        )]),
                    )]),
                ),
                (
                    "postconditionVerified".to_owned(),
                    ipc_types::JsonValue::Bool(true),
                ),
            ])),
        }]
    } else {
        Vec::new()
    };
    ChatToolExchange {
        request: ipc_types::ToolRequest {
            call_id: call_id.clone(),
            name,
            arguments: BTreeMap::new(),
        },
        result: ipc_types::ToolResult {
            call_id,
            outcome: ToolOutcome::Completed { content },
        },
        activity: ipc_types::TaskActivity {
            id: id.to_owned(),
            kind: ipc_types::TaskActivityKind::FileWrite,
            title: "operation".to_owned(),
            status: ipc_types::TaskActivityStatus::Completed,
            detail: None,
        },
        workspace_grant_id: Some(ipc_types::WorkspaceGrantId("grant".to_owned())),
        undo_changes: Vec::new(),
        inspected_asset_id: None,
        computation_reuse_digest: None,
    }
}

#[test]
fn consumed_x402_attempt_is_removed_from_the_request_local_tool_surface() -> Result<(), IpcError> {
    let x402_name = "x402.request";
    let all_tools = vec![prompt_tool(x402_name), prompt_tool("web.retrieve")];
    let mut call = parsed_call("x402-first", x402_name);
    call.arguments
        .insert("operation".to_owned(), serde_json::json!("purchase"));
    let exchange = completed_tool_exchange("x402-first", ipc_types::ToolName::X402Request);
    let mut state = GenerationToolState::default();

    assert!(
        state
            .projected_tools(&all_tools)
            .iter()
            .any(|tool| tool.name == x402_name)
    );

    state.record_results(&[call], &[Ok(exchange)]);

    let projected = state.projected_tools(&all_tools);
    assert!(state.operation_ledger.x402_attempt_consumed());
    assert!(!projected.iter().any(|tool| tool.name == x402_name));
    assert!(projected.iter().any(|tool| tool.name == "web.retrieve"));
    assert!(state.provider_prefix_boundary().x402_attempt_consumed);

    let search = ParsedToolCall {
        call_id: "find-consumed-x402".to_owned(),
        recipient: SEARCH_TOOLS_TOOL.to_owned(),
        name: SEARCH_TOOLS_TOOL.to_owned(),
        arguments: BTreeMap::from([("queries".to_owned(), serde_json::json!([x402_name]))]),
    };
    let outcome = search_tool_definitions(&search, &projected)?;
    assert!(outcome.loaded_names.is_empty());
    Ok(())
}

#[test]
fn x402_availability_is_independent_of_workspace_mutation_availability() {
    let all_tools = vec![prompt_tool("x402.request"), prompt_tool("web.retrieve")];
    let (workspace_mutation_tools_available, suggested_execution_reserve) =
        tool_capabilities(&all_tools);
    let state = GenerationToolState::default();
    let projected = state.projected_tools(&all_tools);

    assert!(!workspace_mutation_tools_available);
    assert_eq!(suggested_execution_reserve, 2);
    assert!(projected.iter().any(|tool| tool.name == "x402.request"));
}

mod budget;
mod completion;
mod dispatch;
mod image_recovery;
mod plan_repair;
mod provider_prefix;
mod recovery;
mod startup_recall;
mod telemetry;

#[test]
fn direct_web_route_is_bounded_and_uses_the_latency_profile() {
    let all = [
        prompt_tool("filesystem.read_text"),
        prompt_tool("web.retrieve"),
        prompt_tool("web.search"),
        prompt_tool("orchestration.submit_plan"),
    ];
    let state = GenerationToolState::default();
    let tools = state.plan_policy.tools(&all);
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["web.retrieve", "web.search", "orchestration.submit_plan"]
    );
    assert_eq!(
        state
            .execution_profile(&empty_conversation("direct-web"), 0, false)
            .profile,
        agent_core::ExecutionProfile::Latency
    );
    assert!(is_direct_discovery_call_batch(&[
        parsed_call("search", "web.search"),
        parsed_call("retrieve", "web.retrieve"),
    ]));
    assert!(!is_direct_discovery_call_batch(&[parsed_call(
        "read",
        "filesystem.read_text"
    )]));
}

#[test]
fn semantic_escape_retains_successful_direct_web_evidence() -> Result<(), IpcError> {
    let all = [
        prompt_tool("web.search"),
        prompt_tool("orchestration.submit_plan"),
        prompt_tool(COMPLETE_STAGE_TOOL),
        prompt_tool(SEARCH_TOOLS_TOOL),
        prompt_tool("filesystem.inspect_image"),
        prompt_tool("filesystem.write_text"),
    ];
    let mut state = GenerationToolState::default();
    let web_call = parsed_call("web-evidence", "web.search");
    state.record_results(
        std::slice::from_ref(&web_call),
        &[Ok(completed_tool_exchange(
            "web-evidence",
            ipc_types::ToolName::WebSearch,
        ))],
    );
    state.plan_policy.accept(&v6_file_plan(), &all, 12)?;
    state.retain_preplan_results();
    assert!(state.plan_policy.retains_context_result("web-evidence"));
    Ok(())
}

#[test]
fn explicit_effort_changes_the_round_policy_without_changing_work_classification() {
    let automatic = ExecutionProfileDecision {
        profile: ExecutionProfile::Balanced,
        work: ExecutionWorkKind::Evidence,
        policy_version: agent_core::EXECUTION_PROFILE_POLICY_VERSION,
        bounded_action: false,
        bulk_arguments: false,
    };
    for (effort, expected) in [
        (ipc_types::ChatEffort::Quick, ExecutionProfile::Latency),
        (ipc_types::ChatEffort::Balanced, ExecutionProfile::Balanced),
        (ipc_types::ChatEffort::Deep, ExecutionProfile::Deep),
    ] {
        let selected = ChatGenerationSettings {
            context_tokens: None,
            effort: Some(effort),
        }
        .apply_effort(automatic);
        assert_eq!(selected.profile, expected);
        assert_eq!(selected.work, automatic.work);
    }
    assert_eq!(
        ChatGenerationSettings::default().apply_effort(automatic),
        automatic
    );
}
