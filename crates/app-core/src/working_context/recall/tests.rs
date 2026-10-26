use super::*;
use ipc_types::{ConversationId, MessageId, MessageRole, ProjectId};

fn conversation() -> Conversation {
    let id = ConversationId("chat".into());
    Conversation {
        id: id.clone(),
        project_id: ProjectId("project".into()),
        title: "Task".into(),
        created_at: "2026-09-10T12:00:00Z".into(),
        updated_at: "2026-09-10T12:00:00Z".into(),
        messages: (0..17)
            .map(|index| Message {
                id: MessageId(format!("message-{index}")),
                conversation_id: id.clone(),
                role: MessageRole::User,
                parts: vec![MessagePart::Text {
                    text: format!("Exact input {index}: مرحبا"),
                }],
                created_at: "2026-09-10T12:00:00Z".into(),
            })
            .collect(),
    }
}

#[test]
fn cap_plus_one_is_discoverable_and_exact_recall_survives_omission()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let conversation = conversation();
    save(&core, &conversation)?;
    let prior = crate::working_context::continuity::prepare(&core, &conversation)?;
    let next = crate::working_context::continuity::pressure_candidate(&conversation, &prior)?;
    crate::working_context::continuity::commit_pressure(&core, &conversation, &prior, &next)?;
    let first = manifest(&conversation, Some(&conversation.messages[16..]), 0)?;
    assert_eq!(first["total"], 17);
    assert_eq!(first["entries"].as_array().map(Vec::len), Some(16));
    assert_eq!(first["nextOffset"], 16);
    assert_eq!(first["entries"][0]["selection"], "retained_omitted");
    let second = core.recall_conversation_context(&conversation, "", 16, 4096)?;
    assert!(second["nextOffset"].is_null());
    let reference = first["entries"][0]["reference"]
        .as_str()
        .ok_or("missing reference")?;
    let mut recovered = String::new();
    let mut offset = 0;
    loop {
        let page = core.recall_conversation_context(&conversation, reference, offset, 4)?;
        recovered.push_str(page["content"].as_str().ok_or("missing content")?);
        match page["nextOffset"].as_u64() {
            Some(next) => offset = usize::try_from(next)?,
            None => break,
        }
    }
    assert_eq!(recovered, "Exact input 0: مرحبا");
    Ok(())
}

#[test]
fn scope_tampering_and_reasoning_never_become_recallable() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let mut conversation = conversation();
    conversation.messages[0].parts.push(MessagePart::Reasoning {
        text: "PRIVATE_REASONING".into(),
    });
    save(&core, &conversation)?;
    let index = manifest(&conversation, Some(&[]), 0)?;
    assert!(!index.to_string().contains("PRIVATE_REASONING"));
    assert_eq!(index["total"], 17);
    let reference = index["entries"][0]["reference"]
        .as_str()
        .ok_or("missing reference")?;
    let mut changed = conversation.clone();
    changed.project_id = ProjectId("other".into());
    assert_eq!(
        core.recall_conversation_context(&changed, reference, 0, 4096)?["status"],
        "unavailable"
    );
    changed = conversation;
    changed.messages[0].parts[0] = MessagePart::Text {
        text: "tampered".into(),
    };
    core.with_storage(|storage| storage.save_conversation(&changed))?;
    assert_eq!(
        core.recall_conversation_context(&changed, reference, 0, 4096)?["status"],
        "unavailable"
    );
    Ok(())
}

#[test]
fn slices_reject_invalid_boundaries_and_report_coverage() {
    assert_eq!(slice("éx", 1, 8)["status"], "invalid_slice");
    assert_eq!(slice("éx", 0, 1)["status"], "invalid_slice");
    let first = slice("éx", 0, 2);
    assert_eq!(first["content"], "é");
    assert_eq!(first["nextOffset"], 2);
    assert_eq!(first["sourceFreshness"], "unverified");
    assert_eq!(first["progressCredited"], false);
}

