//! Semantic validation applied after strict Serde decoding.

use crate::{
    AssetId, ChatRequest, ConversationId, CreateProjectRequest, DictationBinding, DictationCommand,
    DictationEventsRequest, DictationSessionId, FilesystemFindPathsRequest,
    FilesystemInspectImageRequest, FilesystemListRequest, FilesystemReadTextRequest,
    FilesystemReplaceTextRequest, FilesystemScanRequest, FilesystemSearchRequest,
    FilesystemStatRequest, FilesystemUndoRequest, FilesystemWriteTextRequest, GenerationEvent,
    IpcError, JsonValue, MAX_CHAT_ASSETS, MAX_CHAT_TEXT_BYTES, MAX_CONVERSATION_TITLE_BYTES,
    MAX_FINAL_RESPONSE_BYTES, MAX_GENERATION_DELTA_BYTES, MAX_PERMISSION_REASON_BYTES,
    MAX_PROJECT_NAME_BYTES, MAX_TOOL_ARGUMENT_BYTES, MAX_TRANSACTION_FIELDS, ModelDownloadRequest,
    PermissionKind, PermissionRequest, ProjectId, RenameConversationRequest, StartDictationRequest,
    TaskActivity, ToolRequest, TransactionProposal, UpdateProjectWorkspacePermissionRequest,
    WorkspaceGrantId,
};

const MAX_ID_BYTES: usize = 128;
const MAX_COMPOSER_ID_BYTES: usize = 128;
const MAX_SAFE_JS_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_NODES: usize = 4096;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DISPLAY_FIELD_BYTES: usize = 16 * 1024;
const MAX_ACTIVITY_TITLE_BYTES: usize = 512;
const MAX_ACTIVITY_DETAIL_BYTES: usize = 16 * 1024;
const MAX_MODEL_COMPANIONS: usize = 8;
const MAX_FILESYSTEM_SEARCH_QUERY_BYTES: usize = 256;
const MAX_FILESYSTEM_FIND_QUERIES: usize = 8;
const MAX_FILESYSTEM_FIND_NAME_BYTES: usize = 256;
const MAX_FILESYSTEM_FIND_EXTENSIONS: usize = 16;
const MAX_FILESYSTEM_FIND_DEPTH: usize = 32;
const MAX_FILESYSTEM_SCAN_EXTENSIONS: usize = 32;
const MAX_FILESYSTEM_EXTENSION_BYTES: usize = 16;
const MAX_FILESYSTEM_WRITE_FILES: usize = 8;
const MAX_FILESYSTEM_WRITE_FILE_BYTES: usize = 128 * 1024;
const MAX_FILESYSTEM_WRITE_TOTAL_BYTES: usize = 192 * 1024;
const MAX_FILESYSTEM_TEXT_REPLACEMENTS: usize = 16;
const MAX_FILESYSTEM_TEXT_REPLACEMENT_BYTES: usize = 64 * 1024;
const MAX_FILESYSTEM_TEXT_REPLACEMENT_OCCURRENCES: usize = 32;

/// Semantic and size validation required before processing an IPC value.
pub trait ValidateIpc {
    /// Validates a decoded value, returning a sanitized error on failure.
    ///
    /// # Errors
    ///
    /// Returns an error when a field is empty, malformed or exceeds an
    /// explicit IPC limit.
    fn validate(&self) -> Result<(), IpcError>;
}

impl ValidateIpc for ConversationId {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("conversationId", &self.0)
    }
}

impl ValidateIpc for ProjectId {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("projectId", &self.0)
    }
}

impl ValidateIpc for DictationSessionId {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("sessionId", &self.0)
    }
}

impl ValidateIpc for DictationBinding {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        if let Some(conversation_id) = &self.conversation_id {
            conversation_id.validate()?;
        }
        validate_non_empty_bounded("composerId", &self.composer_id, MAX_COMPOSER_ID_BYTES)?;
        if !self.composer_id.is_ascii() || self.composer_id.trim() != self.composer_id {
            return Err(IpcError::invalid_request("composerId is invalid"));
        }
        if self.draft_revision > MAX_SAFE_JS_INTEGER {
            return Err(IpcError::invalid_request("draftRevision is invalid"));
        }
        Ok(())
    }
}

