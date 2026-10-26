use super::*;

#[test]
fn persists_chat_turns_only_at_safe_boundaries() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, "2026-08-15T12:00:00Z")?;
    let conversation = app.create_conversation("2026-08-15T12:00:00Z")?;
    let with_user = app.append_user_message(
        &conversation.id,
        "Plan a local-first editor".to_owned(),
        Vec::new(),
        "2026-08-15T12:01:00Z",
    )?;
    assert_eq!(with_user.messages.len(), 1);
    assert_eq!(with_user.messages[0].role, MessageRole::User);

    let (prompt, assets) = app.render_chat_prompt(&with_user, "2026-08-15")?;
    assert!(prompt.contains("You are Minutus"));
    assert!(prompt.contains("sponsored by FlowZed"));
    assert!(prompt.contains("developed by Omar Peracha"));
    assert!(!prompt.contains("developed by FlowZed"));
    assert!(!prompt.contains("FlowZed knowledge foundation"));
    assert!(prompt.contains("Plan a local-first editor"));
    assert!(assets.is_empty());

    // A streamed partial is intentionally absent until a terminal success persists it.
    let partial = app
        .conversation(&conversation.id)?
        .ok_or("conversation disappeared before completion")?;
    assert_eq!(partial.messages.len(), 1);
    app.append_assistant_message(
        &conversation.id,
        Some("I should propose a concise plan.".to_owned()),
        vec![TaskActivity {
            id: "activity-1".to_owned(),
            kind: TaskActivityKind::Search,
            title: "Searched local knowledge".to_owned(),
            detail: None,
            status: TaskActivityStatus::Completed,
        }],
        "Here is a plan.".to_owned(),
        "2026-08-15T12:02:00Z",
    )?;
    let completed = app
        .conversation(&conversation.id)?
        .ok_or("conversation disappeared after completion")?;
    assert_eq!(completed.title, "Plan a local-first editor");
    assert_eq!(completed.messages.len(), 2);
    assert_eq!(completed.messages[1].role, MessageRole::Assistant);
    assert!(matches!(
        &completed.messages[1].parts[0],
        MessagePart::Reasoning { text } if text == "I should propose a concise plan."
    ));
    assert!(matches!(
        &completed.messages[1].parts[1],
        MessagePart::Activity { activity }
            if activity.kind == TaskActivityKind::Search
                && activity.status == TaskActivityStatus::Completed
    ));
    let with_follow_up = app.append_user_message(
        &conversation.id,
        "Make it actionable.".to_owned(),
        Vec::new(),
        "2026-08-15T12:03:00Z",
    )?;
    let (completed_prompt, _) = app.render_chat_prompt(&with_follow_up, "2026-08-15")?;
    assert!(!completed_prompt.contains("I should propose a concise plan."));
    Ok(())
}

#[test]
fn admits_only_bundled_flowzed_knowledge_and_exposes_safe_instruction_identity()
-> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, "2026-08-17T12:00:00Z")?;
    let conversation = app.create_conversation("2026-08-17T12:00:00Z")?;
    let conversation = app.append_user_message(
        &conversation.id,
        "What is FlowZed Atelier?".to_owned(),
        Vec::new(),
        "2026-08-17T12:01:00Z",
    )?;

    let (prompt, assets) = app.render_chat_prompt(&conversation, "2026-08-17")?;
    assert!(prompt.contains("FlowZed knowledge foundation"));
    assert!(prompt.contains("£1,550 GBP"));
    assert!(prompt.contains("snapshot facts rather than guarantees"));
    assert!(assets.is_empty());

    let identity = app.agent_system_instruction_identity(&conversation, "2026-08-17")?;
    assert_eq!(identity.policy_version(), AGENT_SYSTEM_POLICY_VERSION);
    assert_eq!(identity.knowledge_bundle_version(), Some(3));
    assert_eq!(identity.content_sha256().len(), 64);
    let serialized = serde_json::to_string(&identity)?;
    assert!(!serialized.contains("Atelier"));
    assert!(!serialized.contains("£1,550"));
    Ok(())
}