#[test]
fn result_index_is_paginated_without_losing_omitted_producers()
-> Result<(), Box<dyn std::error::Error>> {
    let mut conversation = conversation();
    for (index, message) in conversation.messages.iter_mut().enumerate() {
        message.role = MessageRole::Tool;
        message.parts = vec![MessagePart::ToolResult {
            result: ipc_types::ToolResult {
                call_id: ipc_types::ToolCallId(format!("producer-{index}")),
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Text {
                        text: format!("exact result {index}"),
                    }],
                },
            },
        }];
    }
    let page = manifest(&conversation, Some(&[]), 0)?;
    assert_eq!(page["nextOffset"], 16);
    let page = manifest(&conversation, Some(&[]), 16)?;
    assert_eq!(page["entries"][0]["toolCallId"], "producer-16");
    assert_eq!(page["entries"][0]["selection"], "retained_omitted");
    Ok(())
}

#[test]
fn missing_pixels_are_a_gap_not_an_available_payload() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let mut conversation = conversation();
    conversation.messages[0].parts = vec![MessagePart::Asset {
        asset_id: ipc_types::AssetId("missing-image".into()),
    }];
    let file = app_storage::AssetFileReference {
        storage_key: "inference/missing-image.png".into(),
        media_type: "image/png".into(),
        content_digest: "a".repeat(64),
        byte_size: 24,
        width: 1,
        height: 1,
    };
    core.with_storage(|storage| {
        storage.save_asset(&app_storage::AssetReference {
            id: ipc_types::AssetId("missing-image".into()),
            original: app_storage::AssetFileReference {
                storage_key: "originals/missing-image.png".into(),
                ..file.clone()
            },
            derivative: file,
            imported_at: conversation.created_at.clone(),
        })
    })?;
    save(&core, &conversation)?;
    let index = manifest(&conversation, Some(&[]), 0)?;
    let reference = index["entries"][0]["reference"]
        .as_str()
        .ok_or("missing reference")?;
    let recalled = core.recall_conversation_context(&conversation, reference, 0, 4096)?;
    assert_eq!(recalled["status"], "unavailable");
    assert_eq!(recalled["contextGap"], true);
    Ok(())
}

fn retained_record(
    id: &str,
    expires: &str,
    sources: Vec<HostSourceBinding>,
) -> Result<agent_core::CompactToolResultRecord, Box<dyn std::error::Error>> {
    use agent_core::{
        ApprovalRequirement, ProjectCapability, ResultContent, RetainedToolPayload, RiskLevel,
        SupportedResultType, ToolClassification, ToolDefinition, ToolLimits, ToolRegistry,
        UntrustedToolRequest,
    };
    let registry = ToolRegistry::new([ToolDefinition::new(
        "web.retrieve",
        "Read bounded public information",
        json!({"type":"object", "properties":{}, "required":[], "additionalProperties":false}),
        ToolClassification::Read,
        ProjectCapability::WorkspaceRead,
        ApprovalRequirement::CapabilityOnly,
        RiskLevel::Low,
        ToolLimits::new(std::time::Duration::from_secs(5), 4096, 1)?,
        [SupportedResultType::Text],
    )?])?;
    let call = registry.validate_request(UntrustedToolRequest {
        call_id: "retained-call".into(),
        name: "web.retrieve".into(),
        arguments: json!({}),
    })?;
    let envelope = registry.completed(
        &call,
        vec![ResultContent::Text {
            text: "bounded preview".into(),
        }],
    )?;
    Ok(agent_core::CompactToolResultRecord::new(
        id,
        "chat",
        "project",
        &envelope,
        RetainedToolPayload::Text {
            text: "exact retained content".into(),
        },
        1,
        sources,
        Vec::new(),
        "2000-01-01T00:00:00Z",
        expires,
    )?)
}