impl ValidateIpc for StartDictationRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.binding.validate()?;
        self.model.validate()?;
        if let Some(language) = &self.language
            && (!(2..=8).contains(&language.len())
                || !language
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-'))
        {
            return Err(IpcError::invalid_request("language is invalid"));
        }
        Ok(())
    }
}

impl ValidateIpc for DictationCommand {
    fn validate(&self) -> Result<(), IpcError> {
        self.session_id.validate()?;
        self.binding.validate()
    }
}

impl ValidateIpc for DictationEventsRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.session_id.validate()?;
        self.binding.validate()?;
        if self.after_sequence > MAX_SAFE_JS_INTEGER {
            return Err(IpcError::invalid_request("afterSequence is invalid"));
        }
        Ok(())
    }
}

impl ValidateIpc for CreateProjectRequest {
    fn validate(&self) -> Result<(), IpcError> {
        validate_non_empty_bounded("name", &self.name, MAX_PROJECT_NAME_BYTES)?;
        if self.name.trim() != self.name {
            return Err(IpcError::invalid_request("name must be trimmed"));
        }
        Ok(())
    }
}

impl ValidateIpc for AssetId {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("assetId", &self.0)
    }
}

impl ValidateIpc for WorkspaceGrantId {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("grantId", &self.0)
    }
}

impl ValidateIpc for UpdateProjectWorkspacePermissionRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()
    }
}

impl ValidateIpc for FilesystemListRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)
    }
}

impl ValidateIpc for FilesystemFindPathsRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        if self.queries.is_empty() || self.queries.len() > MAX_FILESYSTEM_FIND_QUERIES {
            return Err(IpcError::payload_too_large(format!(
                "queries must contain 1 to {MAX_FILESYSTEM_FIND_QUERIES} filters"
            )));
        }
        for query in &self.queries {
            validate_relative_path(&query.relative_path)?;
            if query.name_contains.len() > MAX_FILESYSTEM_FIND_NAME_BYTES {
                return Err(IpcError::payload_too_large(format!(
                    "nameContains exceeds the {MAX_FILESYSTEM_FIND_NAME_BYTES}-byte limit"
                )));
            }
            if query.name_contains.is_empty() && query.extensions.is_empty() {
                return Err(IpcError::invalid_request(
                    "nameContains or at least one extension is required",
                ));
            }
            if query.name_contains.chars().any(char::is_control) {
                return Err(IpcError::invalid_request(
                    "nameContains must not contain control characters",
                ));
            }
            if query.max_depth == 0 || query.max_depth > MAX_FILESYSTEM_FIND_DEPTH {
                return Err(IpcError::invalid_request(format!(
                    "maxDepth must be between 1 and {MAX_FILESYSTEM_FIND_DEPTH}"
                )));
            }
            if query.extensions.len() > MAX_FILESYSTEM_FIND_EXTENSIONS {
                return Err(IpcError::payload_too_large(format!(
                    "extensions exceeds the limit of {MAX_FILESYSTEM_FIND_EXTENSIONS}"
                )));
            }
            for extension in &query.extensions {
                validate_non_empty_bounded("extension", extension, MAX_FILESYSTEM_EXTENSION_BYTES)?;
                if !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                    return Err(IpcError::invalid_request(
                        "extensions must not contain dots or path separators",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl ValidateIpc for FilesystemStatRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)
    }
}

impl ValidateIpc for FilesystemReadTextRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)?;
        if self.relative_path == "." {
            return Err(IpcError::invalid_request(
                "relativePath must name a workspace file",
            ));
        }
        Ok(())
    }
}

impl ValidateIpc for FilesystemInspectImageRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)?;
        if self.relative_path == "." {
            return Err(IpcError::invalid_request(
                "relativePath must name a workspace image",
            ));
        }
        Ok(())
    }
}

impl ValidateIpc for FilesystemSearchRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)?;
        validate_non_empty_bounded("query", &self.query, MAX_FILESYSTEM_SEARCH_QUERY_BYTES)
    }
}

