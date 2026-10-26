//! Unit tests for the compact-result feature boundary.

use std::{error::Error, time::Duration};

use super::*;
use crate::{
    ApprovalRequirement, CancellationReason, ProjectCapability, ResultContent, RiskLevel,
    SupportedResultType, ToolClassification, ToolDefinition, ToolFailure, ToolLimits, ToolRegistry,
    ToolResultEnvelope, UntrustedToolRequest,
};

fn result() -> Result<ToolResultEnvelope, Box<dyn Error>> {
    let registry = ToolRegistry::new([ToolDefinition::new(
        "filesystem.read_text",
        "Read one bounded file.",
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
        ToolLimits::new(Duration::from_secs(1), 4_096, 1)?,
        [SupportedResultType::Text],
    )?])?;
    let request = registry.validate_request(UntrustedToolRequest {
        call_id: "call-1".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
    })?;
    Ok(registry.completed(
        &request,
        vec![ResultContent::Text {
            text: "bounded result".to_owned(),
        }],
    )?)
}

fn workspace_source(revision: &str) -> Result<HostSourceBinding, CompactResultError> {
    HostSourceBinding::workspace(
        "primary",
        "src/lib.rs",
        "secret-capability-id",
        "secret-capability-revision",
        revision,
        "a".repeat(64),
    )
}

fn record() -> Result<CompactToolResultRecord, Box<dyn Error>> {
    Ok(CompactToolResultRecord::new(
        "compact-1",
        "conversation-1",
        "project-1",
        &result()?,
        RetainedToolPayload::Text {
            text: "complete retained payload that must not enter the prompt".to_owned(),
        },
        12,
        vec![
            workspace_source("resource-r1")?,
            HostSourceBinding::managed_asset("asset-1", "asset-r1", "b".repeat(64))?,
        ],
        vec![ModelVisibleExcerpt::new("selected safe excerpt")?],
        "2026-08-18T10:00:00Z",
        "2026-08-25T10:00:00Z",
    )?)
}

#[test]
fn projection_is_bounded_and_omits_retained_content_and_authority() -> Result<(), Box<dyn Error>> {
    let record = record()?;
    let projection = record.model_projection()?;
    let visible = serde_json::to_string(&projection)?;
    assert!(visible.contains("compact-1"));
    assert!(visible.contains("src/lib.rs"));
    assert!(visible.contains("selected safe excerpt"));
    assert!(!visible.contains("complete retained payload"));
    assert!(!visible.contains("secret-capability"));
    assert!(!visible.contains("/Users/"));
    assert!(!visible.contains("bookmark"));
    assert_eq!(projection.status, CompactResultStatus::Completed);
    Ok(())
}

#[test]
fn rehydration_requires_exact_live_scope_capability_and_source_revision()
-> Result<(), Box<dyn Error>> {
    let record = record()?;
    let exact = CompactRehydrationContext::new(
        "project-1",
        "conversation-1",
        vec![
            workspace_source("resource-r1")?,
            HostSourceBinding::managed_asset("asset-1", "asset-r1", "b".repeat(64))?,
        ],
    )?;
    assert!(matches!(
        record.rehydrate(&exact, "2026-08-19T10:00:00Z")?,
        RetainedToolPayload::Text { .. }
    ));

    let changed = CompactRehydrationContext::new(
        "project-1",
        "conversation-1",
        vec![
            workspace_source("resource-r2")?,
            HostSourceBinding::managed_asset("asset-1", "asset-r1", "b".repeat(64))?,
        ],
    )?;
    let changed_error = record
        .rehydrate(&changed, "2026-08-19T10:00:00Z")
        .err()
        .ok_or("changed source unexpectedly rehydrated")?;
    assert!(changed_error.is_recoverable());
    assert!(matches!(
        changed_error,
        CompactResultError::SourceChangedOrUnavailable
    ));

    let wrong_scope =
        CompactRehydrationContext::new("project-2", "conversation-1", exact.sources.clone())?;
    assert!(matches!(
        record.rehydrate(&wrong_scope, "2026-08-19T10:00:00Z"),
        Err(CompactResultError::ScopeMismatch)
    ));
    assert!(matches!(
        record.rehydrate(&exact, "2026-08-25T10:00:00Z"),
        Err(CompactResultError::Expired)
    ));
    Ok(())
}

