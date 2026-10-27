//! Request-local reuse and coverage ledger for completed workspace operations.

use std::collections::{BTreeMap, BTreeSet};

use ipc_types::{AssetId, JsonValue, TaskActivityStatus, ToolOutcome, ToolResultPart};
use model_runtime::ParsedToolCall;

use super::{
    ChatToolExchange, OperationAdmissionRecord, execution::PreparedChatTool,
    find_paths_result_resolves_all_queries,
};

const MAX_ADMISSION_RECORDS: usize = 256;

#[derive(Clone)]
struct LedgerEntry {
    workspace_revision: u64,
    exchange: ChatToolExchange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathQueryCoverage {
    workspace: String,
    path: String,
    name_contains: String,
    kind: String,
    extensions: BTreeSet<String>,
    max_depth: u64,
    originating_call_id: String,
    workspace_revision: u64,
}

#[derive(Default)]
pub(crate) struct OperationLedger {
    workspace_revision: u64,
    useful_result_sequence: u64,
    x402_attempt_consumed: bool,
    semantic_stage: Option<String>,
    image_inspections: BTreeMap<(Option<String>, String), u8>,
    pending_image_inspections: BTreeMap<(Option<String>, String), AssetId>,
    exact: BTreeMap<String, LedgerEntry>,
    path_queries: Vec<PathQueryCoverage>,
    admission_records: Vec<OperationAdmissionRecord>,
}

impl OperationLedger {
    pub(crate) fn set_semantic_stage(&mut self, stage: Option<&str>) {
        self.semantic_stage = stage.map(str::to_owned);
    }

    pub(crate) fn current_stage_has_image_reinspection(
        &self,
        admitted_asset_ids: &[AssetId],
    ) -> bool {
        self.pending_image_inspections
            .iter()
            .any(|(key @ (stage, _), asset_id)| {
                stage == &self.semantic_stage
                    && self.image_inspections.get(key).copied().unwrap_or_default() >= 1
                    && admitted_asset_ids.contains(asset_id)
            })
    }

    pub(crate) fn pending_image_ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.pending_image_inspections.values().cloned()
    }

    /// Retrieval alone does not spend the presentation allowance. Only images
    /// confirmed in a provider round consume a pending inspection.
    pub(crate) fn record_image_presentations(&mut self, asset_ids: &[AssetId]) {
        self.pending_image_inspections.retain(|key, asset_id| {
            if asset_ids.contains(asset_id) {
                let count = self.image_inspections.entry(key.clone()).or_default();
                *count = count.saturating_add(1);
                false
            } else {
                true
            }
        });
    }

    /// Monotonic request-local checkpoint advanced only by a useful completed
    /// result or a deterministic failure worth retaining.
    pub(crate) const fn useful_result_sequence(&self) -> u64 {
        self.useful_result_sequence
    }

    /// Whether one valid x402 workflow already executed in this user request.
    pub(crate) const fn x402_attempt_consumed(&self) -> bool {
        self.x402_attempt_consumed
    }

    pub(crate) fn record_admissions(
        &mut self,
        records: impl IntoIterator<Item = OperationAdmissionRecord>,
    ) {
        for record in records {
            if self.admission_records.len() == MAX_ADMISSION_RECORDS {
                self.admission_records.remove(0);
            }
            self.admission_records.push(record);
        }
    }

    pub(super) fn has_computation_candidate(&self, target: &PreparedChatTool) -> bool {
        is_computation(&target.call.name)
            && self
                .exact
                .get(&equivalence_key(&target.call))
                .is_some_and(|entry| {
                    entry.workspace_revision == self.workspace_revision
                        && entry.exchange.computation_reuse_digest.is_some()
                        && matches!(entry.exchange.result.outcome, ToolOutcome::Completed { .. })
                })
    }

