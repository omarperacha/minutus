//! Restore path identities from paired, persisted discovery calls and tool results.

use ipc_types::{Conversation, MessagePart, MessageRole, ToolName};

use super::{
    JsonValue, MAX_PATH_REFERENCES, PathReferenceLedger, ReferencedPath, ToolOutcome,
    ToolResultPart,
};
use crate::chat_tools::ChatToolWorkspace;

impl PathReferenceLedger {
    /// Existing tool receipts are the durable source of truth, including after restart.
    pub(crate) fn for_conversation(
        conversation: &Conversation,
        workspaces: &[ChatToolWorkspace],
    ) -> Self {
        let mut ledger = Self {
            scopes: workspaces
                .iter()
                .map(|workspace| {
                    (
                        workspace.alias.clone(),
                        serde_json::json!([conversation.id.0, workspace.grant_id.0]).to_string(),
                    )
                })
                .collect(),
            ..Self::default()
        };
        let mut requests = std::collections::BTreeMap::new();
        for message in &conversation.messages {
            if message.conversation_id != conversation.id {
                continue;
            }
            if message.role == MessageRole::User {
                requests.clear();
            }
            for part in &message.parts {
                match part {
                    MessagePart::ToolCall { request }
                        if message.role == MessageRole::Assistant
                            && matches!(
                                request.name,
                                ToolName::FilesystemFindPaths | ToolName::FilesystemCollectEvidence
                            ) =>
                    {
                        requests.insert(request.call_id.0.clone(), request);
                    }
                    MessagePart::ToolResult { result } if message.role == MessageRole::Tool => {
                        let Some(request) = requests.remove(&result.call_id.0) else {
                            continue;
                        };
                        let Some(JsonValue::String(workspace)) = request.arguments.get("workspace")
                        else {
                            continue;
                        };
                        if !ledger.scopes.contains_key(workspace) {
                            continue;
                        }
                        if let ToolOutcome::Completed { content } = &result.outcome {
                            for part in content {
                                if let ToolResultPart::Json { value } = part {
                                    ledger.restore_value(workspace, value);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        ledger
    }

    fn restore_value(&mut self, workspace: &str, value: &JsonValue) {
        match value {
            JsonValue::Object(fields) => {
                if let Some(JsonValue::Array(matches)) = fields.get("matches") {
                    for entry in matches {
                        if let JsonValue::Object(entry) = entry {
                            self.restore_pair(
                                workspace,
                                entry.get("relativePath"),
                                entry.get("pathRef"),
                            );
                        }
                    }
                }
                self.restore_pair(
                    workspace,
                    fields.get("selectedPath"),
                    fields.get("selectedPathRef"),
                );
                for child in fields.values() {
                    self.restore_value(workspace, child);
                }
            }
            JsonValue::Array(values) => {
                for child in values {
                    self.restore_value(workspace, child);
                }
            }
            _ => {}
        }
    }

    fn restore_pair(
        &mut self,
        workspace: &str,
        path: Option<&JsonValue>,
        reference: Option<&JsonValue>,
    ) {
        let (Some(JsonValue::String(path)), Some(JsonValue::String(reference))) = (path, reference)
        else {
            return;
        };
        let bound = ReferencedPath {
            workspace: workspace.to_owned(),
            relative_path: path.clone(),
        };
        // Older numeric IDs lacked grant binding. Restore only unambiguous paired
        // receipts; a collision never silently chooses a different file. New IDs
        // additionally require the original conversation and current stable grant.
        let legacy = reference
            .strip_prefix(super::PATH_REFERENCE_PREFIX)
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            });
        if (!legacy && *reference != self.identity_for(&bound))
            || self.ambiguous.contains(reference)
        {
            return;
        }
        if let Some(previous) = self.by_id.get(reference) {
            if previous != &bound {
                let previous = previous.clone();
                self.by_id.remove(reference);
                if self.by_path.get(&previous) == Some(reference) {
                    self.by_path.remove(&previous);
                }
                self.ambiguous.insert(reference.clone());
            }
            return;
        }
        if self.by_id.len() + self.ambiguous.len() >= MAX_PATH_REFERENCES {
            return;
        }
        self.by_id.insert(reference.clone(), bound.clone());
        self.by_path
            .entry(bound)
            .or_insert_with(|| reference.clone());
    }
}
