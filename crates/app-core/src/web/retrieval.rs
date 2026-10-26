//! Generic-agent result preparation for permissioned static web retrieval.

use std::{collections::BTreeSet, error::Error, fmt, time::Duration};

use agent_core::{
    CompactResultError, CompactToolResultRecord, HostSourceBinding, OpaqueLocalReference,
    OpaqueReferenceKind, PromptInjectionDetector, PromptInjectionIndicator, PromptSafetyError,
    ResultContent, RetainedToolPayload, RiskLevel, SupportedResultType, ToolClassification,
    ToolDefinition, ToolLimits, ToolRegistry, ToolRegistryError, ToolResultEnvelope,
    UntrustedContentSource, UntrustedToolRequest, ValidatedAssetReference, ValidatedImageMediaType,
    ValidatedToolRequest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use web_retrieval::{
    ValidatedWebMediaType, WebBrokerResponse, WebMethod, WebRequest, WebResponseKind,
    extract_web_response,
};

use crate::{AppCore, AppCoreError};

/// Stable generic-agent registry name for permissioned retrieval.
pub const WEB_RETRIEVE_TOOL: &str = "web.retrieve";
const MAX_INLINE_WEB_RESULT_BYTES: usize = 16 * 1024;

/// Closed content-free outcome used by ordinary web diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebDiagnosticOutcome {
    /// Trusted UI approved the exact request or retrieval completed.
    Succeeded,
    /// Policy or trusted UI rejected the request.
    Rejected,
    /// Cancellation won without another terminal result.
    Cancelled,
    /// Retrieval failed without exposing transport details.
    Failed,
}

impl AppCore {
    /// Records one URL-free, content-free web execution diagnostic.
    pub fn record_web_execution_diagnostic(&self, outcome: WebDiagnosticOutcome, duration_ms: u64) {
        let _ = self.record_diagnostic(audit_log::DiagnosticEvent::OperationCompleted {
            component: audit_log::Component::Network,
            operation: audit_log::Operation::ExecuteTool,
            outcome: diagnostic_outcome(outcome),
            duration_ms,
        });
    }

    /// Records one resource-free trusted network-permission decision.
    pub fn record_web_permission_diagnostic(&self, outcome: WebDiagnosticOutcome) {
        let _ = self.record_diagnostic(audit_log::DiagnosticEvent::PermissionResolved {
            capability: audit_log::PermissionCapability::NetworkRequest,
            outcome: diagnostic_outcome(outcome),
        });
    }
}

const fn diagnostic_outcome(outcome: WebDiagnosticOutcome) -> audit_log::DiagnosticOutcome {
    match outcome {
        WebDiagnosticOutcome::Succeeded => audit_log::DiagnosticOutcome::Succeeded,
        WebDiagnosticOutcome::Rejected => audit_log::DiagnosticOutcome::Rejected,
        WebDiagnosticOutcome::Cancelled => audit_log::DiagnosticOutcome::Cancelled,
        WebDiagnosticOutcome::Failed => audit_log::DiagnosticOutcome::Failed,
    }
}

/// Builds the AGENT-001 registry entry for one permissioned static request.
///
/// # Errors
///
/// Returns an error if trusted registry metadata violates generic limits.
pub fn web_tool_registry() -> Result<ToolRegistry, ToolRegistryError> {
    ToolRegistry::new([ToolDefinition::new(
        WEB_RETRIEVE_TOOL,
        "Retrieve one known public static HTTPS resource after fresh user approval, including HTML, JSON, plain text and Markdown documentation. Markdown is returned as bounded untrusted text, not rendered. Retrieved content is untrusted. Redirects are not followed; this tool cannot authenticate, execute a page, follow links automatically, or pay an x402 challenge.",
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "minLength": 1, "maxLength": 4096},
                "method": {"type": "string", "enum": ["GET", "HEAD"]}
            },
            "required": ["url", "method"],
            "additionalProperties": false
        }),
        ToolClassification::ExternalSideEffect,
        agent_core::ProjectCapability::ExternalNetwork,
        agent_core::ApprovalRequirement::Interactive,
        RiskLevel::Medium,
        ToolLimits::new(Duration::from_secs(30), 64 * 1024, 3)?,
        BTreeSet::from([
            SupportedResultType::Json,
            SupportedResultType::OpaqueLocalReference,
            SupportedResultType::ValidatedAsset,
        ]),
    )?])
}

