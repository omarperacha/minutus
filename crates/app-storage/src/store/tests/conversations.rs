use super::*;

#[test]
fn conversation_uses_stable_asset_references() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_project(&store)?;
    let asset_id = AssetId("asset-1".to_owned());
    store.save_asset(&AssetReference {
        id: asset_id.clone(),
        original: AssetFileReference {
            storage_key: "originals/asset-1.png".to_owned(),
            media_type: "image/png".to_owned(),
            content_digest: "a".repeat(64),
            byte_size: 42,
            width: 2,
            height: 3,
        },
        derivative: AssetFileReference {
            storage_key: "inference/asset-1.png".to_owned(),
            media_type: "image/png".to_owned(),
            content_digest: "b".repeat(64),
            byte_size: 24,
            width: 2,
            height: 3,
        },
        imported_at: NOW.to_owned(),
    })?;
    let stored_asset = store
        .asset(&asset_id)?
        .ok_or(StorageError::CorruptDatabase)?;
    assert!(store.save_asset(&stored_asset).is_err());
    let conversation_id = ConversationId("conversation-1".to_owned());
    let conversation = Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Local vision".to_owned(),
        messages: vec![Message {
            id: MessageId("message-1".to_owned()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            parts: vec![
                MessagePart::Text {
                    text: "Describe this".to_owned(),
                },
                MessagePart::Asset {
                    asset_id: asset_id.clone(),
                },
            ],
            created_at: NOW.to_owned(),
        }],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    };
    store.save_conversation(&conversation)?;
    let second_id = ConversationId("conversation-2".to_owned());
    let mut second = conversation.clone();
    second.id = second_id.clone();
    second.messages[0].id = MessageId("message-2".to_owned());
    second.messages[0].conversation_id = second_id.clone();
    store.save_conversation(&second)?;
    assert_eq!(store.conversation(&conversation_id)?, Some(conversation));
    assert!(store.unreferenced_asset_ids()?.is_empty());
    let (deleted, reaped) = store.delete_conversation_and_reap_assets(&conversation_id)?;
    assert!(deleted);
    assert!(reaped.is_empty());
    assert!(store.asset(&asset_id)?.is_some());
    let (deleted, reaped) = store.delete_conversation_and_reap_assets(&second_id)?;
    assert!(deleted);
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].id, asset_id);
    assert!(store.asset(&asset_id)?.is_none());
    assert!(store.unreferenced_asset_ids()?.is_empty());
    Ok(())
}

#[test]
fn conversations_can_be_renamed_and_listed_by_recency() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_project(&store)?;
    let first_id = ConversationId("conversation-1".to_owned());
    let second_id = ConversationId("conversation-2".to_owned());
    for (id, title, updated_at) in [
        (&first_id, "First", "2026-08-14T12:00:00Z"),
        (&second_id, "Second", "2026-08-14T13:00:00Z"),
    ] {
        store.save_conversation(&Conversation {
            id: id.clone(),
            project_id: ProjectId("project-default".to_owned()),
            title: title.to_owned(),
            messages: Vec::new(),
            created_at: NOW.to_owned(),
            updated_at: updated_at.to_owned(),
        })?;
    }

    let renamed = store.rename_conversation(&first_id, "Renamed first", "2026-08-14T14:00:00Z")?;
    assert_eq!(
        renamed,
        Some(ConversationSummary {
            id: first_id.clone(),
            project_id: ProjectId("project-default".to_owned()),
            title: "Renamed first".to_owned(),
            updated_at: "2026-08-14T14:00:00Z".to_owned(),
        })
    );
    assert_eq!(
        store.conversations(&ProjectId("project-default".to_owned()))?[0].id,
        first_id
    );
    assert!(
        store
            .rename_conversation(
                &ConversationId("missing".to_owned()),
                "Missing",
                "2026-08-14T15:00:00Z",
            )?
            .is_none()
    );
    Ok(())
}

#[test]
fn malformed_message_parts_fail_closed() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_project(&store)?;
    let conversation_id = ConversationId("conversation-1".to_owned());
    store.save_conversation(&Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Test".to_owned(),
        messages: vec![Message {
            id: MessageId("message-1".to_owned()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Text {
                text: "Hello".to_owned(),
            }],
            created_at: NOW.to_owned(),
        }],
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    store.connection.execute(
        "UPDATE messages SET parts_json = '[{\"type\":\"futureVariant\"}]'",
        [],
    )?;
    assert!(matches!(
        store.conversation(&conversation_id),
        Err(StorageError::Serialization(_))
    ));
    Ok(())
}

#[test]
fn revocation_erases_and_disables_workspace_bookmark() -> Result<(), Box<dyn Error>> {
    let store = ApplicationStore::open_in_memory()?;
    let grant_id = WorkspaceGrantId("grant-1".to_owned());
    store.save_workspace_grant(&WorkspaceGrant {
        id: grant_id.clone(),
        display_name: "Project".to_owned(),
        canonical_root: "/Users/example/Project".to_owned(),
        bookmark_data: vec![1, 2, 3],
        permission_level: WorkspacePermissionLevel::ReadOnly,
        created_at: NOW.to_owned(),
        revoked_at: None,
    })?;
    assert!(store.workspace_grant(&grant_id)?.is_some());
    assert!(store.revoke_workspace_grant(&grant_id, NOW)?);
    assert!(store.workspace_grant(&grant_id)?.is_none());
    let bookmark_length: i64 = store.connection.query_row(
        "SELECT length(bookmark_data) FROM workspace_grants WHERE id = ?1",
        [&grant_id.0],
        |row| row.get(0),
    )?;
    assert_eq!(bookmark_length, 0);
    assert!(!store.revoke_workspace_grant(&grant_id, NOW)?);
    assert!(store.workspace_grants()?.is_empty());
    Ok(())
}
