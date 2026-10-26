use std::error::Error;

use agent_core::UntrustedWorkingContextSummary;
use ipc_types::{Conversation, ConversationId, ModelSelection, ProjectId};
use tempfile::NamedTempFile;

use super::*;
use crate::Project;

const NOW: &str = "2026-08-18T10:00:00Z";

fn checkpoint(covered: u32) -> Result<SemanticCompactionCheckpoint, Box<dyn Error>> {
    Ok(SemanticCompactionCheckpoint::new(
        "conversation-1",
        "project-default",
        covered,
        format!("message-{covered}"),
        "a".repeat(64),
        "b".repeat(64),
        1,
        "c".repeat(64),
        UntrustedWorkingContextSummary::new(
            "Original request",
            vec!["Keep compatibility".to_owned()],
            vec!["Use a narrow module".to_owned()],
            Vec::new(),
            Vec::new(),
            vec!["src/lib.rs".to_owned()],
            Vec::new(),
        )?,
        NOW,
    )?)
}

fn save_scope(store: &mut ApplicationStore) -> Result<(), StorageError> {
    store.save_project(&Project {
        id: ProjectId("project-default".to_owned()),
        name: "Test project".to_owned(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    store.save_conversation(&Conversation {
        id: ConversationId("conversation-1".to_owned()),
        project_id: ProjectId("project-default".to_owned()),
        title: "Context".to_owned(),
        messages: Vec::new(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })
}

#[test]
fn checkpoint_survives_restart_and_updates_incrementally() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    {
        let mut store = ApplicationStore::open(database.path())?;
        save_scope(&mut store)?;
        store.save_semantic_compaction_checkpoint(&checkpoint(2)?)?;
        store.save_semantic_compaction_checkpoint(&checkpoint(4)?)?;
    }
    let store = ApplicationStore::open(database.path())?;
    let restored = store
        .semantic_compaction_checkpoint("conversation-1", "project-default")?
        .ok_or("checkpoint missing")?;
    assert_eq!(restored.covered_message_count(), 4);
    assert_eq!(restored.covered_message_id(), "message-4");
    Ok(())
}

#[test]
fn checkpoint_scope_and_conversation_lifecycle_are_enforced() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_scope(&mut store)?;
    store.save_semantic_compaction_checkpoint(&checkpoint(2)?)?;
    assert!(
        store
            .semantic_compaction_checkpoint("conversation-1", "other-project")?
            .is_none()
    );
    let conversation_id = ConversationId("conversation-1".to_owned());
    let (deleted, _) = store.delete_conversation_and_reap_assets(&conversation_id)?;
    assert!(deleted);
    assert!(
        store
            .semantic_compaction_checkpoint("conversation-1", "project-default")?
            .is_none()
    );
    Ok(())
}

#[test]
fn changing_model_invalidates_checkpoint_without_changing_history() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_scope(&mut store)?;
    store.save_semantic_compaction_checkpoint(&checkpoint(2)?)?;
    let selection = ModelSelection {
        model_id: "qwen/qwen3.8-27b".to_owned(),
        model_version: "release-1".to_owned(),
    };
    assert!(
        store.set_conversation_model(&ConversationId("conversation-1".to_owned()), &selection)?
    );
    assert_eq!(
        store.conversation_model(&ConversationId("conversation-1".to_owned()))?,
        Some(selection)
    );
    assert!(
        store
            .semantic_compaction_checkpoint("conversation-1", "project-default")?
            .is_none()
    );
    assert!(
        store
            .conversation(&ConversationId("conversation-1".to_owned()))?
            .is_some()
    );
    Ok(())
}