    pub(super) fn reusable_exchange(
        &self,
        target: &PreparedChatTool,
        current_computation_digest: Option<&str>,
    ) -> Option<ChatToolExchange> {
        // External observations are mutable by default. Their retained request
        // and response digests support explicit host revalidation, but never
        // authorize request-local replay or suppress a fresh approval/fetch.
        if matches!(
            target.call.name.as_str(),
            "web.retrieve" | "web.search" | "x402.request" | "wallet.request_transfer"
        ) {
            return None;
        }
        let key = equivalence_key(&target.call);
        let computation = is_computation(&target.call.name);
        if let Some(entry) = self
            .exact
            .get(&key)
            .filter(|entry| entry.workspace_revision == self.workspace_revision)
            .filter(|entry| {
                !computation
                    || entry.exchange.computation_reuse_digest.as_deref()
                        == current_computation_digest
            })
        {
            if target.call.name == "filesystem.inspect_image" {
                let presentations = self
                    .image_inspections
                    .get(&self.image_inspection_key(&key))
                    .copied()
                    .unwrap_or_default();
                if presentations < 2 {
                    return None;
                }
                return Some(reinspection_limit_notice(&entry.exchange, target));
            }
            return Some(match &entry.exchange.result.outcome {
                ToolOutcome::Failed { .. } if computation => return None,
                ToolOutcome::Failed { .. } => reuse_failure_notice(&entry.exchange, target),
                ToolOutcome::Completed { .. } => reuse_notice(
                    &entry.exchange,
                    target,
                    "Reused an equivalent completed operation from this request",
                ),
            });
        }
        if target.call.name == "filesystem.find_paths"
            && let Some(source) = self.covering_find_paths_entry(&target.call)
        {
            return Some(reuse_notice(
                &source.exchange_placeholder(target),
                target,
                "Reused complete broader filename-discovery evidence from this request",
            ));
        }
        None
    }

    pub(crate) fn record_results(
        &mut self,
        calls: &[ParsedToolCall],
        exchanges: &[Result<ChatToolExchange, ipc_types::IpcError>],
    ) {
        for (call, exchange) in calls.iter().zip(exchanges) {
            if call.name == "x402.request"
                && call
                    .arguments
                    .get("operation")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|operation| matches!(operation, "purchase" | "purchase_url"))
            {
                if !self.x402_attempt_consumed
                    && exchange.as_ref().is_ok_and(|exchange| {
                        exchange.activity.status != TaskActivityStatus::Partial
                            && match &exchange.result.outcome {
                                ToolOutcome::Completed { .. } => true,
                                ToolOutcome::Failed { error } => !error.retryable,
                            }
                    })
                {
                    self.x402_attempt_consumed = true;
                    self.useful_result_sequence = self.useful_result_sequence.saturating_add(1);
                }
                continue;
            }
            if matches!(call.name.as_str(), "web.retrieve" | "web.search") {
                continue;
            }
            let Ok(exchange) = exchange else {
                continue;
            };
            if exchange.activity.status == TaskActivityStatus::Reused {
                continue;
            }
            let completed = matches!(exchange.result.outcome, ToolOutcome::Completed { .. });
            let deterministic_read_failure = !is_mutation(&call.name)
                && !is_computation(&call.name)
                // Partial is a host scheduling/admission result, not a failed
                // operation. It must remain executable with identical input.
                && exchange.activity.status == TaskActivityStatus::Failed
                && matches!(
                    &exchange.result.outcome,
                    ToolOutcome::Failed { error }
                        if !error.retryable && error.code != ipc_types::IpcErrorCode::Cancelled
                );
            if !completed && !deterministic_read_failure {
                continue;
            }
            self.useful_result_sequence = self.useful_result_sequence.saturating_add(1);
            if completed && is_mutation(&call.name) {
                self.workspace_revision = self.workspace_revision.saturating_add(1);
                self.exact.clear();
                self.path_queries.clear();
                self.image_inspections.clear();
                self.pending_image_inspections.clear();
            }
            if completed
                && call.name == "filesystem.inspect_image"
                && let Some(asset_id) = &exchange.inspected_asset_id
            {
                let key = self.image_inspection_key(&equivalence_key(call));
                self.pending_image_inspections.insert(key, asset_id.clone());
            }
            if completed {
                self.record_find_paths_coverage(call, exchange);
            }
            if call.name == "filesystem.find_paths"
                && completed
                && !find_paths_result_resolves_all_queries(call, exchange)
            {
                continue;
            }
            self.exact.insert(
                equivalence_key(call),
                LedgerEntry {
                    workspace_revision: self.workspace_revision,
                    exchange: exchange.clone(),
                },
            );
        }
    }

    fn image_inspection_key(&self, operation_key: &str) -> (Option<String>, String) {
        (self.semantic_stage.clone(), operation_key.to_owned())
    }

    fn covering_find_paths_entry(&self, call: &ParsedToolCall) -> Option<&PathQueryCoverage> {
        let workspace = call
            .arguments
            .get("workspace")
            .and_then(serde_json::Value::as_str)?;
        let queries = call
            .arguments
            .get("queries")
            .and_then(serde_json::Value::as_array)?;
        let requested = queries
            .iter()
            .map(|query| normalized_requested_query(workspace, query))
            .collect::<Option<Vec<_>>>()?;
        if requested.is_empty() {
            return None;
        }
        self.path_queries.iter().rev().find(|candidate| {
            candidate.workspace_revision == self.workspace_revision
                && requested.iter().all(|query| {
                    self.path_queries.iter().any(|coverage| {
                        coverage.workspace_revision == self.workspace_revision
                            && coverage.originating_call_id == candidate.originating_call_id
                            && coverage.covers(query)
                    })
                })
        })
    }

    fn record_find_paths_coverage(&mut self, call: &ParsedToolCall, exchange: &ChatToolExchange) {
        if call.name != "filesystem.find_paths" {
            return;
        }
        let Some(workspace) = call
            .arguments
            .get("workspace")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let ToolOutcome::Completed { content } = &exchange.result.outcome else {
            return;
        };
        for part in content {
            let ToolResultPart::Json {
                value: JsonValue::Object(root),
            } = part
            else {
                continue;
            };
            let Some(JsonValue::Array(results)) = root.get("results") else {
                continue;
            };
            for result in results {
                let JsonValue::Object(result) = result else {
                    continue;
                };
                if !matches!(result.get("complete"), Some(JsonValue::Bool(true)))
                    || matches!(result.get("truncated"), Some(JsonValue::Bool(true)))
                {
                    continue;
                }
                let Some(JsonValue::Object(query)) = result.get("query") else {
                    continue;
                };
                if let Some(coverage) =
                    coverage_from_result(workspace, query, &call.call_id, self.workspace_revision)
                {
                    self.path_queries.push(coverage);
                }
            }
        }
    }
}