#[test]
fn renders_persisted_tool_exchange_after_tool_leaves_active_catalogue() -> Result<(), Box<dyn Error>>
{
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, "2026-08-15T12:00:00Z")?;
    let mut conversation = app.create_conversation("2026-08-15T12:00:00Z")?;
    conversation.messages.push(Message {
        id: MessageId("user-request".to_owned()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::User,
        parts: vec![MessagePart::Text {
            text: "Read src/lib.rs".to_owned(),
        }],
        created_at: "2026-08-15T12:00:30Z".to_owned(),
    });
    let call_id = ipc_types::ToolCallId("generation:tool:1:round:1".to_owned());
    conversation.messages.push(Message {
        id: MessageId("assistant-tool-call".to_owned()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::Assistant,
        parts: vec![MessagePart::ToolCall {
            request: ipc_types::ToolRequest {
                call_id: call_id.clone(),
                name: ToolName::FilesystemReadText,
                arguments: std::collections::BTreeMap::from([
                    (
                        "path".to_owned(),
                        JsonValue::String("src/lib.rs".to_owned()),
                    ),
                    (
                        "workspace".to_owned(),
                        JsonValue::String("primary".to_owned()),
                    ),
                ]),
            },
        }],
        created_at: "2026-08-15T12:01:00Z".to_owned(),
    });
    conversation.messages.push(Message {
        id: MessageId("tool-result".to_owned()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::Tool,
        parts: vec![MessagePart::ToolResult {
            result: ipc_types::ToolResult {
                call_id,
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Json {
                        value: JsonValue::Object(std::collections::BTreeMap::from([(
                            "text".to_owned(),
                            JsonValue::String("pub fn safe() {}".to_owned()),
                        )])),
                    }],
                },
            },
        }],
        created_at: "2026-08-15T12:01:01Z".to_owned(),
    });

    let (prompt, assets) = app.render_chat_prompt_with_tools(
        &conversation,
        "2026-08-15",
        vec![PromptToolDefinition {
            name: "filesystem.write_text".to_owned(),
            description: "Write a project file.".to_owned(),
            parameters: serde_json::json!({ "type": "object" }),
        }],
    )?;

    assert!(prompt.contains("filesystem.read_text"));
    assert!(!prompt.contains("{\"name\": \"filesystem.read_text\""));
    assert!(prompt.contains("{\"name\": \"filesystem.write_text\""));
    assert!(prompt.contains("src/lib.rs"));
    assert!(prompt.contains("pub fn safe() {}"));
    assert!(assets.is_empty());
    Ok(())
}

#[test]
fn automatic_titles_are_bounded_and_preserve_manual_renames() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        derive_conversation_title("  Can you tell me a bit about zero-point energy?  "),
        Some("Zero-point energy".to_owned())
    );
    let long_title = derive_conversation_title(
        "Please design a local-first collaborative editor with offline sync and a careful conflict resolution experience",
    )
    .ok_or("title was not derived")?;
    assert!(long_title.len() <= 72);
    assert!(!long_title.chars().last().is_some_and(char::is_whitespace));

    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, "2026-08-15T12:00:00Z")?;
    let conversation = app.create_conversation("2026-08-15T12:00:00Z")?;
    app.append_user_message(
        &conversation.id,
        "Draft release notes".to_owned(),
        Vec::new(),
        "2026-08-15T12:01:00Z",
    )?;
    app.rename_conversation(&conversation.id, "August release", "2026-08-15T12:01:30Z")?;
    app.append_assistant_message(
        &conversation.id,
        None,
        Vec::new(),
        "Release notes".to_owned(),
        "2026-08-15T12:02:00Z",
    )?;
    assert_eq!(
        app.conversation(&conversation.id)?
            .ok_or("conversation disappeared")?
            .title,
        "August release"
    );
    Ok(())
}

#[test]
fn automatic_titles_ignore_intermediate_tool_call_rounds() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, "2026-08-15T12:00:00Z")?;
    let conversation = app.create_conversation("2026-08-15T12:00:00Z")?;
    let mut conversation = app.append_user_message(
        &conversation.id,
        "Inspect the manual image".to_owned(),
        Vec::new(),
        "2026-08-15T12:01:00Z",
    )?;
    conversation.messages.push(Message {
        id: MessageId("assistant-tool-round".to_owned()),
        conversation_id: conversation.id.clone(),
        role: MessageRole::Assistant,
        parts: vec![
            MessagePart::Text {
                text: "I will inspect it.".to_owned(),
            },
            MessagePart::ToolCall {
                request: ipc_types::ToolRequest {
                    call_id: ipc_types::ToolCallId("inspect-image".to_owned()),
                    name: ToolName::FilesystemInspectImage,
                    arguments: std::collections::BTreeMap::new(),
                },
            },
        ],
        created_at: "2026-08-15T12:01:30Z".to_owned(),
    });
    app.save_conversation(&conversation)?;

    app.append_assistant_message(
        &conversation.id,
        None,
        Vec::new(),
        "The image shows the manual.".to_owned(),
        "2026-08-15T12:02:00Z",
    )?;

    assert_eq!(
        app.conversation(&conversation.id)?
            .ok_or("conversation disappeared")?
            .title,
        "Inspect the manual image"
    );
    Ok(())
}

#[test]
fn inference_limits_are_product_bounded() -> Result<(), Box<dyn Error>> {
    assert!(validate_inference_limits(131_072, 4_096).is_ok());
    assert!(validate_inference_limits(262_144, 8_192).is_ok());
    assert!(matches!(
        validate_inference_limits(262_145, 4_096),
        Err(AppCoreError::InvalidInferenceLimits)
    ));
    assert!(matches!(
        validate_inference_limits(4_096, 4_097),
        Err(AppCoreError::InvalidInferenceLimits)
    ));
    assert!(matches!(
        validate_inference_limits(32_768, 8_193),
        Err(AppCoreError::InvalidInferenceLimits)
    ));

    let catalogue = bundled_catalogue()?;
    let glimmer = &catalogue.models()[0];
    let qwen = &catalogue.models()[1];
    assert_eq!(product_output_tokens(glimmer), 4_096);
    assert_eq!(product_output_tokens(qwen), 8_192);
    Ok(())
}

