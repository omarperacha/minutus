use super::*;
use crate::Project;
use ipc_types::{Conversation, ConversationId, ProjectId};

const NOW: &str = "2026-09-10T12:00:00Z";
fn save_scope(store: &mut ApplicationStore) -> Result<(), StorageError> {
    store.save_project(&Project {
        id: ProjectId("project".into()),
        name: "Project".into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
    })?;
    store.save_conversation(&Conversation {
        id: ConversationId("chat".into()),
        project_id: ProjectId("project".into()),
        title: "Chat".into(),
        messages: Vec::new(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
    })
}
fn checkpoint(start: u32) -> Result<DialogueContinuityCheckpoint, agent_core::WorkingContextError> {
    DialogueContinuityCheckpoint::new(
        "chat".into(),
        "project".into(),
        start,
        "user".into(),
        "a".repeat(64),
    )
}
#[test]
fn boundary_survives_reopen_and_cannot_be_implicitly_advanced()
-> Result<(), Box<dyn std::error::Error>> {
    let file = tempfile::NamedTempFile::new()?;
    {
        let mut store = ApplicationStore::open(file.path())?;
        save_scope(&mut store)?;
        store.initialise_dialogue_continuity(&checkpoint(0)?)?;
        store.initialise_dialogue_continuity(&checkpoint(10)?)?;
    }
    let mut store = ApplicationStore::open(file.path())?;
    assert_eq!(
        store.dialogue_continuity_checkpoint("chat", "project")?,
        Some(checkpoint(0)?)
    );
    assert!(
        store
            .dialogue_continuity_checkpoint("chat", "other")?
            .is_none()
    );
    store.delete_conversation_and_reap_assets(&ConversationId("chat".into()))?;
    assert!(
        store
            .dialogue_continuity_checkpoint("chat", "project")?
            .is_none()
    );
    Ok(())
}
#[test]
fn invalid_scope_version_and_corrupt_storage_fail_closed() -> Result<(), Box<dyn std::error::Error>>
{
    let mut store = ApplicationStore::open_in_memory()?;
    assert!(
        store
            .initialise_dialogue_continuity(&checkpoint(0)?)
            .is_err()
    );
    save_scope(&mut store)?;
    let wrong = DialogueContinuityCheckpoint::new(
        "chat".into(),
        "other".into(),
        0,
        "user".into(),
        "a".repeat(64),
    )?;
    assert!(store.initialise_dialogue_continuity(&wrong).is_err());
    store.initialise_dialogue_continuity(&checkpoint(0)?)?;
    let mut encoded = serde_json::to_value(checkpoint(0)?)?;
    encoded["version"] = serde_json::json!(2);
    store.connection.execute(
        "UPDATE dialogue_continuity_checkpoints SET checkpoint_json = ?1",
        [encoded.to_string()],
    )?;
    assert!(
        store
            .dialogue_continuity_checkpoint("chat", "project")
            .is_err()
    );
    encoded["version"] = serde_json::json!(1);
    encoded["conversationId"] = serde_json::json!("wrong");
    store.connection.execute(
        "UPDATE dialogue_continuity_checkpoints SET checkpoint_json = ?1",
        [encoded.to_string()],
    )?;
    assert!(
        store
            .dialogue_continuity_checkpoint("chat", "project")
            .is_err()
    );
    assert!(
        DialogueContinuityCheckpoint::new(
            "chat".into(),
            "project".into(),
            0,
            "user".into(),
            "not-a-digest".into()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn explicit_advancement_compares_the_previous_record_and_rejects_scope_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_scope(&mut store)?;
    let first = checkpoint(0)?;
    let next = checkpoint(10)?;
    store.initialise_dialogue_continuity(&first)?;
    assert!(store.advance_dialogue_continuity(&first, &next)?);
    assert!(!store.advance_dialogue_continuity(&first, &next)?);
    assert!(store.advance_dialogue_continuity(&next, &first).is_err());
    let other = DialogueContinuityCheckpoint::new(
        "chat".into(),
        "other".into(),
        20,
        "user".into(),
        "a".repeat(64),
    )?;
    assert!(store.advance_dialogue_continuity(&next, &other).is_err());
    assert_eq!(
        store.dialogue_continuity_checkpoint("chat", "project")?,
        Some(next)
    );
    Ok(())
}
