//! Persistence and retention tests for compact tool results.

use std::{error::Error, time::Duration};

use agent_core::{
    ApprovalRequirement, CompactRehydrationContext, CompactToolResultRecord,
    ComputationAppliedLimits, ComputationInputProvenance, ComputationProvenanceRecord,
    ComputationTerminalStatus, HostSourceBinding, ModelVisibleExcerpt, ProjectCapability,
    ResultContent, RetainedToolPayload, RiskLevel, SupportedResultType, ToolClassification,
    ToolDefinition, ToolLimits, ToolRegistry, UntrustedToolRequest,
};
use ipc_types::{AssetId, Conversation, ConversationId, ProjectId};
use tempfile::NamedTempFile;

use super::*;
use crate::{AssetFileReference, Project};

const NOW: &str = "2026-08-14T12:00:00Z";

fn save_test_scope(store: &mut ApplicationStore) -> Result<(), StorageError> {
    store.save_project(&Project {
        id: ProjectId("project-default".to_owned()),
        name: "Test project".to_owned(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })?;
    store.save_conversation(&Conversation {
        id: ConversationId("conversation-1".to_owned()),
        project_id: ProjectId("project-default".to_owned()),
        title: "Agent run".to_owned(),
        messages: Vec::new(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })
}

fn compact_result_record(id: &str, asset: bool) -> Result<CompactToolResultRecord, Box<dyn Error>> {
    let registry = ToolRegistry::new([ToolDefinition::new(
        "filesystem.read_text",
        "Read one bounded project file.",
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false
        }),
        ToolClassification::Read,
        ProjectCapability::WorkspaceRead,
        ApprovalRequirement::CapabilityOnly,
        RiskLevel::Low,
        ToolLimits::new(Duration::from_secs(5), 4_096, 1)?,
        [SupportedResultType::Text],
    )?])?;
    let request = registry.validate_request(UntrustedToolRequest {
        call_id: "call-read".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
    })?;
    let result = registry.completed(
        &request,
        vec![ResultContent::Text {
            text: "compact".to_owned(),
        }],
    )?;
    let mut sources = vec![HostSourceBinding::workspace(
        "primary",
        "src/lib.rs",
        "capability-secret",
        "capability-revision-1",
        "resource-r1",
        "a".repeat(64),
    )?];
    if asset {
        sources.push(HostSourceBinding::managed_asset(
            "asset-compact",
            "asset-r1",
            "b".repeat(64),
        )?);
    }
    Ok(CompactToolResultRecord::new(
        id,
        "conversation-1",
        "project-default",
        &result,
        RetainedToolPayload::Text {
            text: "large retained content outside conversation history".to_owned(),
        },
        42,
        sources,
        vec![ModelVisibleExcerpt::new("selected result excerpt")?],
        "2026-08-18T10:00:00Z",
        "2026-08-25T10:00:00Z",
    )?)
}

fn compact_asset() -> AssetReference {
    AssetReference {
        id: AssetId("asset-compact".to_owned()),
        original: AssetFileReference {
            storage_key: "originals/asset-compact.png".to_owned(),
            media_type: "image/png".to_owned(),
            content_digest: "c".repeat(64),
            byte_size: 42,
            width: 2,
            height: 3,
        },
        derivative: AssetFileReference {
            storage_key: "inference/asset-compact.png".to_owned(),
            media_type: "image/png".to_owned(),
            content_digest: "b".repeat(64),
            byte_size: 24,
            width: 2,
            height: 3,
        },
        imported_at: NOW.to_owned(),
    }
}

#[test]
fn survives_restart_and_rehydrates_only_under_exact_live_scope() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let record = compact_result_record("compact-1", false)?;
    {
        let mut store = ApplicationStore::open(database.path())?;
        save_test_scope(&mut store)?;
        store.save_compact_tool_result(&record)?;
    }

    let mut reopened = ApplicationStore::open(database.path())?;
    let restored = reopened
        .compact_tool_result("compact-1", "conversation-1", "project-default")?
        .ok_or("compact result missing after restart")?;
    assert!(
        reopened
            .compact_tool_result("compact-1", "conversation-1", "another-project")?
            .is_none()
    );
    let context = CompactRehydrationContext::new(
        "project-default",
        "conversation-1",
        vec![HostSourceBinding::workspace(
            "primary",
            "src/lib.rs",
            "capability-secret",
            "capability-revision-1",
            "resource-r1",
            "a".repeat(64),
        )?],
    )?;
    assert!(matches!(
        restored.rehydrate(&context, "2026-08-19T10:00:00Z")?,
        RetainedToolPayload::Text { .. }
    ));
    let visible = serde_json::to_string(&restored.model_projection()?)?;
    assert!(!visible.contains("large retained content"));
    assert!(!visible.contains("capability-secret"));

    let conversation_id = ConversationId("conversation-1".to_owned());
    let (deleted, _) = reopened.delete_conversation_and_reap_assets(&conversation_id)?;
    assert!(deleted);
    assert!(
        reopened
            .compact_tool_result("compact-1", "conversation-1", "project-default")?
            .is_none()
    );
    Ok(())
}

