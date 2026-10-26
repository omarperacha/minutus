use super::*;

#[test]
fn initializes_private_rust_owned_storage() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let app = AppCore::initialize(&data_directory)?;
    assert_eq!(
        app.with_storage(|storage| storage.schema_version())?,
        SCHEMA_VERSION
    );
    assert!(app.diagnostic_logging_enabled());
    assert_eq!(app.model_catalogue().models().len(), 3);
    assert_eq!(
        app.model_catalogue()
            .models()
            .iter()
            .filter(|model| model.is_integrated())
            .count(),
        if app
            .model_catalogue()
            .models()
            .iter()
            .any(|model| model.is_mlx() && model.supports_current_runtime())
        {
            3
        } else {
            2
        }
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&data_directory)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(data_directory.join("zeta.sqlite3"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Ok(())
}

#[test]
fn application_core_can_be_created_and_released_inside_tokio() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    runtime.block_on(async {
        let app = AppCore::initialize(parent.path().join("Zeta"))?;
        drop(app);
        Ok::<(), AppCoreError>(())
    })?;
    Ok(())
}

#[test]
fn install_policy_resolves_every_declared_companion() -> Result<(), Box<dyn Error>> {
    let catalogue = bundled_catalogue()?;
    let glimmer = &catalogue.models()[0];

    let complete = complete_bundle_companions(glimmer, &[])?;
    assert_eq!(complete.len(), glimmer.companions().len());
    assert_eq!(complete[0].kind(), CompanionKind::VisionProjector);
    assert_eq!(complete[1].kind(), CompanionKind::DraftModel);

    let legacy_single = vec![complete[0].artifact().key().id().to_owned()];
    assert_eq!(
        complete_bundle_companions(glimmer, &legacy_single)?.len(),
        glimmer.companions().len()
    );
    assert!(matches!(
        complete_bundle_companions(glimmer, &["unknown-module".to_owned()]),
        Err(AppCoreError::InvalidCompanionSelection)
    ));
    Ok(())
}

#[test]
#[ignore = "requires a live local Ollama server and an explicit 18 GB Qwen installation"]
fn live_qwen_install_uses_the_zeta_complete_bundle_path() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    let qwen = app
        .model_catalogue()
        .models()
        .iter()
        .find(|model| model.backend() == InferenceBackend::Ollama)
        .ok_or("Qwen catalogue profile is missing")?;
    let expected_total = std::iter::once(qwen.artifact())
        .chain(qwen.companions().iter().map(CompanionArtifact::artifact))
        .try_fold(0_u64, |total, artifact| {
            total.checked_add(artifact.installed_bytes())
        })
        .ok_or("Qwen bundle size overflowed")?;
    let mut snapshots = Vec::new();
    let mut last_reported_bytes = 0_u64;
    let mut last_reported_phase = None;

    let receipt = app.install_catalogue_model(
        qwen.key(),
        &[],
        &DownloadCancellation::new(),
        |snapshot| {
            let phase = snapshot.artifact().phase();
            let downloaded = snapshot.aggregate_downloaded_bytes();
            if downloaded.saturating_sub(last_reported_bytes) >= 256 * 1024 * 1024
                || last_reported_phase != Some(phase)
            {
                eprintln!(
                    "Qwen bundle {:?}: {} / {} bytes",
                    phase,
                    downloaded,
                    snapshot.aggregate_total_bytes()
                );
                last_reported_bytes = downloaded;
                last_reported_phase = Some(phase);
            }
            snapshots.push(snapshot);
        },
        "2026-08-20T12:00:00Z",
    )?;

    assert_eq!(receipt.installed_bytes(), expected_total);
    assert_eq!(receipt.artifact_ids().len(), qwen.companions().len() + 1);
    assert_eq!(
        snapshots
            .last()
            .map(CatalogueInstallProgress::aggregate_total_bytes),
        Some(expected_total)
    );
    assert!(app.catalogue_model_installed(qwen.key())?);
    Ok(())
}