impl ValidateIpc for FilesystemScanRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)?;
        if self.excluded_extensions.len() > MAX_FILESYSTEM_SCAN_EXTENSIONS {
            return Err(IpcError::payload_too_large(format!(
                "excludedExtensions exceeds the limit of {MAX_FILESYSTEM_SCAN_EXTENSIONS}"
            )));
        }
        for extension in &self.excluded_extensions {
            validate_non_empty_bounded(
                "excluded extension",
                extension,
                MAX_FILESYSTEM_EXTENSION_BYTES,
            )?;
            if !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                return Err(IpcError::invalid_request(
                    "excluded extensions must not contain dots or path separators",
                ));
            }
        }
        Ok(())
    }
}

impl ValidateIpc for FilesystemWriteTextRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        if self.changes.is_empty() || self.changes.len() > MAX_FILESYSTEM_WRITE_FILES {
            return Err(IpcError::payload_too_large(format!(
                "changes must contain 1 to {MAX_FILESYSTEM_WRITE_FILES} files"
            )));
        }
        let mut paths = std::collections::HashSet::with_capacity(self.changes.len());
        let mut total = 0_usize;
        let parent_aware = self
            .changes
            .iter()
            .filter(|change| change.create_missing_parents)
            .count();
        if parent_aware > 0
            && (self.changes.len() != 1
                || parent_aware != 1
                || self.changes[0].expected_sha256.is_some())
        {
            return Err(IpcError::invalid_request(
                "createMissingParents requires exactly one new-file change",
            ));
        }
        for change in &self.changes {
            validate_relative_path(&change.relative_path)?;
            if !paths.insert(change.relative_path.as_str()) {
                return Err(IpcError::invalid_request(
                    "changes contains a duplicate path",
                ));
            }
            validate_bounded("content", &change.content, MAX_FILESYSTEM_WRITE_FILE_BYTES)?;
            if change.content.contains('\0') {
                return Err(IpcError::invalid_request("content must be UTF-8 text"));
            }
            if let Some(digest) = &change.expected_sha256
                && (digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
            {
                return Err(IpcError::invalid_request(
                    "expectedSha256 must be a lowercase SHA-256 digest",
                ));
            }
            total = total.saturating_add(change.content.len());
        }
        if total > MAX_FILESYSTEM_WRITE_TOTAL_BYTES {
            return Err(IpcError::payload_too_large(format!(
                "replacement content exceeds {MAX_FILESYSTEM_WRITE_TOTAL_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

impl ValidateIpc for FilesystemUndoRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.conversation_id.validate()?;
        validate_id("writeCallId", &self.write_call_id.0)
    }
}

impl ValidateIpc for FilesystemReplaceTextRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        self.grant_id.validate()?;
        validate_relative_path(&self.relative_path)?;
        if self.relative_path == "." {
            return Err(IpcError::invalid_request(
                "relativePath must name a workspace file",
            ));
        }
        if self.expected_sha256.len() != 64
            || !self
                .expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(IpcError::invalid_request(
                "expectedSha256 must be a lowercase SHA-256 digest",
            ));
        }
        if self.replacements.is_empty()
            || self.replacements.len() > MAX_FILESYSTEM_TEXT_REPLACEMENTS
        {
            return Err(IpcError::payload_too_large(format!(
                "replacements must contain 1 to {MAX_FILESYSTEM_TEXT_REPLACEMENTS} items"
            )));
        }
        let mut total = 0_usize;
        for replacement in &self.replacements {
            validate_non_empty_bounded(
                "oldText",
                &replacement.old_text,
                MAX_FILESYSTEM_TEXT_REPLACEMENT_BYTES,
            )?;
            validate_bounded(
                "newText",
                &replacement.new_text,
                MAX_FILESYSTEM_TEXT_REPLACEMENT_BYTES,
            )?;
            if replacement.old_text.contains('\0') || replacement.new_text.contains('\0') {
                return Err(IpcError::invalid_request(
                    "replacement text must not contain null bytes",
                ));
            }
            if replacement.expected_occurrences == 0
                || replacement.expected_occurrences > MAX_FILESYSTEM_TEXT_REPLACEMENT_OCCURRENCES
            {
                return Err(IpcError::invalid_request(format!(
                    "expectedOccurrences must be 1 to {MAX_FILESYSTEM_TEXT_REPLACEMENT_OCCURRENCES}"
                )));
            }
            total = total
                .saturating_add(replacement.old_text.len())
                .saturating_add(replacement.new_text.len());
        }
        if total > MAX_FILESYSTEM_TEXT_REPLACEMENT_BYTES {
            return Err(IpcError::payload_too_large(format!(
                "replacement snippets exceed {MAX_FILESYSTEM_TEXT_REPLACEMENT_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

impl ValidateIpc for RenameConversationRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.conversation_id.validate()?;
        let title = self.title.trim();
        validate_non_empty_bounded("title", title, MAX_CONVERSATION_TITLE_BYTES)
    }
}

impl ValidateIpc for ChatRequest {
    fn validate(&self) -> Result<(), IpcError> {
        self.project_id.validate()?;
        validate_non_empty_bounded("text", &self.text, MAX_CHAT_TEXT_BYTES)?;

        if let Some(context_tokens) = self.context_tokens
            && !(16_384..=262_144).contains(&context_tokens)
        {
            return Err(IpcError::invalid_request(
                "The selected capacity is invalid",
            ));
        }
        if let Some(selection) = &self.model_selection {
            validate_id("modelId", &selection.model_id)?;
            validate_id("modelVersion", &selection.model_version)?;
        }

        if self.asset_ids.len() > MAX_CHAT_ASSETS {
            return Err(IpcError::payload_too_large(format!(
                "assetIds exceeds the limit of {MAX_CHAT_ASSETS}"
            )));
        }

        if let Some(conversation_id) = &self.conversation_id {
            validate_id("conversationId", &conversation_id.0)?;
        }

        for asset_id in &self.asset_ids {
            validate_id("assetId", &asset_id.0)?;
        }

        Ok(())
    }
}

impl ValidateIpc for ModelDownloadRequest {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("modelId", &self.model_id)?;
        validate_id("modelVersion", &self.model_version)?;
        if self.companion_artifact_ids.len() > MAX_MODEL_COMPANIONS {
            return Err(IpcError::payload_too_large(format!(
                "companionArtifactIds exceeds the limit of {MAX_MODEL_COMPANIONS}"
            )));
        }
        let mut unique =
            std::collections::HashSet::with_capacity(self.companion_artifact_ids.len());
        for artifact_id in &self.companion_artifact_ids {
            validate_id("companionArtifactId", artifact_id)?;
            if !unique.insert(artifact_id) {
                return Err(IpcError::invalid_request(
                    "companionArtifactIds contains a duplicate",
                ));
            }
        }
        Ok(())
    }
}

impl ValidateIpc for ToolRequest {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("callId", &self.call_id.0)?;

        let encoded = serde_json::to_vec(&self.arguments).map_err(|_| IpcError::internal())?;
        if encoded.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err(IpcError::payload_too_large(format!(
                "arguments exceeds the limit of {MAX_TOOL_ARGUMENT_BYTES} bytes"
            )));
        }

        let mut nodes = 0;
        for (key, value) in &self.arguments {
            validate_non_empty_bounded("argument key", key, MAX_ID_BYTES)?;
            validate_json(value, 1, &mut nodes)?;
        }

        Ok(())
    }
}

impl ValidateIpc for GenerationEvent {
    fn validate(&self) -> Result<(), IpcError> {
        match self {
            Self::Activity { activity, .. } => activity.validate(),
            Self::TextDelta { text, .. }
            | Self::ReasoningDelta { text, .. }
            | Self::TextPreviewDelta { text, .. } => {
                validate_bounded("generation delta", text, MAX_GENERATION_DELTA_BYTES)
            }
            Self::ToolCall { request, .. } => request.validate(),
            Self::Final { text, .. } => {
                validate_bounded("final response", text, MAX_FINAL_RESPONSE_BYTES)
            }
            Self::ToolResultNeeded { call_id, .. } => validate_id("callId", &call_id.0),
            Self::Status { .. }
            | Self::Error { .. }
            | Self::Cancelled { .. }
            | Self::TextPreviewReset { .. } => Ok(()),
        }
    }
}

impl ValidateIpc for TaskActivity {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("activityId", &self.id)?;
        validate_non_empty_bounded("activity title", &self.title, MAX_ACTIVITY_TITLE_BYTES)?;
        if let Some(detail) = &self.detail {
            validate_bounded("activity detail", detail, MAX_ACTIVITY_DETAIL_BYTES)?;
        }
        Ok(())
    }
}