impl PathQueryCoverage {
    fn covers(&self, requested: &Self) -> bool {
        self.workspace == requested.workspace
            && self.path == requested.path
            && self.max_depth >= requested.max_depth
            && requested.name_contains.contains(&self.name_contains)
            && (self.kind == "any" || self.kind == requested.kind)
            && (self.extensions.is_empty()
                || (!requested.extensions.is_empty()
                    && requested.extensions.is_subset(&self.extensions)))
    }

    fn exchange_placeholder(&self, target: &PreparedChatTool) -> ChatToolExchange {
        let mut request = target.request.clone();
        request.call_id = ipc_types::ToolCallId(self.originating_call_id.clone());
        ChatToolExchange {
            request,
            result: ipc_types::ToolResult {
                call_id: target.request.call_id.clone(),
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Json {
                        value: JsonValue::Object(BTreeMap::from([
                            ("reused".to_owned(), JsonValue::Bool(true)),
                            (
                                "originatingCallId".to_owned(),
                                JsonValue::String(self.originating_call_id.clone()),
                            ),
                        ])),
                    }],
                },
            },
            activity: target.activity.clone(),
            workspace_grant_id: target
                .workspace
                .as_ref()
                .map(|workspace| workspace.grant_id.clone()),
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        }
    }
}

fn reuse_notice(
    source: &ChatToolExchange,
    target: &PreparedChatTool,
    detail: &str,
) -> ChatToolExchange {
    let mut exchange = source.clone();
    exchange.request = target.request.clone();
    exchange.result = ipc_types::ToolResult {
        call_id: target.request.call_id.clone(),
        outcome: ToolOutcome::Completed {
            content: vec![ToolResultPart::Json {
                value: JsonValue::Object(BTreeMap::from([
                    ("reused".to_owned(), JsonValue::Bool(true)),
                    (
                        "originatingCallId".to_owned(),
                        JsonValue::String(source.request.call_id.0.clone()),
                    ),
                    (
                        "message".to_owned(),
                        JsonValue::String(
                            "The earlier complete result remains authoritative; do not repeat this operation"
                                .to_owned(),
                        ),
                    ),
                ])),
            }],
        },
    };
    exchange.activity = target.activity.clone();
    exchange.activity.status = TaskActivityStatus::Reused;
    exchange.activity.detail = Some(detail.to_owned());
    exchange.undo_changes.clear();
    exchange.inspected_asset_id = None;
    exchange
}

