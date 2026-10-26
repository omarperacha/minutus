//! Cross-boundary runtime state: retrieval is not presentation or completion.

use super::*;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use ipc_types::{AssetId, Message, MessageId, ToolName, ToolResultPart};
use std::io::Cursor;

#[test]
fn image_exhaustion_keeps_input_without_spending_reinspection_or_advancing_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::TempDir::new()?;
    let core = AppCore::initialize(temp.path().join("Zeta"))?;
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
        .write_to(&mut png, ImageFormat::Png)?;
    let first = core.import_image(png.get_ref(), "2026-09-02T12:00:00Z")?.id;
    let alias = core.import_image(png.get_ref(), "2026-09-02T12:00:00Z")?.id;
    let tools = [
        "orchestration.submit_plan",
        "orchestration.complete_stage",
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .map(prompt_tool);
    let mut state = GenerationToolState::default();
    state
        .plan_policy
        .accept(&v6_file_plan(), &tools, 10)
        .map_err(|error| std::io::Error::other(error.message))?;
    state
        .operation_ledger
        .set_semantic_stage(state.plan_policy.current_stage_id());
    let mut conversation = empty_conversation("images");
    conversation.messages.push(Message {
        id: MessageId("user".to_owned()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::User,
        parts: vec![MessagePart::Text {
            text: "Transcribe the source into output.csv".to_owned(),
        }],
        created_at: conversation.created_at.clone(),
    });
    record_inspection(&mut state, &mut conversation, "inspect-first", &first);
    let selected = |state: &GenerationToolState, conversation: &Conversation| {
        let active = state.plan_policy.tools(&tools);
        let context = state.context(
            5,
            8,
            true,
            state.execution_profile(conversation, 1, false),
            &tools,
            &active,
        );
        core.projected_chat_image_ids(conversation, &context, &state.image_presentation_policy())
    };
    // Repeated preparation and an unsubmitted round do not consume the request.
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    state.record_image_presentation(&core, &[], &"b".repeat(64), 3)?;
    assert_eq!(state.operation_ledger.pending_image_ids().count(), 1);
    state.record_image_presentation(&core, std::slice::from_ref(&first), &"b".repeat(64), 4)?;
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    state.begin_outcome_recovery(GenerationCompletion::Length, std::slice::from_ref(&first));
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    assert_eq!(state.operation_ledger.pending_image_ids().count(), 0);
    assert_eq!(
        state.execution_profile(&conversation, 1, false).work,
        ExecutionWorkKind::Recovery
    );
    // Delivery under an alias satisfies pending recovery too, but never advances
    // the semantic cursor or makes the requested file outcome true.
    record_inspection(&mut state, &mut conversation, "inspect-again", &alias);
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    assert!(
        state
            .operation_ledger
            .current_stage_has_image_reinspection(std::slice::from_ref(&alias))
    );
    assert!(state.image_reinspection(&core, std::slice::from_ref(&first))?);
    state.record_image_presentation(&core, std::slice::from_ref(&first), &"b".repeat(64), 5)?;
    assert_eq!(selected(&state, &conversation)?, vec![first.clone()]);
    assert_eq!(state.operation_ledger.pending_image_ids().count(), 0);
    assert!(
        !state
            .rich_input_presentations
            .requests_recovery_for(&[first, alias])
    );
    assert_eq!(state.plan_policy.current_stage_id(), Some("extract"));
    assert!(!state.plan_policy.final_response_ready());
    // The model may acknowledge extraction without a transcript. Neither that
    // declaration nor a later focused stage removes the actual source.
    let complete = ParsedToolCall {
        call_id: "complete-source".to_owned(),
        recipient: "orchestration.complete_stage".to_owned(),
        name: "orchestration.complete_stage".to_owned(),
        arguments: BTreeMap::new(),
    };
    state
        .plan_policy
        .complete_advisory_stage(&complete)
        .map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(selected(&state, &conversation)?.len(), 1);
    assert_eq!(
        state.execution_profile(&conversation, 1, false).profile,
        agent_core::ExecutionProfile::Balanced
    );
    let counts = state.rich_input_presentations.counts();
    assert_eq!(counts.unique_sources, 1);
    assert_eq!(counts.presentations, 2);
    assert_eq!(counts.explicit_inspections, 2);
    revise_source_stage(&mut state, &tools)
        .map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(selected(&state, &conversation)?.len(), 1);
    Ok(())
}

fn revise_source_stage(
    state: &mut GenerationToolState,
    tools: &[PromptToolDefinition],
) -> Result<(), IpcError> {
    let mut revision = v6_file_plan();
    revision
        .arguments
        .insert("mode".to_owned(), serde_json::json!("revise"));
    let steps = revision
        .arguments
        .get_mut("steps")
        .ok_or_else(IpcError::internal)?;
    steps[0]["id"] = serde_json::json!("revised-source");
    steps[1]["dependsOn"] = serde_json::json!(["revised-source"]);
    state.plan_policy.accept(&revision, tools, 10)?;
    Ok(())
}

fn record_inspection(
    state: &mut GenerationToolState,
    conversation: &mut Conversation,
    call_id: &str,
    asset_id: &AssetId,
) {
    let call = ParsedToolCall {
        call_id: call_id.to_owned(),
        recipient: "filesystem.inspect_image".to_owned(),
        name: "filesystem.inspect_image".to_owned(),
        arguments: BTreeMap::new(),
    };
    let mut exchange = completed_tool_exchange(call_id, ToolName::FilesystemInspectImage);
    exchange.inspected_asset_id = Some(asset_id.clone());
    exchange.result.outcome = ToolOutcome::Completed {
        content: vec![ToolResultPart::Asset {
            asset_id: asset_id.clone(),
        }],
    };
    state.record_results(&[call], &[Ok(exchange.clone())]);
    for (role, part) in [
        (
            MessageRole::Assistant,
            MessagePart::ToolCall {
                request: exchange.request,
            },
        ),
        (
            MessageRole::Tool,
            MessagePart::ToolResult {
                result: exchange.result,
            },
        ),
    ] {
        conversation.messages.push(Message {
            id: MessageId(format!("message-{}", conversation.messages.len())),
            conversation_id: conversation.id.clone(),
            role,
            parts: vec![part],
            created_at: conversation.created_at.clone(),
        });
    }
}

#[test]
fn routine_reinspection_does_not_escalate_provider_thinking() -> Result<(), IpcError> {
    let tools = [
        "orchestration.submit_plan",
        "orchestration.complete_stage",
        "filesystem.inspect_image",
        "filesystem.write_text",
    ]
    .map(prompt_tool);
    let mut state = GenerationToolState::default();
    state.plan_policy.accept(&v6_file_plan(), &tools, 10)?;
    let conversation = empty_conversation("reinspection-profile");
    let first = state.execution_profile(&conversation, 1, false);
    let second = state.execution_profile(&conversation, 1, true);
    assert_eq!(first, second);
    assert_eq!(second.profile, agent_core::ExecutionProfile::Balanced);
    // Two successful task results trigger only advisory completion guidance.
    // They cannot downgrade visual work to a no-thinking checkpoint round.
    for id in ["read-one", "read-two"] {
        let call = ParsedToolCall {
            call_id: id.to_owned(),
            recipient: "filesystem.inspect_image".to_owned(),
            name: "filesystem.inspect_image".to_owned(),
            arguments: BTreeMap::new(),
        };
        state.record_results(
            &[call],
            &[Ok(completed_tool_exchange(
                id,
                ipc_types::ToolName::FilesystemInspectImage,
            ))],
        );
    }
    assert!(state.plan_policy.stage_completion_decision_due());
    assert_eq!(
        state.execution_profile(&conversation, 1, false).profile,
        agent_core::ExecutionProfile::Balanced
    );
    assert_eq!(
        state.execution_profile(&conversation, 0, false).profile,
        agent_core::ExecutionProfile::Latency
    );
    Ok(())
}