impl ValidateIpc for PermissionRequest {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("permissionRequestId", &self.id.0)?;
        validate_non_empty_bounded(
            "permission reason",
            &self.reason,
            MAX_PERMISSION_REASON_BYTES,
        )?;

        match &self.kind {
            PermissionKind::WorkspaceRead {
                grant_id,
                relative_path,
            } => {
                validate_id("grantId", &grant_id.0)?;
                validate_relative_path(relative_path)
            }
            PermissionKind::WorkspaceWrite {
                grant_id,
                summary,
                content_hash,
            } => {
                validate_id("grantId", &grant_id.0)?;
                validate_non_empty_bounded("change summary", summary, MAX_DISPLAY_FIELD_BYTES)?;
                validate_non_empty_bounded("contentHash", content_hash, MAX_ID_BYTES)
            }
            PermissionKind::NetworkRequest { origin, method } => {
                validate_non_empty_bounded("origin", origin, MAX_PATH_BYTES)?;
                validate_non_empty_bounded("method", method, 16)
            }
            PermissionKind::TransactionReview { proposal_id } => {
                validate_id("proposalId", &proposal_id.0)
            }
        }
    }
}

impl ValidateIpc for TransactionProposal {
    fn validate(&self) -> Result<(), IpcError> {
        validate_id("proposalId", &self.id.0)?;
        validate_id("walletId", &self.wallet_id.0)?;
        validate_non_empty_bounded("network", &self.network, MAX_ID_BYTES)?;
        validate_non_empty_bounded("account", &self.account, MAX_ID_BYTES)?;
        validate_non_empty_bounded("summary", &self.summary, MAX_DISPLAY_FIELD_BYTES)?;

        if self.fields.len() > MAX_TRANSACTION_FIELDS {
            return Err(IpcError::payload_too_large(format!(
                "transaction fields exceeds the limit of {MAX_TRANSACTION_FIELDS}"
            )));
        }

        for field in &self.fields {
            validate_non_empty_bounded("transaction field label", &field.label, MAX_ID_BYTES)?;
            validate_non_empty_bounded(
                "transaction field value",
                &field.value,
                MAX_DISPLAY_FIELD_BYTES,
            )?;
            if let Some(warning) = &field.warning {
                validate_bounded("transaction warning", warning, MAX_DISPLAY_FIELD_BYTES)?;
            }
        }

        Ok(())
    }
}

