use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
};

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use ipc_types::{
    AssetId, Conversation, ConversationId, Message, MessageId, MessagePart, MessageRole, ProjectId,
    ToolCallId, ToolName, ToolOutcome, ToolRequest, ToolResult, ToolResultPart,
};

use super::{ResolvedRichInputPresentationPolicy, focused_phase_messages, prompt_message};
use crate::{
    AppCore, RichInputPresentationPolicy, ToolOperationBudget, ToolOrchestrationContext,
    ToolOrchestrationPhase,
};
use tempfile::TempDir;

fn message(id: &str, role: MessageRole, parts: Vec<MessagePart>) -> Message {
    Message {
        id: MessageId(id.to_owned()),
        conversation_id: ConversationId("conversation".to_owned()),
        role,
        parts,
        created_at: "2026-08-30T12:00:00Z".to_owned(),
    }
}

fn completed_json_result_message(id: &str, call_id: &str) -> Message {
    message(
        id,
        MessageRole::Tool,
        vec![MessagePart::ToolResult {
            result: ToolResult {
                call_id: ToolCallId(call_id.to_owned()),
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Json {
                        value: ipc_types::JsonValue::Object(BTreeMap::from([(
                            "loaded".to_owned(),
                            ipc_types::JsonValue::Bool(true),
                        )])),
                    }],
                },
            },
        }],
    )
}

fn png() -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, Rgb([1, 2, 3])))
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
    Ok(bytes)
}

#[test]
fn presentation_policy_suppresses_later_bytes_without_a_checkpoint()
-> Result<(), crate::AppCoreError> {
    let asset_id = AssetId("asset-source".to_owned());
    let user = message(
        "user",
        MessageRole::User,
        vec![MessagePart::Asset {
            asset_id: asset_id.clone(),
        }],
    );
    let mut first_assets = Vec::new();
    let _ = prompt_message(
        &user,
        &mut first_assets,
        &ResolvedRichInputPresentationPolicy::exact_for_test([]),
        None,
    )?;
    assert_eq!(first_assets.as_slice(), std::slice::from_ref(&asset_id));

    let mut later_assets = Vec::new();
    let _ = prompt_message(
        &user,
        &mut later_assets,
        &ResolvedRichInputPresentationPolicy::exact_for_test([asset_id]),
        None,
    )?;
    assert!(later_assets.is_empty());
    Ok(())
}

#[test]
fn profile_projection_counts_only_images_not_already_presented_in_this_request()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = TempDir::new()?;
    let app = AppCore::initialize(temporary.path().join("Zeta"))?;
    let asset_id = app.import_image(&png()?, "2026-08-30T12:00:00Z")?.id;
    let identity = app.rich_input_identity(&asset_id)?;
    let conversation = Conversation {
        id: ConversationId("conversation".to_owned()),
        project_id: ProjectId("project".to_owned()),
        title: "Image task".to_owned(),
        messages: vec![message(
            "user",
            MessageRole::User,
            vec![MessagePart::Asset {
                asset_id: asset_id.clone(),
            }],
        )],
        created_at: "2026-08-30T12:00:00Z".to_owned(),
        updated_at: "2026-08-30T12:00:00Z".to_owned(),
    };
    let orchestration = ToolOrchestrationContext::new(
        ToolOperationBudget::new(1, 11, 0, 18, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    );

    assert_eq!(
        app.projected_chat_image_count(
            &conversation,
            &orchestration,
            &RichInputPresentationPolicy::default(),
        )?,
        1
    );
    assert_eq!(
        app.projected_chat_image_count(
            &conversation,
            &orchestration,
            &RichInputPresentationPolicy::from_verified([(
                asset_id,
                identity.content_sha256().to_owned(),
                "b".repeat(64),
            )]),
        )?,
        0
    );
    Ok(())
}