#[test]
#[ignore = "requires the locally installed pinned Ollama application"]
#[cfg(target_os = "macos")]
fn bundled_ollama_reconnects_provider_after_supervised_stop() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let app = AppCore::initialize_with_bundled_ollama(
        parent.path().join("Zeta"),
        "/Applications/Ollama.app/Contents/Resources/ollama",
        "/usr/bin/false",
    )?;
    let qwen = app
        .model_catalogue()
        .models()
        .iter()
        .find(|model| model.backend() == InferenceBackend::Ollama)
        .ok_or("Qwen catalogue profile is missing")?;

    assert!(app.stop_managed_ollama_for_test()?);
    assert!(!app.catalogue_model_installed(qwen.key())?);
    assert_eq!(
        app.managed_ollama_status_for_test()?,
        Some(model_runtime::OllamaSidecarStatus::Running)
    );
    Ok(())
}

#[test]
fn projects_own_chats_and_folder_capabilities() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let first_root = parent.path().join("Alpha");
    let second_root = parent.path().join("Beta");
    fs::create_dir(&first_root)?;
    fs::create_dir(&second_root)?;
    let app = AppCore::initialize(data_directory)?;
    assert!(app.projects()?.is_empty());

    let first = app.create_project("Research", "2026-08-16T10:00:00Z")?;
    let second = app.create_project("Writing", "2026-08-16T11:00:00Z")?;
    assert!(app.project_workspaces(&first.id)?.is_empty());
    assert!(matches!(
        app.create_project("  ", "2026-08-16T11:30:00Z"),
        Err(AppCoreError::InvalidProjectName)
    ));

    app.add_project_workspace(&first.id, &first_root, vec![1], "2026-08-16T11:40:00Z")?;
    app.add_project_workspace(&first.id, &second_root, vec![2], "2026-08-16T11:41:00Z")?;
    app.add_project_workspace(&second.id, &first_root, vec![3], "2026-08-16T11:42:00Z")?;
    let chat = app.create_project_conversation(&first.id, "2026-08-16T12:00:00Z")?;
    assert_eq!(chat.project_id, first.id);
    assert_eq!(app.project_conversations(&first.id)?.len(), 1);
    assert!(app.project_conversations(&second.id)?.is_empty());

    let first_workspaces = app.project_workspaces(&first.id)?;
    assert_eq!(first_workspaces.len(), 2);
    let first_folder = first_workspaces[0].clone();
    assert_eq!(first_workspaces[1].role, ProjectWorkspaceRole::Additional);
    let second_folder = app.project_workspaces(&second.id)?.remove(0);
    assert_eq!(first_folder.role, ProjectWorkspaceRole::Primary);
    assert_ne!(first_folder.grant_id, second_folder.grant_id);
    app.update_project_workspace_permission(
        &first.id,
        &first_folder.grant_id,
        WorkspacePermissionLevel::ReadAndWrite,
    )?;
    assert_eq!(
        app.project_workspaces(&second.id)?[0].permission_level,
        WorkspacePermissionLevel::ReadOnly
    );
    assert_eq!(
        app.project_workspace_bookmark_data(&first.id, &first_folder.grant_id)?,
        vec![1]
    );
    assert!(matches!(
        app.project_workspace_bookmark_data(&second.id, &first_folder.grant_id),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    Ok(())
}

#[test]
fn manages_persistent_conversation_history() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let app = AppCore::initialize(&data_directory)?;
    create_test_project(&app, "2026-08-15T12:00:00Z")?;
    let conversation = app.create_conversation("2026-08-15T12:00:00Z")?;
    assert_eq!(conversation.title, DEFAULT_CONVERSATION_TITLE);
    assert_eq!(
        app.conversation(&conversation.id)?,
        Some(conversation.clone())
    );

    let renamed = app
        .rename_conversation(&conversation.id, "Launch notes", "2026-08-15T13:00:00Z")?
        .ok_or("created conversation was not found")?;
    assert_eq!(renamed.title, "Launch notes");
    assert_eq!(app.conversations()?, vec![renamed.clone()]);

    drop(app);
    let reopened = AppCore::initialize(&data_directory)?;
    assert_eq!(reopened.conversations()?, vec![renamed]);
    assert!(reopened.delete_conversation(&conversation.id)?);
    assert!(reopened.conversations()?.is_empty());
    Ok(())
}

