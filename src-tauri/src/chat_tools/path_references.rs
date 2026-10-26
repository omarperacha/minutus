//! Conversation-scoped identities for exact paths returned by trusted discovery.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

mod history;
#[cfg(test)]
mod tests;

use ipc_types::{IpcError, IpcErrorCode, JsonValue, ToolOutcome, ToolResultPart};
use model_runtime::ParsedToolCall;

use super::ChatToolExchange;

const PATH_REFERENCE_PREFIX: &str = "ref:path-";
pub(super) const MAX_PATH_REFERENCES: usize = 256;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReferencedPath {
    workspace: String,
    relative_path: String,
}

/// Bounded identities restored from persisted, paired discovery receipts.
#[derive(Clone, Default)]
pub(crate) struct PathReferenceLedger {
    by_id: BTreeMap<String, ReferencedPath>,
    by_path: BTreeMap<ReferencedPath, String>,
    scopes: BTreeMap<String, String>,
    ambiguous: BTreeSet<String>,
}

impl PathReferenceLedger {
    /// Resolves identity only; ordinary validation and current grants still govern access.
    pub(crate) fn resolve_calls(
        &self,
        calls: &mut [ParsedToolCall],
    ) -> Result<(), (usize, IpcError)> {
        for (index, call) in calls.iter_mut().enumerate() {
            let workspace = call
                .arguments
                .get("workspace")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if accepts_exact_path_reference(&call.name)
                && let Some(path) = call.arguments.get_mut("path")
            {
                self.resolve_path(path, &workspace)
                    .map_err(|error| (index, error))?;
            }
            if call.name == "filesystem.collect_evidence"
                && let Some(serde_json::Value::Array(items)) = call.arguments.get_mut("items")
            {
                for item in items {
                    if let Some(path) = item.get_mut("path") {
                        self.resolve_path(path, &workspace)
                            .map_err(|error| (index, error))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_path(&self, value: &mut serde_json::Value, workspace: &str) -> Result<(), IpcError> {
        let Some(path) = value
            .as_str()
            .filter(|path| path.starts_with(PATH_REFERENCE_PREFIX))
        else {
            return Ok(());
        };
        let referenced = self.by_id.get(path).ok_or_else(|| IpcError {
            code: IpcErrorCode::InvalidRequest,
            message: format!(
                "Unknown or ambiguous path reference `{path}`. Rediscover the path in the current workspace and copy its returned pathRef verbatim; do not guess or reconstruct the filename"
            ),
            retryable: true,
        })?;
        if workspace != referenced.workspace {
            return Err(IpcError::scope_correction(format!(
                "Path reference `{path}` belongs to workspace `{}` and cannot be used with another workspace",
                referenced.workspace
            )));
        }
        *value = serde_json::Value::String(referenced.relative_path.clone());
        Ok(())
    }

    /// Adds stable references beside exact paths in one successful discovery result.
    pub(crate) fn annotate_exchange(&mut self, exchange: &mut ChatToolExchange) {
        if !matches!(
            exchange.request.name,
            ipc_types::ToolName::FilesystemFindPaths
                | ipc_types::ToolName::FilesystemCollectEvidence
        ) {
            return;
        }
        let Some(JsonValue::String(workspace)) = exchange.request.arguments.get("workspace") else {
            return;
        };
        let workspace = workspace.clone();
        let ToolOutcome::Completed { content } = &mut exchange.result.outcome else {
            return;
        };
        for part in content {
            let ToolResultPart::Json { value } = part else {
                continue;
            };
            self.annotate_value(&workspace, value);
        }
    }

    fn annotate_value(&mut self, workspace: &str, value: &mut JsonValue) {
        match value {
            JsonValue::Object(fields) => {
                if let Some(JsonValue::Array(matches)) = fields.get_mut("matches") {
                    for entry in matches {
                        let JsonValue::Object(entry) = entry else {
                            continue;
                        };
                        let Some(JsonValue::String(path)) = entry.get("relativePath") else {
                            continue;
                        };
                        if let Some(reference) = self.reference_for(workspace, path) {
                            entry.insert("pathRef".to_owned(), JsonValue::String(reference));
                        }
                    }
                }
                if let Some(JsonValue::String(path)) = fields.get("selectedPath")
                    && let Some(reference) = self.reference_for(workspace, path)
                {
                    fields.insert("selectedPathRef".to_owned(), JsonValue::String(reference));
                }
                for child in fields.values_mut() {
                    self.annotate_value(workspace, child);
                }
            }
            JsonValue::Array(values) => {
                for child in values {
                    self.annotate_value(workspace, child);
                }
            }
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
        }
    }

    fn reference_for(&mut self, workspace: &str, relative_path: &str) -> Option<String> {
        let path = ReferencedPath {
            workspace: workspace.to_owned(),
            relative_path: relative_path.to_owned(),
        };
        if let Some(reference) = self.by_path.get(&path)
            && reference.contains("v2-")
        {
            return Some(reference.clone());
        }
        if self.by_id.len() + self.ambiguous.len() >= MAX_PATH_REFERENCES {
            return None;
        }
        let reference = self.identity_for(&path);
        self.by_id.insert(reference.clone(), path.clone());
        self.by_path.insert(path, reference.clone());
        Some(reference)
    }
    fn identity_for(&self, path: &ReferencedPath) -> String {
        // Length-delimited serialisation prevents separator ambiguities. The scope
        // binds the conversation and stable grant, independently of permission level.
        let scope = self
            .scopes
            .get(&path.workspace)
            .map(String::as_str)
            .unwrap_or_default();
        let encoded = serde_json::json!([scope, path.workspace, path.relative_path]).to_string();
        format!(
            "{PATH_REFERENCE_PREFIX}v2-{:x}",
            Sha256::digest(encoded.as_bytes())
        )
    }
}

fn accepts_exact_path_reference(operation: &str) -> bool {
    matches!(
        operation,
        "filesystem.list"
            | "filesystem.stat"
            | "filesystem.read_text"
            | "filesystem.inspect_image"
            | "filesystem.search"
    )
}