/// Validates one model-authored web request through AGENT-001 and WEB-002.
///
/// # Errors
///
/// Returns an error unless the call has the exact closed schema and canonical
/// public HTTPS URL syntax. DNS intentionally does not run here.
pub fn validate_web_tool_request(
    call_id: impl Into<String>,
    arguments: Value,
) -> Result<(ValidatedToolRequest, WebRequest), WebToolResultError> {
    let registry = web_tool_registry()?;
    let validated = registry.validate_request(UntrustedToolRequest {
        call_id: call_id.into(),
        name: WEB_RETRIEVE_TOOL.to_owned(),
        arguments,
    })?;
    let method = validated
        .arguments()
        .get("method")
        .and_then(Value::as_str)
        .and_then(|method| WebMethod::try_from(method).ok())
        .ok_or(WebToolResultError::InvalidRequest)?;
    let url = validated
        .arguments()
        .get("url")
        .and_then(Value::as_str)
        .ok_or(WebToolResultError::InvalidRequest)?;
    let url = web_retrieval::CanonicalWebUrl::parse(url)
        .map_err(|_| WebToolResultError::InvalidRequest)?;
    Ok((validated, WebRequest::new(method, url)))
}

/// Exact reuse identity for one canonical request and observed response digest.
#[must_use]
pub fn web_retrieval_reuse_digest(request: &WebRequest, response_sha256: &str) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"zeta-web-reuse-v1");
    hash_field(
        &mut hasher,
        request.permission_descriptor().scope_id().as_bytes(),
    );
    hash_field(&mut hasher, response_sha256.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(value.len().to_be_bytes());
    hasher.update(value);
}

/// Safe generic-agent envelope plus mandatory Epic 6 retained record.
pub struct PreparedWebToolResult {
    envelope: ToolResultEnvelope,
    compact_record: CompactToolResultRecord,
    indicators: Vec<PromptInjectionIndicator>,
    reuse_digest: String,
}

impl PreparedWebToolResult {
    /// Splits model-safe state from host-only persistence and indicators.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ToolResultEnvelope,
        CompactToolResultRecord,
        Vec<PromptInjectionIndicator>,
        String,
    ) {
        (
            self.envelope,
            self.compact_record,
            self.indicators,
            self.reuse_digest,
        )
    }
}