#[test]
fn grants_lists_refreshes_and_revokes_workspaces() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let workspace = parent.path().join("Project");
    fs::create_dir(&workspace)?;
    let app = AppCore::initialize(&data_directory)?;

    let granted = app.grant_workspace(&workspace, vec![1, 2, 3], "2026-08-15T12:00:00Z")?;
    assert_eq!(granted.display_name, "Project");
    assert_eq!(granted.permission_level, WorkspacePermissionLevel::ReadOnly);
    assert_eq!(app.workspace_grants()?, vec![granted.clone()]);
    assert!(
        app.authorize_workspace_operation(&granted.id, WorkspaceOperation::Read)
            .is_ok()
    );
    assert!(
        app.authorize_workspace_operation(&granted.id, WorkspaceOperation::Write)
            .is_err()
    );

    let refreshed = app.grant_workspace(&workspace, vec![4, 5, 6], "2026-08-15T13:00:00Z")?;
    assert_eq!(refreshed.id, granted.id);
    assert_eq!(refreshed.created_at, granted.created_at);
    assert_eq!(app.workspace_grants()?, vec![refreshed.clone()]);

    let updated =
        app.update_workspace_permission(&refreshed.id, WorkspacePermissionLevel::ReadAndWrite)?;
    assert_eq!(
        updated.permission_level,
        WorkspacePermissionLevel::ReadAndWrite
    );
    assert!(
        app.authorize_workspace_operation(&updated.id, WorkspaceOperation::Write)
            .is_ok()
    );

    assert!(app.revoke_workspace(&updated.id, "2026-08-15T14:00:00Z")?);
    assert!(matches!(
        app.authorize_workspace_operation(&updated.id, WorkspaceOperation::Read),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    assert!(app.workspace_grants()?.is_empty());
    drop(app);
    assert!(
        AppCore::initialize(&data_directory)?
            .workspace_grants()?
            .is_empty()
    );
    Ok(())
}

#[test]
fn filesystem_reads_require_an_active_grant_and_stop_after_revocation() -> Result<(), Box<dyn Error>>
{
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let workspace = parent.path().join("Project");
    fs::create_dir(&workspace)?;
    fs::create_dir(workspace.join("src"))?;
    fs::write(workspace.join("src/lib.rs"), "pub fn safe() {}\n")?;
    let app = AppCore::initialize(&data_directory)?;
    let grant = app.grant_workspace(&workspace, vec![1, 2, 3], "2026-08-15T12:00:00Z")?;

    assert_eq!(app.workspace_bookmark_data(&grant.id)?, vec![1, 2, 3]);
    let listing = app.filesystem_list(&grant.id, &workspace, ".")?;
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].kind, FilesystemEntryKind::Directory);
    let read = app.filesystem_read_text(&grant.id, &workspace, "src/lib.rs")?;
    assert_eq!(read.text, "pub fn safe() {}\n");
    let search = app.filesystem_search(&grant.id, &workspace, ".", "safe")?;
    assert_eq!(search.matches.len(), 1);
    let scan = app.filesystem_scan(&grant.id, &workspace, ".", true, 1, &["md".to_owned()])?;
    assert_eq!(scan.matches.len(), 1);
    assert_eq!(scan.matches[0].relative_path, "src/lib.rs");
    assert_eq!(scan.matches[0].line_count, 1);

    assert!(app.revoke_workspace(&grant.id, "2026-08-15T13:00:00Z")?);
    assert!(matches!(
        app.filesystem_read_text(&grant.id, &workspace, "src/lib.rs"),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    assert!(matches!(
        app.workspace_bookmark_data(&grant.id),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    assert_eq!(app.with_storage(|storage| storage.audit_event_count())?, 7);
    Ok(())
}

#[test]
fn filesystem_writes_require_write_permission_and_exact_original_digest()
-> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let data_directory = parent.path().join("Zeta");
    let workspace = parent.path().join("Project");
    fs::create_dir(&workspace)?;
    fs::write(workspace.join("notes.txt"), "before\n")?;
    let app = AppCore::initialize(&data_directory)?;
    let grant = app.grant_workspace(&workspace, vec![1, 2, 3], "2026-08-16T12:00:00Z")?;
    let original = app.filesystem_read_text(&grant.id, &workspace, "notes.txt")?;
    let change = FilesystemWriteTextChange {
        relative_path: "notes.txt".to_owned(),
        expected_sha256: Some(original.content_sha256),
        content: "after\n".to_owned(),
        create_missing_parents: false,
    };

    assert!(matches!(
        app.filesystem_write_text(&grant.id, &workspace, std::slice::from_ref(&change)),
        Err(AppCoreError::WorkspaceAuthorization(_))
    ));
    app.update_workspace_permission(&grant.id, WorkspacePermissionLevel::ReadAndWrite)?;
    let result = app.filesystem_write_text(&grant.id, &workspace, std::slice::from_ref(&change))?;
    assert_eq!(result.changes.len(), 1);
    assert_eq!(fs::read_to_string(workspace.join("notes.txt"))?, "after\n");
    assert!(matches!(
        app.filesystem_write_text(&grant.id, &workspace, std::slice::from_ref(&change)),
        Err(AppCoreError::FilesystemWrite(
            FilesystemWriteError::Conflict
        ))
    ));
    app.update_workspace_permission(&grant.id, WorkspacePermissionLevel::ReadOnly)?;
    let undo = FilesystemUndoChange {
        relative_path: "notes.txt".to_owned(),
        applied_sha256: result.changes[0].new_sha256.clone(),
        previous_content: Some("before\n".to_owned()),
        created_parent_directories: Vec::new(),
    };
    assert!(matches!(
        app.filesystem_undo_text(&grant.id, &workspace, std::slice::from_ref(&undo)),
        Err(AppCoreError::WorkspaceAuthorization(_))
    ));
    app.update_workspace_permission(&grant.id, WorkspacePermissionLevel::ReadAndWrite)?;
    let undo_result =
        app.filesystem_undo_text(&grant.id, &workspace, std::slice::from_ref(&undo))?;
    assert_eq!(undo_result.files[0].status, FilesystemUndoStatus::Undone);
    assert_eq!(fs::read_to_string(workspace.join("notes.txt"))?, "before\n");
    let restored = app.filesystem_read_text(&grant.id, &workspace, "notes.txt")?;
    let targeted = app.filesystem_replace_text(
        &grant.id,
        &workspace,
        "notes.txt",
        &restored.content_sha256,
        &[FilesystemTextReplacement {
            old_text: "before".to_owned(),
            new_text: "targeted".to_owned(),
            expected_occurrences: 1,
        }],
    )?;
    assert_eq!(targeted.changes.len(), 1);
    assert_eq!(
        fs::read_to_string(workspace.join("notes.txt"))?,
        "targeted\n"
    );
    assert!(app.revoke_workspace(&grant.id, "2026-08-16T13:00:00Z")?);
    assert!(matches!(
        app.filesystem_write_text(
            &grant.id,
            &workspace,
            &[FilesystemWriteTextChange {
                relative_path: "new.txt".to_owned(),
                expected_sha256: None,
                content: "new\n".to_owned(),
                create_missing_parents: false,
            }]
        ),
        Err(AppCoreError::WorkspaceGrantNotFound)
    ));
    assert_eq!(app.with_storage(|storage| storage.audit_event_count())?, 14);
    Ok(())
}

#[test]
fn audit_failures_do_not_weaken_filesystem_validation() -> Result<(), Box<dyn Error>> {
    let parent = TempDir::new()?;
    let workspace = parent.path().join("Project");
    fs::create_dir(&workspace)?;
    let app = AppCore::initialize(parent.path().join("Zeta"))?;
    let grant = app.grant_workspace(&workspace, vec![1], "2026-08-16T12:00:00Z")?;
    let baseline = app.with_storage(|storage| storage.audit_event_count())?;

    app.record_audit_event(&AuditEvent {
        id: uuid::Uuid::now_v7().to_string(),
        kind: AuditEventKind::FilesystemTool,
        outcome: AuditOutcome::Failed,
        subject_id: None,
        operation_id: Some("invalid-audit".to_owned()),
        tool: Some(AuditFilesystemTool::ReadText),
        workspace_grant_id: Some(grant.id.clone()),
        relative_path: Some("/absolute/path-must-not-be-recorded".to_owned()),
        permission_decision: AuditPermissionDecision::Allowed,
        result_count: None,
        truncated: None,
        occurred_at: "2026-08-16T12:00:00Z".to_owned(),
    });
    assert_eq!(
        app.with_storage(|storage| storage.audit_event_count())?,
        baseline
    );

    assert!(matches!(
        app.filesystem_read_text(&grant.id, &workspace, "../outside.txt"),
        Err(AppCoreError::Filesystem(_))
    ));
    assert_eq!(
        app.with_storage(|storage| storage.audit_event_count())?,
        baseline + 1
    );
    Ok(())
}