#[test]
fn retained_payload_recall_obeys_scope_expiry_and_live_source_guards()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let conversation = conversation();
    core.with_storage(|storage| {
        storage.save_project(&crate::Project {
            id: conversation.project_id.clone(),
            name: "Test".into(),
            created_at: conversation.created_at.clone(),
            updated_at: conversation.updated_at.clone(),
        })?;
        storage.save_conversation(&conversation)
    })?;
    for record in [
        retained_record("retained", "2099-01-01T00:00:00Z", vec![])?,
        retained_record("expired", "2001-01-01T00:00:00Z", vec![])?,
        retained_record(
            "mutable-web",
            "2099-01-01T00:00:00Z",
            vec![HostSourceBinding::external_response(
                "response",
                "a".repeat(64),
                "b".repeat(64),
                false,
            )?],
        )?,
        retained_record(
            "workspace",
            "2099-01-01T00:00:00Z",
            vec![HostSourceBinding::workspace(
                "primary",
                "data.txt",
                "grant",
                "v1",
                "r1",
                "a".repeat(64),
            )?],
        )?,
    ] {
        core.with_storage(|storage| storage.save_compact_tool_result(&record))?;
    }
    let recalled = core.recall_conversation_context(&conversation, "retained", 0, 4096)?;
    assert_eq!(recalled["status"], "recalled");
    assert!(
        recalled["content"]
            .as_str()
            .is_some_and(|text| text.contains("exact retained content"))
    );
    assert_eq!(
        core.recall_conversation_context(&conversation, "expired", 0, 4096)?["status"],
        "unavailable"
    );
    assert_eq!(
        core.recall_conversation_context(&conversation, "mutable-web", 0, 4096)?["status"],
        "stale"
    );
    assert_eq!(
        core.recall_conversation_context(&conversation, "workspace", 0, 4096)?["status"],
        "stale"
    );
    let mut other = conversation.clone();
    other.project_id = ProjectId("other".into());
    assert_eq!(
        core.recall_conversation_context(&other, "retained", 0, 4096)?["status"],
        "unavailable"
    );
    core.with_storage(|storage| storage.delete_conversation_and_reap_assets(&conversation.id))?;
    assert_eq!(
        core.recall_conversation_context(&conversation, "retained", 0, 4096)?["status"],
        "unavailable"
    );
    Ok(())
}

fn save(core: &AppCore, conversation: &Conversation) -> Result<(), AppCoreError> {
    core.with_storage(|storage| {
        storage.save_project(&crate::Project {
            id: conversation.project_id.clone(),
            name: "Test".into(),
            created_at: conversation.created_at.clone(),
            updated_at: conversation.updated_at.clone(),
        })?;
        storage.save_conversation(conversation)
    })
}

#[test]
fn standalone_index_does_not_claim_visible_input_was_omitted()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::TempDir::new()?;
    let core = AppCore::initialize(dir.path().join("Zeta"))?;
    let mut conversation = conversation();
    conversation.messages.truncate(1);
    save(&core, &conversation)?;
    let index = core.recall_conversation_context(&conversation, "", 0, 4096)?;
    assert_eq!(index["entries"][0]["selection"], "unknown");
    assert!(index["entries"][0]["selectedVerbatim"].is_null());
    assert!(index["includedCount"].is_null());
    assert!(index["retainedOmittedCount"].is_null());
    let selected = manifest(&conversation, Some(&conversation.messages), 0)?;
    assert_eq!(selected["entries"][0]["selection"], "included");
    assert_eq!(selected["entries"][0]["selectedVerbatim"], true);
    assert_eq!(selected["retainedOmittedCount"], 0);
    let reference = index["entries"][0]["reference"]
        .as_str()
        .ok_or("reference")?;
    let recalled = core.recall_conversation_context(&conversation, reference, 0, 4096)?;
    assert_eq!(recalled["content"], "Exact input 0: مرحبا");
    Ok(())
}

#[test]
fn verbatim_count_includes_entries_beyond_the_first_page() -> Result<(), Box<dyn std::error::Error>>
{
    let conversation = conversation();
    let full = manifest(&conversation, Some(&conversation.messages), 0)?;
    assert_eq!(full["selectedVerbatimCount"], 17);
    let partial = manifest(&conversation, Some(&conversation.messages[..16]), 0)?;
    assert_eq!(partial["selectedVerbatimCount"], 16);
    assert_eq!(partial["retainedOmittedCount"], 1);
    Ok(())
}