#[test]
fn multimodal_generation_accepts_only_managed_verified_asset_ids() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    let asset = app.import_image(&source_png()?, "2026-08-15T12:00:00Z")?;
    let model = &app.model_catalogue().models()[0];
    let binding = GenerationBinding::glimmer_ollama(
        model.key().id(),
        model.key().version(),
        model.artifact().sha256(),
        model.backend_compatibility_version(),
        model.capabilities().clone(),
    );
    let prompt = "describe <|patch|>".to_owned();
    let result = app.start_generation_with_assets(
        "managed-image",
        PreparedChatPrompt::Local {
            rendered: prompt,
            tools: Vec::new(),
        },
        model.defaults().context_tokens(),
        32,
        &binding,
        std::slice::from_ref(&asset.id),
        |_| {},
    );
    assert!(
        matches!(
            result,
            Err(AppCoreError::InstalledModelNotFound
                | AppCoreError::Inference(InferenceError::ProfileBindingMismatch))
        ),
        "unexpected generation result: {result:?}"
    );
    let missing = app.start_generation_with_assets(
        "missing-image",
        PreparedChatPrompt::Local {
            rendered: "<|patch|>".to_owned(),
            tools: Vec::new(),
        },
        model.defaults().context_tokens(),
        32,
        &binding,
        &[AssetId("missing".to_owned())],
        |_| {},
    );
    assert!(matches!(missing, Err(AppCoreError::AssetNotFound)));
    Ok(())
}

#[test]
fn rich_evidence_checkpoint_reuse_is_transactional_and_presentation_is_independent()
-> Result<(), Box<dyn Error>> {
    const NOW: &str = "2026-08-23T12:00:00Z";
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    create_test_project(&app, NOW)?;
    let asset = app.import_image(&source_png()?, NOW)?;
    let conversation = app.create_conversation(NOW)?;
    let conversation = app.append_user_message(
        &conversation.id,
        "Extract the chart values".to_owned(),
        vec![asset.id.clone()],
        NOW,
    )?;
    let model = &app.model_catalogue().models()[0];
    let binding = GenerationBinding::glimmer_ollama(
        model.key().id(),
        model.key().version(),
        model.artifact().sha256(),
        model.backend_compatibility_version(),
        model.capabilities().clone(),
    );
    assert!(!app.capture_rich_evidence_checkpoint(
        &conversation,
        std::slice::from_ref(&asset.id),
        &binding.identity_sha256(),
        "   ",
        NOW,
    )?);
    assert!(app.capture_rich_evidence_checkpoint(
        &conversation,
        std::slice::from_ref(&asset.id),
        &binding.identity_sha256(),
        "The chart has columns labeled Month and Amount.",
        NOW,
    )?);

    let working = working_context::derive(&conversation)?;
    let replay = |work, retained| {
        working_context::rich_evidence::prompt_checkpoint(
            &app,
            &conversation,
            &working,
            &binding,
            work,
            retained,
        )
    };
    assert!(replay(ExecutionWorkKind::Evidence, false)?.is_none());
    assert!(
        replay(ExecutionWorkKind::Transformation, true)?.is_none(),
        "a recap cannot replace an actively retained source"
    );
    assert!(
        replay(ExecutionWorkKind::Planning, false)?.is_some(),
        "restart planning should reuse an exact persisted extraction"
    );
    let checkpoint =
        replay(ExecutionWorkKind::Transformation, false)?.ok_or("checkpoint was not reused")?;
    let mut submitted = Vec::new();
    let rendered = prompt_message(
        conversation.messages.last().ok_or("message missing")?,
        &mut submitted,
        &crate::chat_generation::ResolvedRichInputPresentationPolicy::exact_for_test([asset
            .id
            .clone()]),
        Some(&checkpoint),
    )?;
    assert!(submitted.is_empty());
    assert!(matches!(rendered, PromptMessage::User { .. }));
    assert!(checkpoint.render().contains("untrusted_extraction"));
    assert!(!checkpoint.render().contains("success\":true"));
    let oversized = "x".repeat(12 * 1024 + 1);
    for invalid in ["", oversized.as_str()] {
        assert!(!app.capture_rich_evidence_checkpoint(
            &conversation,
            std::slice::from_ref(&asset.id),
            &binding.identity_sha256(),
            invalid,
            NOW,
        )?);
    }
    assert!(
        replay(ExecutionWorkKind::Transformation, false)?.is_some(),
        "malformed renewed extraction must preserve the older valid checkpoint"
    );
    Ok(())
}