#[test]
fn mutable_external_response_requires_a_fresh_exact_observation() -> Result<(), Box<dyn Error>> {
    let source =
        HostSourceBinding::external_response("response-1", "c".repeat(64), "d".repeat(64), false)?;
    let record = CompactToolResultRecord::new(
        "compact-web-1",
        "conversation-1",
        "project-1",
        &result()?,
        RetainedToolPayload::Text {
            text: "retained remote observation".to_owned(),
        },
        1,
        vec![source.clone()],
        Vec::new(),
        "2026-08-18T10:00:00Z",
        "2026-08-25T10:00:00Z",
    )?;
    let retained_only =
        CompactRehydrationContext::new("project-1", "conversation-1", vec![source.clone()])?;
    assert!(matches!(
        record.rehydrate(&retained_only, "2026-08-19T10:00:00Z"),
        Err(CompactResultError::SourceChangedOrUnavailable)
    ));
    let refreshed = CompactRehydrationContext::after_fresh_external_retrieval(
        "project-1",
        "conversation-1",
        vec![source],
        ["response-1".to_owned()],
    )?;
    assert!(record.rehydrate(&refreshed, "2026-08-19T10:00:00Z").is_ok());
    Ok(())
}

#[test]
fn corrupted_payload_and_unsafe_excerpts_fail_closed() -> Result<(), Box<dyn Error>> {
    for unsafe_excerpt in [
        "/Users/alice/private.txt",
        "Authorization: Bearer abc",
        "bookmark bytes",
        "hidden diagnostic details",
        "see file:///Users/alice/private.txt",
        "credential sk-1234567890abcdefghijklmnopqrstuvwxyz",
    ] {
        assert!(matches!(
            ModelVisibleExcerpt::new(unsafe_excerpt),
            Err(CompactResultError::UnsafeModelProjection)
        ));
        assert!(
            serde_json::from_value::<ModelVisibleExcerpt>(serde_json::json!(unsafe_excerpt))
                .is_err()
        );
    }

    let mut encoded = serde_json::to_value(record()?)?;
    encoded["payload"]["text"] = serde_json::json!("tampered");
    let decoded: CompactToolResultRecord = serde_json::from_value(encoded)?;
    assert!(matches!(
        decoded.validate(),
        Err(CompactResultError::DigestMismatch)
    ));
    Ok(())
}

#[test]
fn duplicate_sources_and_invalid_retention_fail_closed() -> Result<(), Box<dyn Error>> {
    let source = workspace_source("resource-r1")?;
    assert!(matches!(
        CompactToolResultRecord::new(
            "compact-duplicate",
            "conversation-1",
            "project-1",
            &result()?,
            RetainedToolPayload::Json {
                value: serde_json::json!({"count": 2})
            },
            2,
            vec![source.clone(), source],
            Vec::new(),
            "2026-08-18T10:00:00Z",
            "2026-08-25T10:00:00Z",
        ),
        Err(CompactResultError::InvalidRecord)
    ));
    assert!(matches!(
        CompactToolResultRecord::new(
            "compact-expiry",
            "conversation-1",
            "project-1",
            &result()?,
            RetainedToolPayload::Text {
                text: "payload".to_owned()
            },
            1,
            Vec::new(),
            Vec::new(),
            "2026-08-25T10:00:00Z",
            "2026-08-18T10:00:00Z",
        ),
        Err(CompactResultError::InvalidRecord)
    ));
    Ok(())
}

#[test]
fn failed_and_cancelled_statuses_are_preserved() -> Result<(), Box<dyn Error>> {
    let registry = ToolRegistry::new([ToolDefinition::new(
        "filesystem.read_text",
        "Read one bounded file.",
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
        ToolLimits::new(Duration::from_secs(1), 4_096, 1)?,
        [SupportedResultType::Text],
    )?])?;
    let request = registry.validate_request(UntrustedToolRequest {
        call_id: "call-status".to_owned(),
        name: "filesystem.read_text".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
    })?;
    let failed = registry.failed(
        &request,
        ToolFailure::new("unavailable", "Source is unavailable", true)?,
    )?;
    let cancelled = registry.cancelled(&request, CancellationReason::UserRequested)?;
    for (id, result, expected) in [
        ("compact-failed", failed, CompactResultStatus::Failed),
        (
            "compact-cancelled",
            cancelled,
            CompactResultStatus::Cancelled,
        ),
    ] {
        let record = CompactToolResultRecord::new(
            id,
            "conversation-1",
            "project-1",
            &result,
            RetainedToolPayload::Text {
                text: "bounded retained diagnostic-free payload".to_owned(),
            },
            0,
            Vec::new(),
            Vec::new(),
            "2026-08-18T10:00:00Z",
            "2026-08-25T10:00:00Z",
        )?;
        assert_eq!(record.status(), expected);
        assert_eq!(record.model_projection()?.status, expected);
    }
    Ok(())
}
