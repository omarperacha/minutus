use std::error::Error;

use agent_core::{RichEvidenceCheckpoint, RichEvidenceSource};
use ipc_types::{Conversation, ConversationId, ModelSelection, ProjectId};
use tempfile::NamedTempFile;

use super::*;
use crate::Project;

const NOW: &str = "2026-08-23T10:00:00Z";

fn checkpoint() -> Result<RichEvidenceCheckpoint, Box<dyn Error>> {
    Ok(RichEvidenceCheckpoint::new(
        "conversation-1",
        "project-default",
        2,
        "message-2",
        "a".repeat(64),
        "b".repeat(64),
        1,
        "c".repeat(64),
        vec![RichEvidenceSource::new("asset-1", "d".repeat(64))?],
        "The image contains two columns.",
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
        title: "Evidence".to_owned(),
        messages: Vec::new(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })
}

#[test]
fn checkpoint_survives_restart_and_conversation_deletion_reaps_it() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    {
        let mut store = ApplicationStore::open(database.path())?;
        save_scope(&mut store)?;
        store.save_rich_evidence_checkpoint(&checkpoint()?)?;
    }
    let mut store = ApplicationStore::open(database.path())?;
    let restored = store
        .rich_evidence_checkpoint("conversation-1", "project-default")?
        .ok_or("checkpoint missing")?;
    assert_eq!(restored.extraction(), "The image contains two columns.");
    store.delete_conversation_and_reap_assets(&ConversationId("conversation-1".to_owned()))?;
    assert!(
        store
            .rich_evidence_checkpoint("conversation-1", "project-default")?
            .is_none()
    );
    Ok(())
}

#[test]
fn changing_the_model_invalidates_rich_evidence() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_scope(&mut store)?;
    store.save_rich_evidence_checkpoint(&checkpoint()?)?;
    assert!(store.set_conversation_model(
        &ConversationId("conversation-1".to_owned()),
        &ModelSelection {
            model_id: "model".to_owned(),
            model_version: "1".to_owned(),
        },
    )?);
    assert!(
        store
            .rich_evidence_checkpoint("conversation-1", "project-default")?
            .is_none()
    );
    Ok(())
}