fn reuse_failure_notice(source: &ChatToolExchange, target: &PreparedChatTool) -> ChatToolExchange {
    let mut exchange = source.clone();
    exchange.request = target.request.clone();
    exchange.result.call_id = target.request.call_id.clone();
    exchange.activity = target.activity.clone();
    exchange.activity.status = TaskActivityStatus::Reused;
    exchange.activity.detail = Some(
        "Reused an equivalent non-retryable failure from this request; correct the arguments before retrying"
            .to_owned(),
    );
    exchange.undo_changes.clear();
    exchange.inspected_asset_id = None;
    exchange
}

fn reinspection_limit_notice(
    source: &ChatToolExchange,
    target: &PreparedChatTool,
) -> ChatToolExchange {
    let mut exchange = reuse_notice(
        source,
        target,
        "The exact image was already presented and reinspected for this semantic stage",
    );
    exchange.result = ipc_types::ToolResult {
        call_id: target.request.call_id.clone(),
        outcome: ToolOutcome::Completed {
            content: vec![ToolResultPart::Json {
                value: JsonValue::Object(BTreeMap::from([
                    ("reinspectionLimited".to_owned(), JsonValue::Bool(true)),
                    (
                        "message".to_owned(),
                        JsonValue::String(
                            "The exact image was already presented and reinspected for this stage. Its pixels are not automatically replayed in later rounds. Use established extracted values if sufficient; otherwise identify the specific unresolved source detail. Complete the stage if doneWhen is satisfied, or revise the plan if the source must change."
                                .to_owned(),
                        ),
                    ),
                ])),
            }],
        },
    };
    exchange
}

fn equivalence_key(call: &ParsedToolCall) -> String {
    serde_json::to_string(&(&call.name, &call.arguments)).unwrap_or_default()
}

fn is_computation(name: &str) -> bool {
    matches!(name, "computation.run" | "computation.run_project")
}

fn is_mutation(name: &str) -> bool {
    matches!(
        name,
        "filesystem.create_directory"
            | "filesystem.write_text"
            | "filesystem.replace_text"
            | "filesystem.write_delimited_table"
            | "orchestration.transform_artifact"
    )
}

fn normalized_requested_query(
    workspace: &str,
    value: &serde_json::Value,
) -> Option<PathQueryCoverage> {
    let query = value.as_object()?;
    let name_contains = query.get("nameContains")?.as_str()?.to_ascii_lowercase();
    Some(PathQueryCoverage {
        workspace: workspace.to_owned(),
        path: query
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".")
            .to_owned(),
        name_contains,
        kind: query
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("any")
            .to_owned(),
        extensions: serde_extensions(query.get("extensions")),
        max_depth: query
            .get("maxDepth")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(8),
        originating_call_id: String::new(),
        workspace_revision: 0,
    })
}

fn coverage_from_result(
    workspace: &str,
    query: &BTreeMap<String, JsonValue>,
    call_id: &str,
    workspace_revision: u64,
) -> Option<PathQueryCoverage> {
    Some(PathQueryCoverage {
        workspace: workspace.to_owned(),
        path: json_string(query, "relativePath").unwrap_or(".").to_owned(),
        name_contains: json_string(query, "nameContains")?.to_ascii_lowercase(),
        kind: json_string(query, "kind").unwrap_or("any").to_owned(),
        extensions: ipc_extensions(query.get("extensions")),
        max_depth: match query.get("maxDepth") {
            Some(JsonValue::Number(value)) if value.is_finite() && *value >= 0.0 => {
                value.to_string().parse().unwrap_or(8)
            }
            _ => 8,
        },
        originating_call_id: call_id.to_owned(),
        workspace_revision,
    })
}

fn json_string<'a>(fields: &'a BTreeMap<String, JsonValue>, name: &str) -> Option<&'a str> {
    match fields.get(name) {
        Some(JsonValue::String(value)) => Some(value),
        _ => None,
    }
}