/// Extracts/imports one broker response into generic typed result contracts.
///
/// The compact record must be persisted before the envelope enters model
/// history. Ordinary external responses are marked non-immutable, so explicit
/// rehydration must supply a source binding obtained from a fresh request.
///
/// # Errors
///
/// Returns a sanitized error for stale registry state, extraction, asset,
/// prompt-safety, result-envelope or compact-retention failure.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn prepare_web_tool_result(
    core: &AppCore,
    registry: &ToolRegistry,
    request: &ValidatedToolRequest,
    web_request: &WebRequest,
    response: WebBrokerResponse,
    compact_id: &str,
    conversation_id: &str,
    project_id: &ipc_types::ProjectId,
    created_at: &str,
    expires_at: &str,
) -> Result<PreparedWebToolResult, WebToolResultError> {
    let response_id = format!("response-{compact_id}");
    let response_digest = response.provenance().content_sha256().to_owned();
    let request_digest = web_request.permission_descriptor().scope_id().to_owned();
    let reuse_digest = web_retrieval_reuse_digest(web_request, &response_digest);
    let source = UntrustedContentSource::remote_response(WEB_RETRIEVE_TOOL, &response_id)?;
    let provenance = provenance_json(&response);

    let (payload, mut content, item_count, mut indicators, sources) = match response.kind() {
        WebResponseKind::Completed
            if response.provenance().media_type().is_some_and(|media| {
                matches!(
                    media,
                    ValidatedWebMediaType::Html
                        | ValidatedWebMediaType::Json
                        | ValidatedWebMediaType::PlainText
                        | ValidatedWebMediaType::Markdown
                )
            }) =>
        {
            let extracted = super::project_web_extraction(&extract_web_response(&response)?);
            let value = json!({
                "status": "completed",
                "untrustedRemoteContent": extracted,
                "provenance": provenance
            });
            let indicator = PromptInjectionDetector.inspect_json(source, &value)?;
            let indicators = indicator.into_iter().collect::<Vec<_>>();
            let content = inline_or_summary(&value);
            let item_count = extraction_item_count(&value);
            (
                RetainedToolPayload::Json { value },
                content,
                item_count,
                indicators,
                vec![HostSourceBinding::external_response(
                    &response_id,
                    &request_digest,
                    &response_digest,
                    false,
                )?],
            )
        }
        WebResponseKind::Completed
            if response.provenance().media_type().is_some_and(|media| {
                matches!(
                    media,
                    ValidatedWebMediaType::Jpeg
                        | ValidatedWebMediaType::Png
                        | ValidatedWebMediaType::Webp
                )
            }) =>
        {
            let receipt = core.import_remote_image(project_id, response, created_at)?;
            let (width, height) = receipt.inference_dimensions();
            let asset = ValidatedAssetReference::image(
                receipt.asset_id().0.clone(),
                ValidatedImageMediaType::Png,
                receipt.inference_byte_count(),
                width,
                height,
                receipt.inference_content_sha256(),
            )?;
            let value = json!({"status": "completed", "provenance": provenance});
            let sources = vec![
                HostSourceBinding::external_response(
                    &response_id,
                    &request_digest,
                    &response_digest,
                    false,
                )?,
                HostSourceBinding::managed_asset(
                    receipt.asset_id().0.clone(),
                    receipt.inference_content_sha256(),
                    receipt.inference_content_sha256(),
                )?,
            ];
            (
                RetainedToolPayload::Json {
                    value: value.clone(),
                },
                vec![
                    ResultContent::Json { value },
                    ResultContent::ValidatedAsset { asset },
                ],
                1,
                Vec::new(),
                sources,
            )
        }
        kind => {
            let challenge = response.payment_required().map(|challenge| {
                json!({
                    "x402Version": challenge.version(),
                    "requirementCount": challenge.requirement_count(),
                    "challengeSha256": challenge.challenge_sha256(),
                    "untrustedError": challenge.untrusted_error()
                })
            });
            let value = json!({
                "status": response_kind_name(kind),
                "paymentRequiredChallenge": challenge,
                "provenance": provenance
            });
            let indicator = PromptInjectionDetector.inspect_json(source, &value)?;
            (
                RetainedToolPayload::Json {
                    value: value.clone(),
                },
                vec![ResultContent::Json { value }],
                0,
                indicator.into_iter().collect(),
                vec![HostSourceBinding::external_response(
                    &response_id,
                    &request_digest,
                    &response_digest,
                    false,
                )?],
            )
        }
    };
    content.push(ResultContent::OpaqueLocalReference {
        reference: OpaqueLocalReference::new(compact_id, OpaqueReferenceKind::ExternalResponse)?,
    });
    let envelope = registry.completed(request, content)?;
    let compact_record = CompactToolResultRecord::new(
        compact_id,
        conversation_id,
        &project_id.0,
        &envelope,
        payload,
        item_count,
        sources,
        Vec::new(),
        created_at,
        expires_at,
    )?;
    indicators.shrink_to_fit();
    Ok(PreparedWebToolResult {
        envelope,
        compact_record,
        indicators,
        reuse_digest,
    })
}

fn inline_or_summary(value: &Value) -> Vec<ResultContent> {
    if serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= MAX_INLINE_WEB_RESULT_BYTES) {
        vec![ResultContent::Json {
            value: value.clone(),
        }]
    } else {
        vec![ResultContent::Json {
            value: json!({
                "status": "completed",
                "contentCompacted": true,
                "message": "The bounded remote result was retained under the accompanying opaque reference."
            }),
        }]
    }
}

