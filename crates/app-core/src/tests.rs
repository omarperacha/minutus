use std::{
    error::Error,
    io::Cursor,
    sync::{Arc, Barrier},
    thread,
};

use app_storage::SCHEMA_VERSION;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use ipc_types::{
    Conversation, Message, MessageId, MessagePart, MessageRole, TaskActivityKind,
    TaskActivityStatus,
};
use tempfile::TempDir;

use super::*;

fn source_png() -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
        .write_to(&mut bytes, ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

fn create_test_project(app: &AppCore, timestamp: &str) -> Result<ProjectId, AppCoreError> {
    let project = Project {
        id: ProjectId("project-default".to_owned()),
        name: "Test project".to_owned(),
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
    };
    app.with_storage(|storage| storage.save_project(&project))?;
    Ok(project.id)
}

#[test]
fn new_chat_selection_covers_install_and_provider_state_matrix() {
    let glimmer = ModelSelection {
        model_id: "meta-models/muse-glimmer-30b".to_owned(),
        model_version: "g".to_owned(),
    };
    let qwen = ModelSelection {
        model_id: RECOMMENDED_MODEL_ID.to_owned(),
        model_version: "q".to_owned(),
    };

    assert_eq!(
        resolve_new_conversation_selection(
            None,
            &[(glimmer.clone(), false), (qwen.clone(), false)]
        ),
        None
    );
    assert_eq!(
        resolve_new_conversation_selection(None, &[(glimmer.clone(), true), (qwen.clone(), false)]),
        Some(glimmer.clone())
    );
    assert_eq!(
        resolve_new_conversation_selection(None, &[(glimmer.clone(), false), (qwen.clone(), true)]),
        Some(qwen.clone())
    );
    assert_eq!(
        resolve_new_conversation_selection(None, &[(glimmer.clone(), true), (qwen.clone(), true)]),
        Some(qwen.clone())
    );
    // An explicitly configured Qwen remains exact while its provider is
    // unavailable (represented here by no currently verified install).
    assert_eq!(
        resolve_new_conversation_selection(Some(&qwen), &[(glimmer, true), (qwen.clone(), false)]),
        Some(qwen)
    );
}

fn workspace_image_tool_conversation(asset_id: AssetId, timestamp: &str) -> Conversation {
    let conversation_id = ConversationId("workspace-image-chat".to_owned());
    let call_id = ipc_types::ToolCallId("inspect-image-call".to_owned());
    Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Inspect diagram".to_owned(),
        messages: vec![
            Message {
                id: MessageId("workspace-image-user".to_owned()),
                conversation_id: conversation_id.clone(),
                role: MessageRole::User,
                parts: vec![MessagePart::Text {
                    text: "Describe the diagram".to_owned(),
                }],
                created_at: timestamp.to_owned(),
            },
            Message {
                id: MessageId("workspace-image-assistant".to_owned()),
                conversation_id: conversation_id.clone(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::ToolCall {
                    request: ipc_types::ToolRequest {
                        call_id: call_id.clone(),
                        name: ToolName::FilesystemInspectImage,
                        arguments: std::collections::BTreeMap::from([
                            (
                                "path".to_owned(),
                                JsonValue::String("diagram.png".to_owned()),
                            ),
                            (
                                "workspace".to_owned(),
                                JsonValue::String("primary".to_owned()),
                            ),
                        ]),
                    },
                }],
                created_at: timestamp.to_owned(),
            },
            Message {
                id: MessageId("workspace-image-result".to_owned()),
                conversation_id: conversation_id.clone(),
                role: MessageRole::Tool,
                parts: vec![MessagePart::ToolResult {
                    result: ipc_types::ToolResult {
                        call_id,
                        outcome: ToolOutcome::Completed {
                            content: vec![ToolResultPart::Asset { asset_id }],
                        },
                    },
                }],
                created_at: timestamp.to_owned(),
            },
        ],
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
    }
}

mod assets;
mod conversation_generation;
mod execution_policy;
mod focus;
mod storage_and_workspaces;
