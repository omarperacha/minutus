use std::time::Duration;

use super::*;
use crate::{
    ApprovalRequirement, OpaqueLocalReference, OpaqueReferenceKind, ProjectCapability, RiskLevel,
    SupportedResultType, ToolClassification, ToolLimits, ToolOutcome, ValidatedAssetReference,
    ValidatedImageMediaType,
};

fn definition() -> Result<ToolDefinition, ToolRegistryError> {
    ToolDefinition::new(
        "filesystem.inspect_image",
        "Inspect a validated project image.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "workspace": { "type": "string", "enum": ["primary"] },
                "path": { "type": "string", "minLength": 1, "maxLength": 256 }
            },
            "required": ["workspace", "path"],
            "additionalProperties": false
        }),
        ToolClassification::Read,
        ProjectCapability::WorkspaceRead,
        ApprovalRequirement::CapabilityOnly,
        RiskLevel::Low,
        ToolLimits::new(Duration::from_secs(5), 2_048, 3)?,
        [
            SupportedResultType::ValidatedAsset,
            SupportedResultType::Text,
        ],
    )
}

fn registry() -> Result<ToolRegistry, ToolRegistryError> {
    ToolRegistry::new([definition()?])
}

fn valid_request() -> UntrustedToolRequest {
    UntrustedToolRequest {
        call_id: "request-1:tool:1".to_owned(),
        name: "filesystem.inspect_image".to_owned(),
        arguments: serde_json::json!({ "workspace": "primary", "path": "diagram.png" }),
    }
}

#[test]
fn registry_rejects_unknown_tools_arguments_and_schema_violations() -> Result<(), ToolRegistryError>
{
    let registry = registry()?;
    let mut unknown = valid_request();
    unknown.name = "filesystem.shell".to_owned();
    assert!(matches!(
        registry.validate_request(unknown),
        Err(ToolRegistryError::UnknownTool(_))
    ));

    let mut extra = valid_request();
    extra.arguments["command"] = serde_json::json!("cat ~/.ssh/id_rsa");
    assert!(matches!(
        registry.validate_request(extra),
        Err(ToolRegistryError::InvalidArguments { .. })
    ));

    let mut wrong_type = valid_request();
    wrong_type.arguments["path"] = serde_json::json!(false);
    assert!(matches!(
        registry.validate_request(wrong_type),
        Err(ToolRegistryError::InvalidArguments { .. })
    ));
    Ok(())
}

#[test]
fn trusted_metadata_is_not_taken_from_the_model_projection() -> Result<(), ToolRegistryError> {
    let registry = registry()?;
    let projected = registry.model_definitions();
    assert_eq!(projected[0].name, "filesystem.inspect_image");
    let encoded = serde_json::to_string(&projected)
        .map_err(|error| ToolRegistryError::InvalidResult(error.to_string()))?;
    assert!(!encoded.contains("capability"));
    assert!(!encoded.contains("approval"));
    assert!(!encoded.contains("timeout"));
    let validated = registry.validate_request(valid_request())?;
    assert_eq!(
        validated.definition().capability(),
        ProjectCapability::WorkspaceRead
    );
    assert_eq!(validated.definition().risk(), RiskLevel::Low);
    Ok(())
}

#[test]
fn model_projection_renders_through_the_active_adapter() -> Result<(), Box<dyn std::error::Error>> {
    let tools = registry()?
        .model_definitions()
        .into_iter()
        .map(|tool| model_runtime::PromptToolDefinition {
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        })
        .collect();
    let rendered = model_runtime::render_model_prompt(
        model_runtime::ModelAdapterKind::GlimmerAtem,
        &model_runtime::PromptRenderRequest {
            messages: vec![model_runtime::PromptMessage::User {
                content: vec![model_runtime::PromptContent::Text(
                    "Inspect diagram.png".to_owned(),
                )],
            }],
            tools,
            historical_tool_names: Vec::new(),
            current_date: "2026-08-17".to_owned(),
            reasoning_strength: model_runtime::ReasoningStrength::High,
            add_generation_prompt: true,
        },
    )?;
    assert!(rendered.contains("filesystem.inspect_image"));
    assert!(rendered.contains("additionalProperties"));
    assert!(!rendered.contains("WorkspaceRead"));
    assert!(!rendered.contains("timeout_ms"));
    Ok(())
}

#[test]
fn result_envelope_is_typed_bounded_and_asset_reference_only() -> Result<(), ToolRegistryError> {
    let registry = registry()?;
    let request = registry.validate_request(valid_request())?;
    let asset = ValidatedAssetReference::image(
        "asset-018f",
        ValidatedImageMediaType::Png,
        512,
        32,
        16,
        "a".repeat(64),
    )?;
    let completed = registry.completed(&request, vec![ResultContent::ValidatedAsset { asset }])?;
    assert!(matches!(completed.outcome(), ToolOutcome::Completed { .. }));
    let encoded = serde_json::to_string(&completed)
        .map_err(|error| ToolRegistryError::InvalidResult(error.to_string()))?;
    assert!(encoded.contains("asset-018f"));
    assert!(!encoded.contains("bytes"));
    assert!(!encoded.contains("path"));

    let failure = ToolFailure::new("image_invalid", "The image could not be validated", false)?;
    assert!(matches!(
        registry.failed(&request, failure)?.outcome(),
        ToolOutcome::Failed { .. }
    ));
    assert!(matches!(
        registry
            .cancelled(&request, CancellationReason::UserRequested)?
            .outcome(),
        ToolOutcome::Cancelled { .. }
    ));
    Ok(())
}

#[test]
fn unsupported_and_oversized_results_are_rejected() -> Result<(), ToolRegistryError> {
    let registry = registry()?;
    let request = registry.validate_request(valid_request())?;
    let reference = OpaqueLocalReference::new("change-1", OpaqueReferenceKind::Change)?;
    assert_eq!(
        registry.completed(
            &request,
            vec![ResultContent::OpaqueLocalReference { reference }]
        ),
        Err(ToolRegistryError::UnsupportedResultType)
    );
    assert_eq!(
        registry.completed(
            &request,
            vec![ResultContent::Text {
                text: "x".repeat(4_096)
            }]
        ),
        Err(ToolRegistryError::ResultLimitExceeded)
    );
    Ok(())
}

#[test]
fn duplicate_registry_entries_fail() -> Result<(), ToolRegistryError> {
    let first = definition()?;
    assert!(matches!(
        ToolRegistry::new([first.clone(), first]),
        Err(ToolRegistryError::DuplicateTool(_))
    ));
    Ok(())
}