#[test]
fn computation_provenance_survives_compact_result_restart() -> Result<(), Box<dyn Error>> {
    let database = NamedTempFile::new()?;
    let source = HostSourceBinding::workspace(
        "primary",
        "src/lib.rs",
        "capability-secret",
        "capability-revision-1",
        "resource-r1",
        "a".repeat(64),
    )?;
    let provenance = ComputationProvenanceRecord::new(
        "zeta-contract-1",
        "quickjs-test-runtime",
        "export default () => zeta.text('input');",
        serde_json::json!({"source": "export default () => zeta.text('input');"}),
        vec![ComputationInputProvenance::new(
            "input", "utf8", 42, source,
        )?],
        ComputationAppliedLimits {
            cpu_time_ms: 3_000,
            wall_time_ms: 5_000,
            process_memory_bytes: 128 * 1024 * 1024,
            javascript_heap_bytes: 64 * 1024 * 1024,
            javascript_stack_bytes: 512 * 1024,
            source_bytes: 65_536,
            total_input_bytes: 8 * 1024 * 1024,
            output_bytes: 4_096,
            output_items: 1,
        },
        ComputationTerminalStatus::Completed,
        Some("c".repeat(64)),
        Vec::new(),
    )?;
    let record = compact_result_record("compact-computation", false)?
        .with_computation_provenance(provenance)?;
    {
        let mut store = ApplicationStore::open(database.path())?;
        save_test_scope(&mut store)?;
        store.save_compact_tool_result(&record)?;
    }
    let reopened = ApplicationStore::open(database.path())?;
    let restored = reopened
        .compact_tool_result("compact-computation", "conversation-1", "project-default")?
        .ok_or("computation result missing after restart")?;
    let restored_provenance = restored
        .computation_provenance()
        .ok_or("computation provenance missing after restart")?;
    assert_eq!(restored_provenance.script_sha256().len(), 64);
    assert_eq!(
        restored_provenance.terminal_status(),
        ComputationTerminalStatus::Completed
    );
    let visible = serde_json::to_string(&restored.model_projection()?)?;
    assert!(!visible.contains("export default"));
    assert!(!visible.contains("capability-secret"));
    Ok(())
}

#[test]
fn retention_keeps_assets_live_then_reaps_them() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_scope(&mut store)?;
    let asset = compact_asset();
    store.save_asset(&asset)?;
    store.save_compact_tool_result(&compact_result_record("compact-asset", true)?)?;
    assert!(store.unreferenced_asset_ids()?.is_empty());
    assert!(store.delete_unreferenced_asset(&asset.id)?.is_none());

    let (deleted, reaped) = store.reap_expired_compact_results("2026-08-24T10:00:00Z")?;
    assert_eq!(deleted, 0);
    assert!(reaped.is_empty());
    assert!(store.asset(&asset.id)?.is_some());

    let (deleted, reaped) = store.reap_expired_compact_results("2026-08-25T10:00:00Z")?;
    assert_eq!(deleted, 1);
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].id, asset.id);
    assert!(store.asset(&asset.id)?.is_none());
    Ok(())
}

#[test]
fn rejects_missing_assets_and_scope_rebinding() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_scope(&mut store)?;
    assert!(matches!(
        store.save_compact_tool_result(&compact_result_record("compact-missing", true)?),
        Err(StorageError::InvalidRecord(_))
    ));

    store.save_compact_tool_result(&compact_result_record("compact-once", false)?)?;
    assert!(
        store
            .save_compact_tool_result(&compact_result_record("compact-once", false)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn loading_retained_payload_revalidates_current_asset_digest() -> Result<(), Box<dyn Error>> {
    let mut store = ApplicationStore::open_in_memory()?;
    save_test_scope(&mut store)?;
    store.save_asset(&compact_asset())?;
    let record = compact_result_record("asset-revalidation", true)?;
    save(&mut store, &record)?;
    store.connection.execute(
        "UPDATE assets SET content_digest = ?1 WHERE id = ?2",
        params!["d".repeat(64), "asset-compact"],
    )?;
    assert!(
        load(
            &store,
            record.id(),
            record.conversation_id(),
            record.project_id()
        )
        .is_err()
    );
    Ok(())
}