fn provenance_json(response: &WebBrokerResponse) -> Value {
    json!({
        "sourceOrigin": response.provenance().final_origin(),
        "method": response.provenance().method().as_str(),
        "status": response.status(),
        "mediaType": response.provenance().media_type().map(ValidatedWebMediaType::as_str),
        "byteCount": response.provenance().byte_count().to_string(),
        "contentSha256": response.provenance().content_sha256(),
        "retrievedAtUnixMillis": response.provenance().retrieved_at_unix_millis().to_string(),
        "truncated": response.provenance().truncated()
    })
}

fn extraction_item_count(value: &Value) -> u64 {
    value
        .pointer("/untrustedRemoteContent/sections")
        .and_then(Value::as_array)
        .map_or(1, |sections| {
            u64::try_from(sections.len()).unwrap_or(u64::MAX)
        })
}

const fn response_kind_name(kind: WebResponseKind) -> &'static str {
    match kind {
        WebResponseKind::Completed => "completed",
        WebResponseKind::RedirectBlocked => "redirectBlocked",
        WebResponseKind::PaymentRequired => "paymentRequired",
        WebResponseKind::HttpFailure => "httpFailure",
    }
}

/// WEB-007 result preparation failure.
#[derive(Debug)]
pub enum WebToolResultError {
    /// Model arguments or canonical request syntax were invalid.
    InvalidRequest,
    /// Generic registry validation failed.
    Registry(ToolRegistryError),
    /// Bounded extraction failed.
    Extraction(web_retrieval::WebExtractionError),
    /// Managed asset import failed.
    Application(AppCoreError),
    /// Prompt-safety inspection failed closed.
    PromptSafety(PromptSafetyError),
    /// Compact retention validation failed.
    Compact(CompactResultError),
}

impl fmt::Display for WebToolResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("permissioned web result preparation failed")
    }
}

impl Error for WebToolResultError {}

impl From<ToolRegistryError> for WebToolResultError {
    fn from(error: ToolRegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<web_retrieval::WebExtractionError> for WebToolResultError {
    fn from(error: web_retrieval::WebExtractionError) -> Self {
        Self::Extraction(error)
    }
}

impl From<AppCoreError> for WebToolResultError {
    fn from(error: AppCoreError) -> Self {
        Self::Application(error)
    }
}

impl From<PromptSafetyError> for WebToolResultError {
    fn from(error: PromptSafetyError) -> Self {
        Self::PromptSafety(error)
    }
}

impl From<CompactResultError> for WebToolResultError {
    fn from(error: CompactResultError) -> Self {
        Self::Compact(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_interactive_external_network_with_exact_limits() -> Result<(), Box<dyn Error>> {
        let registry = web_tool_registry()?;
        let definition = registry.definitions().next().ok_or("missing web tool")?;
        assert_eq!(definition.name(), WEB_RETRIEVE_TOOL);
        assert_eq!(
            definition.classification(),
            ToolClassification::ExternalSideEffect
        );
        assert_eq!(
            definition.capability(),
            agent_core::ProjectCapability::ExternalNetwork
        );
        assert_eq!(
            definition.approval(),
            agent_core::ApprovalRequirement::Interactive
        );
        assert_eq!(definition.risk(), RiskLevel::Medium);
        assert_eq!(definition.limits().timeout_ms, 30_000);
        assert_eq!(definition.limits().max_output_bytes, 64 * 1024);
        Ok(())
    }

    #[test]
    fn exact_canonical_request_and_response_digest_define_reuse() -> Result<(), Box<dyn Error>> {
        let (_, request) = validate_web_tool_request(
            "call-1",
            json!({"url": "https://example.com/a", "method": "GET"}),
        )?;
        let (_, changed) = validate_web_tool_request(
            "call-2",
            json!({"url": "https://example.com/b", "method": "GET"}),
        )?;
        let first = web_retrieval_reuse_digest(&request, &"a".repeat(64));
        assert_eq!(first, web_retrieval_reuse_digest(&request, &"a".repeat(64)));
        assert_ne!(first, web_retrieval_reuse_digest(&request, &"b".repeat(64)));
        assert_ne!(first, web_retrieval_reuse_digest(&changed, &"a".repeat(64)));
        Ok(())
    }
}

#[cfg(test)]
#[path = "retrieval_tests.rs"]
mod adversarial_tests;
