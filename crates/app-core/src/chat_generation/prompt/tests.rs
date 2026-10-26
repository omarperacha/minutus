//! Regression across focused history, digest policy and native provider JSON.

use super::*;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use model_runtime::{OllamaProvider, OllamaProviderConfig, QwenAdapter, VerifiedOllamaModel};
use std::{
    io::{Cursor, Read, Write},
    net::TcpListener,
};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const NOW: &str = "2026-09-02T12:00:00Z";

pub(super) fn verified_model() -> TestResult<VerifiedOllamaModel> {
    // Exercise the public verification boundary; no test-only authority bypass.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(listener.local_addr()?)?)?;
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        for body in [
            serde_json::json!({"version":model_runtime::PINNED_OLLAMA_VERSION}),
            serde_json::json!({"models":[{"name":"test-qwen", "digest":"a".repeat(64)}]}),
        ] {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte)?;
                request.push(byte[0]);
            }
            let body = body.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )?;
        }
        Ok(())
    });
    let model = provider.verify_registered_tag("test-qwen")?;
    server
        .join()
        .map_err(|_| "verification server panicked")??;
    Ok(model)
}

fn append_inspection(conversation: &mut Conversation, call: &str, asset_id: &AssetId) {
    for (role, part) in [
        (
            MessageRole::Assistant,
            MessagePart::ToolCall {
                request: ipc_types::ToolRequest {
                    call_id: ipc_types::ToolCallId(call.to_owned()),
                    name: ToolName::FilesystemInspectImage,
                    arguments: BTreeMap::new(),
                },
            },
        ),
        (
            MessageRole::Tool,
            MessagePart::ToolResult {
                result: ipc_types::ToolResult {
                    call_id: ipc_types::ToolCallId(call.to_owned()),
                    outcome: ToolOutcome::Completed {
                        content: vec![
                            ToolResultPart::Asset {
                                asset_id: asset_id.clone(),
                            },
                            ToolResultPart::Asset {
                                asset_id: asset_id.clone(),
                            },
                        ],
                    },
                },
            },
        ),
    ] {
        conversation.messages.push(Message {
            id: MessageId(format!("message-{}", conversation.messages.len())),
            conversation_id: conversation.id.clone(),
            role,
            parts: vec![part],
            created_at: NOW.to_owned(),
        });
    }
}

fn orchestration(call: &str) -> ToolOrchestrationContext {
    ToolOrchestrationContext::new(
        ToolOperationBudget::new(5, 7, 4, 14, 0, 2),
        true,
        ToolOrchestrationPhase::Execution,
    )
    .with_plan_progress(Some(
        ToolPlanProgress::new(
            1,
            0,
            Vec::new(),
            None,
            ToolPlanOutcome::CreateFile,
            false,
            false,
            2,
            true,
            0,
        )
        .with_retained_call_ids([call.to_owned()]),
    ))
}