fn serde_extensions(value: Option<&serde_json::Value>) -> BTreeSet<String> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn ipc_extensions(value: Option<&JsonValue>) -> BTreeSet<String> {
    match value {
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                JsonValue::String(value) => Some(value.to_ascii_lowercase()),
                _ => None,
            })
            .collect(),
        _ => BTreeSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ipc_types::{
        ToolCallId, ToolName, ToolRequest, ToolResult, WorkspaceGrantId, WorkspacePermissionLevel,
    };

    use super::*;
    use crate::chat_tools::ChatToolWorkspace;

    fn coverage(name_contains: &str, complete_revision: u64) -> PathQueryCoverage {
        PathQueryCoverage {
            workspace: "primary".to_owned(),
            path: ".".to_owned(),
            name_contains: name_contains.to_owned(),
            kind: "any".to_owned(),
            extensions: BTreeSet::new(),
            max_depth: 8,
            originating_call_id: "original".to_owned(),
            workspace_revision: complete_revision,
        }
    }

    fn read_call(call_id: &str) -> ParsedToolCall {
        ParsedToolCall {
            call_id: call_id.to_owned(),
            recipient: "filesystem.read_text".to_owned(),
            name: "filesystem.read_text".to_owned(),
            arguments: BTreeMap::from([
                ("path".to_owned(), serde_json::json!("manifest.json")),
                ("workspace".to_owned(), serde_json::json!("primary")),
            ]),
        }
    }

    fn inspect_call(call_id: &str) -> ParsedToolCall {
        ParsedToolCall {
            call_id: call_id.to_owned(),
            recipient: "filesystem.inspect_image".to_owned(),
            name: "filesystem.inspect_image".to_owned(),
            arguments: BTreeMap::from([
                ("path".to_owned(), serde_json::json!("source.png")),
                ("workspace".to_owned(), serde_json::json!("primary")),
            ]),
        }
    }

    fn find_call(call_id: &str) -> ParsedToolCall {
        ParsedToolCall {
            call_id: call_id.to_owned(),
            recipient: "filesystem.find_paths".to_owned(),
            name: "filesystem.find_paths".to_owned(),
            arguments: BTreeMap::from([
                (
                    "queries".to_owned(),
                    serde_json::json!([{"nameContains":"input","extensions":["png"]}]),
                ),
                ("workspace".to_owned(), serde_json::json!("primary")),
            ]),
        }
    }

    fn prepared(call: ParsedToolCall, name: ToolName) -> PreparedChatTool {
        let call_id = ToolCallId(call.call_id.clone());
        PreparedChatTool {
            request: ToolRequest {
                call_id: call_id.clone(),
                name,
                arguments: BTreeMap::new(),
            },
            workspace: Some(ChatToolWorkspace {
                alias: "primary".to_owned(),
                display_name: "project".to_owned(),
                grant_id: WorkspaceGrantId("grant".to_owned()),
                permission_level: WorkspacePermissionLevel::ReadAndWrite,
            }),
            activity: crate::chat_runtime::tool_activity(&call_id.0, &call.name),
            call,
        }
    }

    fn completed_exchange(tool: &PreparedChatTool) -> ChatToolExchange {
        ChatToolExchange {
            request: tool.request.clone(),
            result: ToolResult {
                call_id: tool.request.call_id.clone(),
                outcome: ToolOutcome::Completed {
                    content: vec![ToolResultPart::Text {
                        text: "authoritative result".to_owned(),
                    }],
                },
            },
            activity: tool.activity.clone(),
            workspace_grant_id: tool
                .workspace
                .as_ref()
                .map(|workspace| workspace.grant_id.clone()),
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        }
    }

    fn failed_exchange(tool: &PreparedChatTool, retryable: bool) -> ChatToolExchange {
        ChatToolExchange {
            request: tool.request.clone(),
            result: ToolResult {
                call_id: tool.request.call_id.clone(),
                outcome: ToolOutcome::Failed {
                    error: ipc_types::IpcError {
                        code: ipc_types::IpcErrorCode::InvalidRequest,
                        message: "The workspace path is unavailable or unsafe".to_owned(),
                        retryable,
                    },
                },
            },
            activity: ipc_types::TaskActivity {
                status: TaskActivityStatus::Failed,
                ..tool.activity.clone()
            },
            workspace_grant_id: tool
                .workspace
                .as_ref()
                .map(|workspace| workspace.grant_id.clone()),
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        }
    }

    #[test]
    fn complete_broader_filename_query_covers_narrower_query() {
        assert!(coverage("cashflow", 0).covers(&coverage("monthly cashflow", 0)));
        assert!(!coverage("monthly cashflow", 0).covers(&coverage("cashflow", 0)));
    }

    #[test]
    fn narrower_kind_or_extension_cannot_cover_a_broader_request() {
        let mut prior = coverage("cashflow", 0);
        prior.kind = "file".to_owned();
        assert!(!prior.covers(&coverage("cashflow", 0)));

        prior.kind = "any".to_owned();
        prior.extensions.insert("png".to_owned());
        assert!(!prior.covers(&coverage("cashflow", 0)));
    }

    #[test]
    fn exact_completed_operations_are_reused_until_a_mutation_invalidates_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let original_call = read_call("original");
        let original = prepared(original_call.clone(), ToolName::FilesystemReadText);
        let original_exchange = completed_exchange(&original);
        let mut ledger = OperationLedger::default();
        ledger.record_results(&[original_call], &[Ok(original_exchange)]);

        let repeated = prepared(read_call("repeated"), ToolName::FilesystemReadText);
        let Some(reused) = ledger.reusable_exchange(&repeated, None) else {
            return Err(std::io::Error::other("equivalent operation was not reused").into());
        };
        assert_eq!(reused.activity.status, TaskActivityStatus::Reused);

        let write_call = ParsedToolCall {
            call_id: "write".to_owned(),
            recipient: "filesystem.write_text".to_owned(),
            name: "filesystem.write_text".to_owned(),
            arguments: BTreeMap::from([("workspace".to_owned(), serde_json::json!("primary"))]),
        };
        let write = prepared(write_call.clone(), ToolName::FilesystemWriteText);
        ledger.record_results(&[write_call], &[Ok(completed_exchange(&write))]);
        assert!(ledger.reusable_exchange(&repeated, None).is_none());
        Ok(())
    }

    #[test]
    fn image_reinspection_executes_once_per_stage_then_returns_a_bounded_notice()
    -> Result<(), ipc_types::IpcError> {
        let first_call = inspect_call("inspect-first");
        let first = prepared(first_call.clone(), ToolName::FilesystemInspectImage);
        let mut ledger = OperationLedger::default();
        ledger.set_semantic_stage(Some("read-source"));
        let mut first_exchange = completed_exchange(&first);
        first_exchange.inspected_asset_id = Some(ipc_types::AssetId("asset-inspected".to_owned()));
        ledger.record_results(&[first_call], &[Ok(first_exchange)]);
        ledger.record_image_presentations(&[ipc_types::AssetId("asset-inspected".to_owned())]);

        let repeated_call = inspect_call("inspect-repeat");
        let repeated = prepared(repeated_call.clone(), ToolName::FilesystemInspectImage);
        assert!(ledger.reusable_exchange(&repeated, None).is_none());
        let inspected_asset = ipc_types::AssetId("asset-reinspected".to_owned());
        let mut repeated_exchange = completed_exchange(&repeated);
        repeated_exchange.inspected_asset_id = Some(inspected_asset.clone());
        ledger.record_results(&[repeated_call], &[Ok(repeated_exchange)]);
        assert!(
            ledger.current_stage_has_image_reinspection(std::slice::from_ref(&inspected_asset))
        );
        assert!(
            !ledger.current_stage_has_image_reinspection(&[ipc_types::AssetId(
                "asset-other".to_owned()
            )])
        );
        // Preparation/connection failure has not consumed this reinspection.
        assert!(ledger.reusable_exchange(&repeated, None).is_none());
        ledger.record_image_presentations(&[ipc_types::AssetId("not-submitted".to_owned())]);
        assert!(ledger.reusable_exchange(&repeated, None).is_none());
        ledger.record_image_presentations(std::slice::from_ref(&inspected_asset));
        assert_eq!(ledger.pending_image_ids().count(), 0);

        let limited = prepared(
            inspect_call("inspect-limited"),
            ToolName::FilesystemInspectImage,
        );
        let notice = ledger
            .reusable_exchange(&limited, None)
            .ok_or_else(ipc_types::IpcError::internal)?;
        assert_eq!(notice.activity.status, TaskActivityStatus::Reused);
        assert!(matches!(
            notice.result.outcome,
            ToolOutcome::Completed { ref content }
                if content.iter().any(|part| matches!(part,
                    ToolResultPart::Json { value: JsonValue::Object(fields) }
                        if matches!(fields.get("reinspectionLimited"), Some(JsonValue::Bool(true)))))
        ));

        ledger.set_semantic_stage(Some("verify-source"));
        assert!(
            !ledger.current_stage_has_image_reinspection(std::slice::from_ref(&inspected_asset))
        );
        assert!(ledger.reusable_exchange(&limited, None).is_none());
        Ok(())
    }

    #[test]
    fn cancelled_image_inspection_does_not_consume_presentation_or_reinspection() {
        let cancelled_call = inspect_call("inspect-cancelled");
        let cancelled = prepared(cancelled_call.clone(), ToolName::FilesystemInspectImage);
        let mut exchange = failed_exchange(&cancelled, false);
        exchange.result.outcome = ToolOutcome::Failed {
            error: ipc_types::IpcError {
                code: ipc_types::IpcErrorCode::Cancelled,
                message: "The tool operation was cancelled".to_owned(),
                retryable: false,
            },
        };
        let mut ledger = OperationLedger::default();
        ledger.set_semantic_stage(Some("read-source"));
        ledger.record_results(&[cancelled_call], &[Ok(exchange)]);

        let retry = prepared(
            inspect_call("inspect-after-cancel"),
            ToolName::FilesystemInspectImage,
        );
        assert!(ledger.reusable_exchange(&retry, None).is_none());
    }

    #[test]
    fn incomplete_empty_discovery_is_not_reused_as_authoritative_progress() {
        let original_call = find_call("original-find");
        let original = prepared(original_call.clone(), ToolName::FilesystemFindPaths);
        let mut incomplete = completed_exchange(&original);
        incomplete.result.outcome = ToolOutcome::Completed {
            content: vec![ToolResultPart::Json {
                value: JsonValue::Object(BTreeMap::from([(
                    "results".to_owned(),
                    JsonValue::Array(vec![JsonValue::Object(BTreeMap::from([
                        ("complete".to_owned(), JsonValue::Bool(false)),
                        ("matches".to_owned(), JsonValue::Array(Vec::new())),
                    ]))]),
                )])),
            }],
        };
        let mut ledger = OperationLedger::default();
        ledger.record_results(&[original_call], &[Ok(incomplete)]);

        let repeated = prepared(find_call("repeat-find"), ToolName::FilesystemFindPaths);
        assert!(ledger.reusable_exchange(&repeated, None).is_none());
    }

    #[test]
    fn exact_non_retryable_read_failure_is_reused_but_transient_failure_is_not()
    -> Result<(), ipc_types::IpcError> {
        let original_call = read_call("original-failure");
        let original = prepared(original_call.clone(), ToolName::FilesystemReadText);
        let mut ledger = OperationLedger::default();
        ledger.record_results(&[original_call], &[Ok(failed_exchange(&original, false))]);

        let repeated = prepared(read_call("repeated-failure"), ToolName::FilesystemReadText);
        let reused = ledger
            .reusable_exchange(&repeated, None)
            .ok_or_else(ipc_types::IpcError::internal)?;
        assert_eq!(reused.activity.status, TaskActivityStatus::Reused);
        assert!(matches!(
            reused.result.outcome,
            ToolOutcome::Failed { ref error } if !error.retryable
        ));
        assert_eq!(reused.result.call_id.0, "repeated-failure");

        let transient_call = read_call("transient");
        let transient = prepared(transient_call.clone(), ToolName::FilesystemReadText);
        let mut transient_ledger = OperationLedger::default();
        transient_ledger
            .record_results(&[transient_call], &[Ok(failed_exchange(&transient, true))]);
        assert!(
            transient_ledger
                .reusable_exchange(&repeated, None)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn computation_reuse_requires_exact_live_digest_and_never_reuses_failure() {
        let call = |id: &str| ParsedToolCall {
            call_id: id.to_owned(),
            recipient: "computation.run".to_owned(),
            name: "computation.run".to_owned(),
            arguments: BTreeMap::from([
                (
                    "source".to_owned(),
                    serde_json::json!("export default () => 42"),
                ),
                (
                    "output".to_owned(),
                    serde_json::json!({"kind":"json","maxBytes":1024,"maxItems":8}),
                ),
            ]),
        };
        let original_call = call("compute-original");
        let original = prepared(original_call.clone(), ToolName::ComputationRun);
        let mut success = completed_exchange(&original);
        success.computation_reuse_digest = Some("exact-runtime-and-input-revision".to_owned());
        let mut ledger = OperationLedger::default();
        ledger.record_results(&[original_call], &[Ok(success)]);

        let repeated = prepared(call("compute-repeat"), ToolName::ComputationRun);
        assert!(
            ledger
                .reusable_exchange(&repeated, Some("exact-runtime-and-input-revision"))
                .is_some()
        );
        assert!(
            ledger
                .reusable_exchange(&repeated, Some("changed-runtime-or-input-revision"))
                .is_none()
        );

        let failed_call = call("compute-failed");
        let failed = prepared(failed_call.clone(), ToolName::ComputationRun);
        let mut failed_exchange = failed_exchange(&failed, false);
        failed_exchange.computation_reuse_digest = Some("failed-digest".to_owned());
        let mut failed_ledger = OperationLedger::default();
        failed_ledger.record_results(&[failed_call], &[Ok(failed_exchange)]);
        assert!(
            failed_ledger
                .reusable_exchange(&repeated, Some("failed-digest"))
                .is_none()
        );
    }

    #[test]
    fn first_executed_x402_result_consumes_the_request_local_attempt() {
        let call = ParsedToolCall {
            call_id: "x402-first".to_owned(),
            recipient: "x402.request".to_owned(),
            name: "x402.request".to_owned(),
            arguments: BTreeMap::from([
                ("operation".to_owned(), serde_json::json!("purchase")),
                ("reason".to_owned(), serde_json::json!("diagnostic")),
                ("request".to_owned(), serde_json::Value::Null),
                (
                    "url".to_owned(),
                    serde_json::json!("http://127.0.0.1:5001/resource"),
                ),
            ]),
        };
        let tool = prepared(call.clone(), ToolName::X402Request);
        let mut attempted = failed_exchange(&tool, false);
        attempted.result = ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed {
                error: ipc_types::IpcError::invalid_request("Payment workflow stopped"),
            },
        };
        let mut ledger = OperationLedger::default();

        ledger.record_results(std::slice::from_ref(&call), &[Ok(attempted)]);

        assert!(ledger.x402_attempt_consumed());
        assert_eq!(ledger.useful_result_sequence(), 1);

        let mut deferred = failed_exchange(&tool, true);
        deferred.activity.status = TaskActivityStatus::Partial;
        let mut deferred_ledger = OperationLedger::default();
        deferred_ledger.record_results(std::slice::from_ref(&call), &[Ok(deferred)]);
        assert!(!deferred_ledger.x402_attempt_consumed());
        assert_eq!(deferred_ledger.useful_result_sequence(), 0);

        let mut correctable = failed_exchange(&tool, false);
        let mut error = ipc_types::IpcError::invalid_request("Fix resourceRef");
        error.retryable = true;
        correctable.result = ToolResult {
            call_id: tool.request.call_id.clone(),
            outcome: ToolOutcome::Failed { error },
        };
        let mut correctable_ledger = OperationLedger::default();
        correctable_ledger.record_results(&[call], &[Ok(correctable)]);
        assert!(!correctable_ledger.x402_attempt_consumed());
    }

    #[test]
    fn x402_discovery_never_consumes_the_purchase_attempt() {
        let call = ParsedToolCall {
            call_id: "x402-discovery".to_owned(),
            recipient: "x402.request".to_owned(),
            name: "x402.request".to_owned(),
            arguments: BTreeMap::from([
                ("operation".to_owned(), serde_json::json!("discover")),
                ("reason".to_owned(), serde_json::json!("discover offer")),
                ("request".to_owned(), serde_json::Value::Null),
                (
                    "url".to_owned(),
                    serde_json::json!("http://127.0.0.1:5001/test/api/x402/offers"),
                ),
            ]),
        };
        let tool = prepared(call.clone(), ToolName::X402Request);
        let exchange = completed_exchange(&tool);
        let mut ledger = OperationLedger::default();

        ledger.record_results(&[call], &[Ok(exchange)]);

        assert!(!ledger.x402_attempt_consumed());
    }

    #[test]
    fn admission_coordinates_are_bounded_and_retain_no_arguments() {
        use crate::chat_tools::{
            OperationPermissionOutcome, OperationProgressContribution, OperationStepRelationship,
        };

        let mut ledger = OperationLedger::default();
        for index in 0..=MAX_ADMISSION_RECORDS {
            ledger.record_admissions([OperationAdmissionRecord {
                call_id: format!("call-{index}"),
                permission: OperationPermissionOutcome::Admitted,
                relationship: OperationStepRelationship::AdaptiveCurrent,
                progress: OperationProgressContribution::Refinement,
            }]);
        }

        assert_eq!(ledger.admission_records.len(), MAX_ADMISSION_RECORDS);
        assert_eq!(ledger.admission_records[0].call_id, "call-1");
        assert_eq!(
            ledger.admission_records.last().map(|record| (
                record.permission,
                record.relationship,
                record.progress,
            )),
            Some((
                OperationPermissionOutcome::Admitted,
                OperationStepRelationship::AdaptiveCurrent,
                OperationProgressContribution::Refinement,
            ))
        );
    }
}
