use super::*;

#[test]
fn imports_references_and_cleans_up_managed_image_assets() -> Result<(), Box<dyn Error>> {
    const NOW: &str = "2026-08-14T12:00:00Z";
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let app = AppCore::initialize(&data_directory)?;
    let asset = app.import_image(&source_png()?, NOW)?;
    assert!(
        app.inference_asset(&asset.id)?
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    let original_path = data_directory
        .join("assets")
        .join(&asset.original.storage_key);
    let derivative_path = data_directory
        .join("assets")
        .join(&asset.derivative.storage_key);
    assert!(original_path.is_file());
    assert!(derivative_path.is_file());

    let conversation_id = ConversationId("conversation-asset".to_owned());
    create_test_project(&app, NOW)?;
    let conversation = Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Image".to_owned(),
        messages: vec![Message {
            id: MessageId("message-asset".to_owned()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            parts: vec![MessagePart::Asset {
                asset_id: asset.id.clone(),
            }],
            created_at: NOW.to_owned(),
        }],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    };
    app.save_conversation(&conversation)?;
    assert!(app.delete_conversation(&conversation_id)?);
    assert!(!original_path.exists());
    assert!(!derivative_path.exists());
    assert!(matches!(
        app.inference_asset(&asset.id),
        Err(AppCoreError::AssetNotFound)
    ));
    Ok(())
}

#[test]
fn inspects_workspace_images_as_managed_tool_assets() -> Result<(), Box<dyn Error>> {
    const NOW: &str = "2026-08-17T10:00:00Z";
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let workspace = parent.path().join("Project");
    fs::create_dir(&workspace)?;
    fs::write(workspace.join("diagram.png"), source_png()?)?;
    fs::write(workspace.join("not-an-image.txt"), "plain text")?;
    let app = AppCore::initialize(&data_directory)?;
    let grant = app.grant_workspace(&workspace, vec![1], NOW)?;

    let asset = app.filesystem_inspect_image(&grant.id, &workspace, "diagram.png", NOW)?;
    assert!(
        app.inference_asset(&asset.id)?
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert!(matches!(
        app.filesystem_inspect_image(&grant.id, &workspace, "not-an-image.txt", NOW,),
        Err(AppCoreError::AssetStore(_))
    ));

    create_test_project(&app, NOW)?;
    let conversation = workspace_image_tool_conversation(asset.id.clone(), NOW);
    let conversation_id = conversation.id.clone();
    app.save_conversation(&conversation)?;
    let (prompt, asset_ids) = app.render_chat_prompt_with_tools(
        &conversation,
        "2026-08-17",
        vec![PromptToolDefinition {
            name: "filesystem.inspect_image".to_owned(),
            description: "Inspect a workspace image.".to_owned(),
            parameters: serde_json::json!({ "type": "object" }),
        }],
    )?;
    assert_eq!(asset_ids, vec![asset.id.clone()]);
    assert!(prompt.contains("<|patch|>"));
    assert!(!app.discard_unreferenced_image(&asset.id)?);

    assert!(app.revoke_workspace(&grant.id, "2026-08-17T11:00:00Z")?);
    assert!(matches!(
        app.filesystem_inspect_image(&grant.id, &workspace, "diagram.png", NOW),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    assert_eq!(app.with_storage(|storage| storage.audit_event_count())?, 5);
    assert!(
        app.inference_asset(&asset.id)?
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert!(app.delete_conversation(&conversation_id)?);
    assert!(matches!(
        app.inference_asset(&asset.id),
        Err(AppCoreError::AssetNotFound)
    ));
    Ok(())
}

#[test]
fn discards_only_unreferenced_managed_images() -> Result<(), Box<dyn Error>> {
    const NOW: &str = "2026-08-14T12:00:00Z";
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    let pending = app.import_image(&source_png()?, NOW)?;
    assert!(app.discard_unreferenced_image(&pending.id)?);
    assert!(!app.discard_unreferenced_image(&pending.id)?);
    assert!(matches!(
        app.inference_asset(&pending.id),
        Err(AppCoreError::AssetNotFound)
    ));

    let referenced = app.import_image(&source_png()?, NOW)?;
    create_test_project(&app, NOW)?;
    let conversation_id = ConversationId("conversation-retained-image".to_owned());
    app.save_conversation(&Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Retained image".to_owned(),
        messages: vec![Message {
            id: MessageId("message-retained-image".to_owned()),
            conversation_id,
            role: MessageRole::User,
            parts: vec![MessagePart::Asset {
                asset_id: referenced.id.clone(),
            }],
            created_at: NOW.to_owned(),
        }],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    assert!(!app.discard_unreferenced_image(&referenced.id)?);
    assert!(app.inference_asset(&referenced.id)?.starts_with(b"\x89PNG"));
    Ok(())
}

#[test]
fn concurrent_inference_read_and_deletion_are_serialized() -> Result<(), Box<dyn Error>> {
    const NOW: &str = "2026-08-14T12:00:00Z";
    let parent = TempDir::new()?;
    let app = Arc::new(AppCore::initialize(parent.path().join("Zeta"))?);
    create_test_project(&app, NOW)?;
    let asset = app.import_image(&source_png()?, NOW)?;
    let conversation_id = ConversationId("conversation-race".to_owned());
    app.save_conversation(&Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Race".to_owned(),
        messages: vec![Message {
            id: MessageId("message-race".to_owned()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            parts: vec![MessagePart::Asset {
                asset_id: asset.id.clone(),
            }],
            created_at: NOW.to_owned(),
        }],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    let barrier = Arc::new(Barrier::new(3));
    let reader_app = Arc::clone(&app);
    let reader_barrier = Arc::clone(&barrier);
    let asset_id = asset.id.clone();
    let reader = thread::spawn(move || {
        reader_barrier.wait();
        reader_app.inference_asset(&asset_id)
    });
    let deleter_app = Arc::clone(&app);
    let deleter_barrier = Arc::clone(&barrier);
    let deleter = thread::spawn(move || {
        deleter_barrier.wait();
        deleter_app.delete_conversation(&conversation_id)
    });
    barrier.wait();
    let read_result = reader
        .join()
        .map_err(|_| std::io::Error::other("reader thread panicked"))?;
    let delete_result = deleter
        .join()
        .map_err(|_| std::io::Error::other("deleter thread panicked"))?;
    assert!(delete_result?);
    assert!(matches!(
        read_result,
        Ok(_) | Err(AppCoreError::AssetNotFound)
    ));
    assert!(matches!(
        app.inference_asset(&asset.id),
        Err(AppCoreError::AssetNotFound)
    ));
    Ok(())
}

#[test]
fn diagnostic_preference_persists_and_export_is_previewed() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let app = AppCore::initialize(&data_directory)?;
    let plan = app.plan_diagnostic_export()?;
    assert!(plan.preview().record_count() >= 1);
    assert!(!plan.preview().privacy_description().is_empty());
    let destination = parent.path().join("diagnostics.jsonl");
    let receipt = app.write_diagnostic_export(plan, &destination)?;
    assert!(receipt.bytes_written() > 0);
    let _ = app.set_diagnostic_logging(false, "2026-08-14T12:00:00Z")?;
    assert!(!app.diagnostic_logging_enabled());
    drop(app);

    let reopened = AppCore::initialize(&data_directory)?;
    assert!(!reopened.diagnostic_logging_enabled());
    Ok(())
}