fn native_images(
    app: &AppCore,
    model: &VerifiedOllamaModel,
    conversation: &Conversation,
    policy: &RichInputPresentationPolicy,
    expanded: bool,
) -> TestResult<Vec<String>> {
    // Deliberately focus a newer unrelated result: pending images must bring
    // their own complete tool pair back into the selected history.
    let orchestration = orchestration("search-after-image");
    let projected = app.projected_chat_image_ids(conversation, &orchestration, policy)?;
    let working = working_context::derive(conversation)?;
    let mut focus = orchestration.focused_history_call_ids();
    policy.extend_focus(conversation, &mut focus);
    let history = focused_phase_messages(
        conversation,
        &focus.unwrap_or_default(),
        0..conversation.messages.len(),
    );
    let resolved =
        policy.resolve_for_candidates(app, &"b".repeat(64), message_asset_ids(&history))?;
    let tools = if expanded {
        vec![PromptToolDefinition {
            name: "filesystem.write_text".to_owned(),
            description: "Write the transcribed file".to_owned(),
            parameters: serde_json::json!({"type":"object", "properties": {"content":{"type":"string"}}}),
        }]
    } else {
        Vec::new()
    };
    let (request, ids) = AppCore::build_working_chat_request(
        conversation,
        WorkingPromptAdapterContext {
            current_date: "2026-09-02",
            tools,
            adapter: ModelAdapterKind::QwenNative,
            reasoning_strength: ReasoningStrength::Low,
        },
        WorkingPromptContext {
            working: &working,
            summary: None,
            ledger: None,
            orchestration: Some(orchestration),
            presentation: &resolved,
            rich_evidence: None,
            compact_completed_exchanges: false,
            output_token_limit: None,
        },
    )?;
    assert_eq!(ids, projected, "profile and actual prompt must agree");
    let bytes = ids
        .iter()
        .map(|id| app.inference_asset(id))
        .collect::<Result<Vec<_>, _>>()?;
    let native = QwenAdapter.render_with_context(model, &request, &bytes, 16384, 2048)?;
    let json = serde_json::to_value(native)?;
    if !ids.is_empty() {
        let rendered = json.to_string();
        assert!(rendered.contains("a duplicate prose transcription is not required"));
        assert!(rendered.contains("may be called alone"));
        assert!(
            !rendered.contains("Do not end an image-derived stage with a bare completion call")
        );
    }
    assert!(!json.to_string().contains("discarded-exhausted-reasoning"));
    if expanded {
        assert!(json.to_string().contains("filesystem.write_text"));
    }
    Ok(json["messages"]
        .as_array()
        .ok_or("missing messages")?
        .iter()
        .filter_map(|message| message["images"].as_array())
        .flatten()
        .filter_map(|image| image.as_str().map(str::to_owned))
        .collect())
}

#[test]
fn image_recovery_and_reinspection_reach_native_provider_once() -> TestResult {
    let temp = TempDir::new()?;
    let app = AppCore::initialize(temp.path().join("Zeta"))?;
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
        .write_to(&mut png, ImageFormat::Png)?;
    let first = app.import_image(png.get_ref(), NOW)?.id;
    let second = app.import_image(png.get_ref(), NOW)?.id;
    assert_ne!(first, second);
    let model = verified_model()?;
    let mut conversation = Conversation {
        id: ConversationId("conversation".to_owned()),
        project_id: ProjectId("project".to_owned()),
        title: "Transcribe".to_owned(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
        messages: vec![Message {
            id: MessageId("user".to_owned()),
            conversation_id: ConversationId("conversation".to_owned()),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                text: "Transcribe the source into output.csv".to_owned(),
            }],
            created_at: NOW.to_owned(),
        }],
    };
    append_inspection(&mut conversation, "first-inspect", &first);
    let first_images = native_images(
        &app,
        &model,
        &conversation,
        &RichInputPresentationPolicy::default().with_requested_images([first.clone()]),
        false,
    )?;
    assert_eq!(first_images.len(), 1);
    let presented = RichInputPresentationPolicy::from_verified([(
        first.clone(),
        app.rich_input_identity(&first)?.content_sha256().to_owned(),
        "b".repeat(64),
    )])
    .with_retained_presentations();
    assert_eq!(
        native_images(&app, &model, &conversation, &presented, false)?,
        first_images
    );
    // The exhausted output is not appended. Recovery retains the original input.
    assert_eq!(
        native_images(
            &app,
            &model,
            &conversation,
            &presented.clone().with_requested_images([first.clone()]),
            false
        )?,
        first_images
    );
    // Tool search expands the catalogue, then explicit inspection returns a new
    // managed ID for identical bytes. Old digest suppression must not win.
    append_inspection(&mut conversation, "second-inspect", &second);
    assert_eq!(
        native_images(
            &app,
            &model,
            &conversation,
            &presented.clone().with_requested_images([first, second]),
            true
        )?,
        first_images
    );
    assert_eq!(
        native_images(&app, &model, &conversation, &presented, true)?,
        first_images
    );
    Ok(())
}