#[test]
fn focused_history_keeps_image_continuation_and_its_completed_tool_pair() {
    let inspect_id = ToolCallId("inspect-call".to_owned());
    let later_id = ToolCallId("later-call".to_owned());
    let conversation = Conversation {
        id: ConversationId("conversation".to_owned()),
        project_id: ProjectId("project".to_owned()),
        title: "Image task".to_owned(),
        messages: vec![
            message(
                "user",
                MessageRole::User,
                vec![MessagePart::Text {
                    text: "Transcribe the image".to_owned(),
                }],
            ),
            message(
                "inspect-request",
                MessageRole::Assistant,
                vec![MessagePart::ToolCall {
                    request: ToolRequest {
                        call_id: inspect_id.clone(),
                        name: ToolName::FilesystemInspectImage,
                        arguments: BTreeMap::new(),
                    },
                }],
            ),
            message(
                "inspect-result",
                MessageRole::Tool,
                vec![MessagePart::ToolResult {
                    result: ToolResult {
                        call_id: inspect_id.clone(),
                        outcome: ToolOutcome::Completed {
                            content: vec![ToolResultPart::Asset {
                                asset_id: AssetId("inspected-image".to_owned()),
                            }],
                        },
                    },
                }],
            ),
            message(
                "image-continuation",
                MessageRole::Assistant,
                vec![
                    MessagePart::Reasoning {
                        text: "Untrusted extracted rows with one uncertainty".to_owned(),
                    },
                    MessagePart::Text {
                        text: "The image remains legible.".to_owned(),
                    },
                    MessagePart::ToolCall {
                        request: ToolRequest {
                            call_id: later_id,
                            name: ToolName::OrchestrationCompleteStage,
                            arguments: BTreeMap::new(),
                        },
                    },
                ],
            ),
            completed_json_result_message("later-result", "later-call"),
        ],
        created_at: "2026-08-30T12:00:00Z".to_owned(),
        updated_at: "2026-08-30T12:00:00Z".to_owned(),
    };

    let focused = focused_phase_messages(
        &conversation,
        &BTreeSet::from([inspect_id.0.clone()]),
        0..conversation.messages.len(),
    );
    assert!(focused.iter().any(|message| {
        message.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::Reasoning { text } if text.contains("Untrusted extracted rows")
            )
        })
    }));
    assert!(focused.iter().any(|message| {
        message.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::ToolCall { request } if request.call_id.0 == "later-call"
            )
        })
    }));
    assert!(focused.iter().any(|message| {
        message.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::ToolResult { result } if result.call_id.0 == "later-call"
            )
        })
    }));
}

#[test]
fn focused_dialogue_keeps_multiple_answers_corrections_and_native_pairs_in_order() {
    let text =
        |id, role, value: &str| message(id, role, vec![MessagePart::Text { text: value.into() }]);
    let call = |id: &str| MessagePart::ToolCall {
        request: ToolRequest {
            call_id: ToolCallId(id.into()),
            name: ToolName::WebSearch,
            arguments: BTreeMap::new(),
        },
    };
    let conversation = Conversation {
        id: ConversationId("conversation".into()),
        project_id: ProjectId("project".into()),
        title: "Task".into(),
        messages: vec![
            text(
                "request",
                MessageRole::User,
                "Create report at https://example.org/report",
            ),
            text("ask-a", MessageRole::Assistant, "Which account?"),
            text("a", MessageRole::User, "account=A-001"),
            text("ask-b", MessageRole::Assistant, "Which delivery address?"),
            text("b", MessageRole::User, "delivery=B-002"),
            message(
                "old-call",
                MessageRole::Assistant,
                vec![
                    MessagePart::Reasoning {
                        text: "OLD_PRIVATE".into(),
                    },
                    call("old"),
                ],
            ),
            completed_json_result_message("old-result", "old"),
            text(
                "correction",
                MessageRole::User,
                "Actually delivery=B-003; use the account above",
            ),
            message(
                "batch",
                MessageRole::Assistant,
                vec![
                    MessagePart::Reasoning {
                        text: "NATIVE_CONTINUATION".into(),
                    },
                    MessagePart::Text {
                        text: "Checking both sources.".into(),
                    },
                    call("first"),
                    call("second"),
                ],
            ),
            completed_json_result_message("first-result", "first"),
            completed_json_result_message("second-result", "second"),
        ],
        created_at: "2026-09-10T12:00:00Z".into(),
        updated_at: "2026-09-10T12:00:00Z".into(),
    };
    let selected = BTreeSet::from(["old".into(), "first".into()]);
    let history = focused_phase_messages(&conversation, &selected, 0..conversation.messages.len());
    assert_eq!(
        history.iter().map(|m| &m.id).collect::<Vec<_>>(),
        conversation
            .messages
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>()
    );
    let encoded = serde_json::to_string(&history).unwrap_or_default();
    for value in [
        "A-001",
        "B-002",
        "B-003",
        "account above",
        "Which account?",
        "Which delivery address?",
        "https://example.org/report",
        "second",
        "NATIVE_CONTINUATION",
    ] {
        assert!(encoded.contains(value), "missing {value}");
    }
    assert!(!encoded.contains("OLD_PRIVATE"));
    // References may select a pair before the retained dialogue boundary.
    let clipped = focused_phase_messages(&conversation, &selected, 7..conversation.messages.len());
    assert_eq!(clipped[0].id.0, "old-call");
    assert_eq!(clipped[1].id.0, "old-result");
    assert_eq!(clipped[2].id.0, "correction");
}
