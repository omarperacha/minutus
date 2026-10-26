use super::*;

#[test]
fn active_workspace_grants_are_listed_and_found_by_root() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let grant = WorkspaceGrant {
        id: WorkspaceGrantId("grant-1".to_owned()),
        display_name: "Project".to_owned(),
        canonical_root: "/Users/example/Project".to_owned(),
        bookmark_data: vec![1, 2, 3],
        permission_level: WorkspacePermissionLevel::ReadOnly,
        created_at: NOW.to_owned(),
        revoked_at: None,
    };
    store.save_workspace_grant(&grant)?;

    assert_eq!(store.workspace_grants()?, vec![grant.clone()]);
    assert_eq!(
        store.workspace_grant_for_root(&grant.canonical_root)?,
        Some(grant)
    );
    assert!(store.workspace_grant_for_root("/missing")?.is_none());
    Ok(())
}

#[test]
fn workspace_permission_updates_are_immediate_and_grant_scoped() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    for (id, root) in [("grant-1", "/Project"), ("grant-2", "/Other")] {
        store.save_workspace_grant(&WorkspaceGrant {
            id: WorkspaceGrantId(id.to_owned()),
            display_name: id.to_owned(),
            canonical_root: root.to_owned(),
            bookmark_data: vec![1],
            permission_level: WorkspacePermissionLevel::ReadOnly,
            created_at: NOW.to_owned(),
            revoked_at: None,
        })?;
    }

    let updated = store
        .update_workspace_permission(
            &WorkspaceGrantId("grant-1".to_owned()),
            WorkspacePermissionLevel::ReadAndWrite,
        )?
        .ok_or("active grant was not updated")?;
    assert_eq!(
        updated.permission_level,
        WorkspacePermissionLevel::ReadAndWrite
    );
    assert_eq!(
        store
            .workspace_grant(&WorkspaceGrantId("grant-2".to_owned()))?
            .ok_or("unrelated grant disappeared")?
            .permission_level,
        WorkspacePermissionLevel::ReadOnly
    );
    assert!(
        store
            .update_workspace_permission(
                &WorkspaceGrantId("missing".to_owned()),
                WorkspacePermissionLevel::ReadAndWrite,
            )?
            .is_none()
    );
    Ok(())
}

#[test]
fn public_metadata_and_allow_listed_audit_events_are_stored() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let model = ModelMetadata {
        id: "glimmer".to_owned(),
        display_name: "Muse Glimmer".to_owned(),
        format: "gguf".to_owned(),
        local_path: "/models/glimmer.gguf".to_owned(),
        content_digest: "digest".to_owned(),
        byte_size: 24,
        imported_at: NOW.to_owned(),
    };
    store.save_model(&model)?;
    assert_eq!(store.model("glimmer")?, Some(model));
    let companion = ModelCompanionMetadata {
        model_id: "glimmer".to_owned(),
        artifact_id: "vision".to_owned(),
        kind: "visionProjector".to_owned(),
        required: false,
        local_path: "/models/vision.gguf".to_owned(),
        content_digest: "companion-digest".to_owned(),
        byte_size: 12,
        compatibility_version: "profile-v1".to_owned(),
        memory_requirement_bytes: 16,
        installed_at: NOW.to_owned(),
    };
    store.save_model_companion(&companion)?;
    assert_eq!(
        store.model_companion("glimmer", "vision")?,
        Some(companion.clone())
    );
    assert_eq!(store.model_companions("glimmer")?, vec![companion]);
    let wallet_id = WalletId("wallet-1".to_owned());
    let wallet = PublicWalletMetadata {
        id: wallet_id.clone(),
        address: "0x1234".to_owned(),
        network: "eip155:8453".to_owned(),
        locked: true,
        updated_at: NOW.to_owned(),
    };
    store.save_public_wallet(&wallet)?;
    assert_eq!(store.public_wallet(&wallet_id)?, Some(wallet.clone()));
    assert_eq!(store.latest_public_wallet()?, Some(wallet));
    store.append_audit_event(&AuditEvent {
        id: "event-1".to_owned(),
        kind: AuditEventKind::ModelRegistered,
        outcome: AuditOutcome::Succeeded,
        subject_id: Some("glimmer".to_owned()),
        operation_id: None,
        tool: None,
        workspace_grant_id: None,
        relative_path: None,
        permission_decision: crate::AuditPermissionDecision::NotApplicable,
        result_count: None,
        truncated: None,
        occurred_at: NOW.to_owned(),
    })?;
    let audit_count: i64 =
        store
            .connection
            .query_row("SELECT count(*) FROM audit_events", [], |row| row.get(0))?;
    assert_eq!(audit_count, 1);
    Ok(())
}

#[test]
fn complete_model_bundle_metadata_is_replaced_atomically() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    let model = ModelMetadata {
        id: "glimmer".to_owned(),
        display_name: "Muse Glimmer".to_owned(),
        format: "gguf".to_owned(),
        local_path: "/models/glimmer.gguf".to_owned(),
        content_digest: "primary-digest".to_owned(),
        byte_size: 24,
        imported_at: NOW.to_owned(),
    };
    let companion = |artifact_id: &str| ModelCompanionMetadata {
        model_id: model.id.clone(),
        artifact_id: artifact_id.to_owned(),
        kind: if artifact_id == "vision" {
            "visionProjector".to_owned()
        } else {
            "draftModel".to_owned()
        },
        required: false,
        local_path: format!("/models/{artifact_id}.gguf"),
        content_digest: format!("{artifact_id}-digest"),
        byte_size: 12,
        compatibility_version: "profile-v1".to_owned(),
        memory_requirement_bytes: 16,
        installed_at: NOW.to_owned(),
    };
    let complete = vec![companion("vision"), companion("draft")];

    store.save_model_bundle(&model, &complete)?;
    assert_eq!(store.model("glimmer")?, Some(model.clone()));
    assert_eq!(store.model_companions("glimmer")?.len(), 2);

    let invalid = ModelCompanionMetadata {
        model_id: "another-model".to_owned(),
        ..companion("replacement")
    };
    assert!(store.save_model_bundle(&model, &[invalid]).is_err());
    assert_eq!(store.model_companions("glimmer")?.len(), 2);

    store.save_model_bundle(&model, &[companion("vision")])?;
    assert_eq!(
        store.model_companions("glimmer")?,
        vec![companion("vision")]
    );
    Ok(())
}
