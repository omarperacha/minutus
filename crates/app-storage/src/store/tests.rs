use std::{
    error::Error,
    fs,
    sync::{Arc, Barrier},
    time::Duration,
};

use agent_core::{
    ApprovalRequirement, ExecutionBudgets, LedgerCapability, LedgerNextAction, LedgerRequest,
    NewMutationChange, ProjectCapability, ResultContent, RiskLevel, SupportedResultType,
    ToolClassification, ToolDefinition, ToolLimits, ToolRegistry, UntrustedToolRequest,
    WorkspaceResolution,
};
use ipc_types::{ConversationId, MessageId};
use rusqlite::Connection;
use tempfile::NamedTempFile;

use super::*;
use crate::{AuditEventKind, AuditOutcome, SCHEMA_VERSION, SettingValue, Theme};

const NOW: &str = "2026-08-14T12:00:00Z";

fn save_test_project(store: &ApplicationStore) -> Result<(), StorageError> {
    store.save_project(&Project {
        id: ProjectId("project-default".to_owned()),
        name: "Test project".to_owned(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })
}

fn save_test_conversation(
    store: &mut ApplicationStore,
    conversation_id: &ConversationId,
) -> Result<(), StorageError> {
    store.save_conversation(&Conversation {
        id: conversation_id.clone(),
        project_id: ProjectId("project-default".to_owned()),
        title: "Agent run".to_owned(),
        messages: Vec::new(),
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
    })
}

fn execution_ledger_snapshot() -> Result<ExecutionLedgerSnapshot, Box<dyn Error>> {
    let registry = ToolRegistry::new([ToolDefinition::new(
        "filesystem.write_text",
        "Write one bounded project file.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string" },
                "path": { "type": "string" }
            },
            "required": ["workspace", "path"],
            "additionalProperties": false
        }),
        ToolClassification::Write,
        ProjectCapability::WorkspaceWrite,
        ApprovalRequirement::CapabilityOnly,
        RiskLevel::Low,
        ToolLimits::new(Duration::from_secs(5), 4_096, 1)?,
        [SupportedResultType::Text],
    )?])?;
    let request = registry.validate_request(UntrustedToolRequest {
        call_id: "call-write".to_owned(),
        name: "filesystem.write_text".to_owned(),
        arguments: serde_json::json!({"workspace": "primary", "path": "src/lib.rs"}),
    })?;
    let result = registry.completed(
        &request,
        vec![ResultContent::Text {
            text: "applied".to_owned(),
        }],
    )?;
    let mut ledger = ExecutionLedger::new(
        "run-1",
        LedgerRequest::new(
            "request-1",
            "conversation-1",
            "project-default",
            "Update src/lib.rs",
        )?,
        agent_core::build_agent_system_instructions("2026-08-17", None)?
            .identity()
            .clone(),
        ExecutionBudgets {
            rounds: 4,
            tool_calls: 8,
            elapsed_ms: 30_000,
            tool_ms: 10_000,
            context_tokens: 4_000,
            final_response_tokens: 500,
            tool_output_bytes: 32_000,
        },
        LedgerNextAction::Generate,
    )?;
    ledger.resolve_workspace(WorkspaceResolution::new(
        "primary",
        "capability-secret",
        "capability-revision-1",
        LedgerCapability::ReadWrite,
    )?)?;
    ledger.record_mutation(
        &request,
        &result,
        "result-write",
        "mutation-1",
        vec![NewMutationChange {
            workspace_alias: "primary".to_owned(),
            relative_path: "src/lib.rs".to_owned(),
            previous_sha256: Some("a".repeat(64)),
            resulting_sha256: "b".repeat(64),
            resource_revision: "resource-r2".to_owned(),
        }],
    )?;
    Ok(ledger.snapshot()?)
}

mod conversations;
mod core;
mod evm;
mod metadata;