fn validate_json(value: &JsonValue, depth: usize, nodes: &mut usize) -> Result<(), IpcError> {
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err(IpcError::payload_too_large(format!(
            "arguments exceeds the limit of {MAX_JSON_NODES} values"
        )));
    }
    if depth > MAX_JSON_DEPTH {
        return Err(IpcError::payload_too_large(format!(
            "arguments exceeds the nesting limit of {MAX_JSON_DEPTH}"
        )));
    }

    match value {
        JsonValue::Number(number) if !number.is_finite() => Err(IpcError::invalid_request(
            "arguments contains a non-finite number",
        )),
        JsonValue::Array(values) => {
            for item in values {
                validate_json(item, depth + 1, nodes)?;
            }
            Ok(())
        }
        JsonValue::Object(values) => {
            for (key, item) in values {
                validate_non_empty_bounded("argument key", key, MAX_ID_BYTES)?;
                validate_json(item, depth + 1, nodes)?;
            }
            Ok(())
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
            Ok(())
        }
    }
}

fn validate_relative_path(path: &str) -> Result<(), IpcError> {
    validate_non_empty_bounded("relativePath", path, MAX_PATH_BYTES)?;
    if path == "." {
        return Ok(());
    }
    if path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || path.contains('\0')
    {
        return Err(IpcError::invalid_request(
            "relativePath must remain inside its workspace grant",
        ));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> Result<(), IpcError> {
    validate_non_empty_bounded(field, value, MAX_ID_BYTES)
}

fn validate_non_empty_bounded(field: &str, value: &str, max_bytes: usize) -> Result<(), IpcError> {
    if value.is_empty() {
        return Err(IpcError::invalid_request(format!(
            "{field} must not be empty"
        )));
    }
    validate_bounded(field, value, max_bytes)
}

fn validate_bounded(field: &str, value: &str, max_bytes: usize) -> Result<(), IpcError> {
    if value.len() > max_bytes {
        return Err(IpcError::payload_too_large(format!(
            "{field} exceeds the limit of {max_bytes} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{AssetId, RequestId, TaskActivityKind, TaskActivityStatus, ToolCallId, ToolName};

    #[test]
    fn rejects_unknown_generation_variant() {
        let decoded = serde_json::from_str::<GenerationEvent>(
            r#"{"type":"futureEvent","requestId":"request-1","sequence":0}"#,
        );
        assert!(decoded.is_err());
    }

    #[test]
    fn rejects_unknown_generation_field() {
        let decoded = serde_json::from_str::<GenerationEvent>(
            r#"{"type":"cancelled","requestId":"request-1","sequence":0,"extra":true}"#,
        );
        assert!(decoded.is_err());
    }

    #[test]
    fn rejects_malformed_generation_event() {
        let decoded = serde_json::from_str::<GenerationEvent>(
            r#"{"type":"textDelta","requestId":"request-1","sequence":"first","text":"hi"}"#,
        );
        assert!(decoded.is_err());
    }

    #[test]
    fn rejects_oversized_chat_text() {
        let request = ChatRequest {
            project_id: ProjectId("project-1".to_owned()),
            conversation_id: None,
            text: "x".repeat(MAX_CHAT_TEXT_BYTES + 1),
            asset_ids: Vec::new(),
            model_selection: None,
            context_tokens: None,
            effort: None,
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn chat_capacity_stays_within_product_bounds() {
        let mut request = ChatRequest {
            project_id: ProjectId("project-1".to_owned()),
            conversation_id: None,
            text: "hello".to_owned(),
            asset_ids: Vec::new(),
            model_selection: None,
            context_tokens: None,
            effort: None,
        };
        for offered in [16_384, 32_768, 65_536, 131_072, 262_144] {
            request.context_tokens = Some(offered);
            assert!(request.validate().is_ok());
        }
        for unsupported in [0, 16_383, 262_145] {
            request.context_tokens = Some(unsupported);
            assert!(request.validate().is_err());
        }
    }

    #[test]
    fn validates_model_download_identity_bounds() {
        let valid = ModelDownloadRequest {
            model_id: "publisher/model".to_owned(),
            model_version: "revision-1".to_owned(),
            companion_artifact_ids: vec!["vision".to_owned()],
        };
        assert!(valid.validate().is_ok());
        let empty = ModelDownloadRequest {
            model_id: String::new(),
            model_version: "revision-1".to_owned(),
            companion_artifact_ids: Vec::new(),
        };
        assert!(empty.validate().is_err());
        let oversized = ModelDownloadRequest {
            model_id: "x".repeat(MAX_ID_BYTES + 1),
            model_version: "revision-1".to_owned(),
            companion_artifact_ids: Vec::new(),
        };
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn rejects_too_many_chat_assets() {
        let request = ChatRequest {
            project_id: ProjectId("project-1".to_owned()),
            conversation_id: None,
            text: "hello".to_owned(),
            asset_ids: (0..=MAX_CHAT_ASSETS)
                .map(|index| AssetId(format!("asset-{index}")))
                .collect(),
            model_selection: None,
            context_tokens: None,
            effort: None,
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn validates_conversation_rename_bounds() {
        let valid = RenameConversationRequest {
            conversation_id: ConversationId("conversation-1".to_owned()),
            title: "Release plan".to_owned(),
        };
        assert_eq!(valid.validate(), Ok(()));

        let blank = RenameConversationRequest {
            conversation_id: valid.conversation_id.clone(),
            title: "   ".to_owned(),
        };
        assert!(blank.validate().is_err());

        let oversized = RenameConversationRequest {
            conversation_id: valid.conversation_id,
            title: "x".repeat(MAX_CONVERSATION_TITLE_BYTES + 1),
        };
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn validates_project_name_bounds_and_whitespace() {
        assert_eq!(
            CreateProjectRequest {
                name: "Personal research".to_owned(),
            }
            .validate(),
            Ok(())
        );
        for name in [String::new(), "  ".to_owned(), " Untrimmed".to_owned()] {
            assert!(CreateProjectRequest { name }.validate().is_err());
        }
        assert!(
            CreateProjectRequest {
                name: "x".repeat(MAX_PROJECT_NAME_BYTES + 1),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validates_strict_filesystem_paths_and_queries() {
        let grant_id = WorkspaceGrantId("grant-1".to_owned());
        let project_id = ProjectId("project-1".to_owned());
        assert!(
            FilesystemListRequest {
                project_id: project_id.clone(),
                grant_id: grant_id.clone(),
                relative_path: ".".to_owned(),
            }
            .validate()
            .is_ok()
        );
        for relative_path in [
            "",
            "/tmp",
            "../secret",
            "src/../secret",
            "./src",
            "src//lib.rs",
        ] {
            assert!(
                FilesystemStatRequest {
                    project_id: project_id.clone(),
                    grant_id: grant_id.clone(),
                    relative_path: relative_path.to_owned(),
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            FilesystemReadTextRequest {
                project_id: project_id.clone(),
                grant_id: grant_id.clone(),
                relative_path: ".".to_owned(),
            }
            .validate()
            .is_err()
        );
        assert!(
            FilesystemSearchRequest {
                project_id: project_id.clone(),
                grant_id: grant_id.clone(),
                relative_path: ".".to_owned(),
                query: String::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            FilesystemScanRequest {
                project_id: project_id.clone(),
                grant_id: grant_id.clone(),
                relative_path: ".".to_owned(),
                recursive: true,
                minimum_line_count: 600,
                excluded_extensions: vec!["md".to_owned()],
            }
            .validate()
            .is_ok()
        );
        assert!(
            FilesystemScanRequest {
                project_id,
                grant_id,
                relative_path: ".".to_owned(),
                recursive: true,
                minimum_line_count: 600,
                excluded_extensions: vec![".md".to_owned()],
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validates_bounded_digest_checked_writes() {
        let request = FilesystemWriteTextRequest {
            project_id: ProjectId("project-1".to_owned()),
            grant_id: WorkspaceGrantId("grant-1".to_owned()),
            changes: vec![crate::FilesystemWriteTextChange {
                relative_path: "src/lib.rs".to_owned(),
                expected_sha256: Some("a".repeat(64)),
                content: "pub fn safe() {}\n".to_owned(),
                create_missing_parents: false,
            }],
        };
        assert!(request.validate().is_ok());

        let mut invalid_digest = request.clone();
        invalid_digest.changes[0].expected_sha256 = Some("A".repeat(64));
        assert!(invalid_digest.validate().is_err());

        let mut duplicate = request;
        duplicate.changes.push(duplicate.changes[0].clone());
        assert!(duplicate.validate().is_err());

        let parent_aware_existing = FilesystemWriteTextRequest {
            project_id: ProjectId("project-1".to_owned()),
            grant_id: WorkspaceGrantId("grant-1".to_owned()),
            changes: vec![crate::FilesystemWriteTextChange {
                relative_path: "new/deep/file.txt".to_owned(),
                expected_sha256: Some("a".repeat(64)),
                content: "content".to_owned(),
                create_missing_parents: true,
            }],
        };
        assert!(parent_aware_existing.validate().is_err());
    }

    #[test]
    fn validates_exact_targeted_text_replacements() {
        let request = FilesystemReplaceTextRequest {
            project_id: ProjectId("project-1".to_owned()),
            grant_id: WorkspaceGrantId("grant-1".to_owned()),
            relative_path: "cv.html".to_owned(),
            expected_sha256: "a".repeat(64),
            replacements: vec![crate::FilesystemTextReplacement {
                old_text: "Old subheader".to_owned(),
                new_text: "ZETA Dev | Real G".to_owned(),
                expected_occurrences: 1,
            }],
        };
        assert!(request.validate().is_ok());

        let mut ambiguous = request.clone();
        ambiguous.replacements[0].expected_occurrences = 0;
        assert!(ambiguous.validate().is_err());

        let mut empty_match = request;
        empty_match.replacements[0].old_text.clear();
        assert!(empty_match.validate().is_err());
    }

    #[test]
    fn rejects_deep_tool_arguments() {
        let mut value = JsonValue::Null;
        for _ in 0..=MAX_JSON_DEPTH {
            value = JsonValue::Array(vec![value]);
        }
        let request = ToolRequest {
            call_id: ToolCallId("call-1".to_owned()),
            name: ToolName::Lookup,
            arguments: BTreeMap::from([("query".to_owned(), value)]),
        };
        assert!(request.validate().is_err());
    }

    #[test]
    fn accepts_bounded_generation_delta() {
        let event = GenerationEvent::TextDelta {
            request_id: RequestId("request-1".to_owned()),
            sequence: 0,
            text: "hello".to_owned(),
        };
        assert_eq!(event.validate(), Ok(()));
    }

    #[test]
    fn generation_status_is_closed_and_content_free() -> Result<(), Box<dyn std::error::Error>> {
        let event = GenerationEvent::Status {
            request_id: RequestId("request-1".to_owned()),
            sequence: 4,
            phase: crate::ChatGenerationPhase::ProcessingContext,
        };
        assert_eq!(event.validate(), Ok(()));
        assert_eq!(
            serde_json::to_value(event)?,
            serde_json::json!({
                "type": "status",
                "requestId": "request-1",
                "sequence": 4,
                "phase": "processingContext"
            })
        );
        Ok(())
    }

    #[test]
    fn preview_events_are_round_scoped_and_bounded() -> Result<(), Box<dyn std::error::Error>> {
        let event = GenerationEvent::TextPreviewDelta {
            request_id: RequestId("request-1".to_owned()),
            sequence: 2,
            round: 1,
            text: "hello 🌍".to_owned(),
        };
        assert_eq!(event.validate(), Ok(()));
        let json = serde_json::to_value(&event)?;
        assert_eq!(json["type"], "textPreviewDelta");
        assert_eq!(json["round"], 1);
        assert_eq!(serde_json::from_value::<GenerationEvent>(json)?, event);
        let oversized = GenerationEvent::TextPreviewDelta {
            request_id: RequestId("request-1".to_owned()),
            sequence: 3,
            round: 1,
            text: "x".repeat(MAX_GENERATION_DELTA_BYTES + 1),
        };
        assert!(oversized.validate().is_err());
        let reset = GenerationEvent::TextPreviewReset {
            request_id: RequestId("request-1".to_owned()),
            sequence: 4,
            round: 2,
        };
        assert_eq!(reset.validate(), Ok(()));
        assert_eq!(
            serde_json::from_str::<GenerationEvent>(&serde_json::to_string(&reset)?)?,
            reset
        );
        Ok(())
    }

    #[test]
    fn validates_bounded_task_activity() {
        let event = GenerationEvent::Activity {
            request_id: RequestId("request-1".to_owned()),
            sequence: 0,
            activity: TaskActivity {
                id: "activity-1".to_owned(),
                kind: TaskActivityKind::PermissionDecision,
                title: "Workspace read approved".to_owned(),
                detail: Some("Read access was approved for this request.".to_owned()),
                status: TaskActivityStatus::Approved,
            },
        };
        assert_eq!(event.validate(), Ok(()));

        let oversized = TaskActivity {
            id: "activity-2".to_owned(),
            kind: TaskActivityKind::TransactionStatus,
            title: "x".repeat(MAX_ACTIVITY_TITLE_BYTES + 1),
            detail: None,
            status: TaskActivityStatus::Failed,
        };
        assert!(oversized.validate().is_err());
    }
}
