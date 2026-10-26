//! Tool request and result schemas.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AssetId, IpcError, ToolCallId};

/// JSON-compatible value without an untyped TypeScript escape hatch.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(untagged)]
pub enum JsonValue {
    /// JSON null.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// Finite JSON number.
    Number(f64),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<Self>),
    /// JSON object with deterministic key ordering.
    Object(BTreeMap<String, Self>),
}

/// Closed set of tool names understood by the MVP contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ToolName {
    /// Perform a bounded local knowledge lookup.
    Lookup,
    /// Run a bounded local computation without project authority.
    #[serde(rename = "computation.run")]
    #[ts(rename = "computation.run")]
    ComputationRun,
    /// Run a bounded local computation over explicitly bound project files.
    #[serde(rename = "computation.run_project")]
    #[ts(rename = "computation.run_project")]
    ComputationRunProject,
    /// Retrieve one bounded public HTTPS resource after fresh approval.
    #[serde(rename = "web.retrieve")]
    #[ts(rename = "web.retrieve")]
    WebRetrieve,
    /// Discover bounded public-web result candidates after fresh approval.
    #[serde(rename = "web.search")]
    #[ts(rename = "web.search")]
    WebSearch,
    /// Human-approved x402 discovery and purchase workflow.
    #[serde(rename = "x402.request", alias = "x402.purchase_flowzed_test_resource")]
    #[ts(rename = "x402.request")]
    X402Request,
    /// List one granted workspace directory.
    #[serde(rename = "filesystem.list")]
    #[ts(rename = "filesystem.list")]
    FilesystemList,
    /// Find files or directories recursively by bounded filename filters.
    #[serde(rename = "filesystem.find_paths")]
    #[ts(rename = "filesystem.find_paths")]
    FilesystemFindPaths,
    /// Collect bounded discovered text and image evidence in one operation.
    #[serde(rename = "filesystem.collect_evidence")]
    #[ts(rename = "filesystem.collect_evidence")]
    FilesystemCollectEvidence,
    /// Return safe metadata for one granted workspace entry.
    #[serde(rename = "filesystem.stat")]
    #[ts(rename = "filesystem.stat")]
    FilesystemStat,
    /// Read one bounded UTF-8 file from a granted workspace.
    #[serde(rename = "filesystem.read_text")]
    #[ts(rename = "filesystem.read_text")]
    FilesystemReadText,
    /// Inspect one bounded static image from a granted workspace.
    #[serde(rename = "filesystem.inspect_image")]
    #[ts(rename = "filesystem.inspect_image")]
    FilesystemInspectImage,
    /// Run a bounded recursive literal search in a granted workspace.
    #[serde(rename = "filesystem.search")]
    #[ts(rename = "filesystem.search")]
    FilesystemSearch,
    /// Run a bounded aggregate line-count scan without returning file contents.
    #[serde(rename = "filesystem.scan")]
    #[ts(rename = "filesystem.scan")]
    FilesystemScan,
    /// Create exactly one directory beneath an existing writable parent.
    #[serde(rename = "filesystem.create_directory")]
    #[ts(rename = "filesystem.create_directory")]
    FilesystemCreateDirectory,
    /// Apply bounded digest-checked text replacements inside a writable project folder.
    #[serde(rename = "filesystem.write_text")]
    #[ts(rename = "filesystem.write_text")]
    FilesystemWriteText,
    /// Apply exact digest-checked substitutions without re-emitting the whole file.
    #[serde(rename = "filesystem.replace_text")]
    #[ts(rename = "filesystem.replace_text")]
    FilesystemReplaceText,
    /// Materialize one validated CSV or TSV through the filesystem broker.
    #[serde(rename = "filesystem.write_delimited_table")]
    #[ts(rename = "filesystem.write_delimited_table")]
    FilesystemWriteDelimitedTable,
    /// Materialize a host-validated closed artifact representation through the filesystem broker.
    ///
    /// Retained only for persisted semantic-execution-v1 conversations.
    #[serde(rename = "orchestration.transform_artifact")]
    #[ts(rename = "orchestration.transform_artifact")]
    OrchestrationTransformArtifact,
    /// Submit or amend a bounded host-validated orchestration plan.
    #[serde(rename = "orchestration.submit_plan")]
    #[ts(rename = "orchestration.submit_plan")]
    OrchestrationSubmitPlan,
    /// Submit or amend a bounded advisory execution forecast.
    #[serde(rename = "orchestration.submit_forecast")]
    #[ts(rename = "orchestration.submit_forecast")]
    OrchestrationSubmitForecast,
    /// Complete the active bounded orchestration stage.
    #[serde(rename = "orchestration.complete_stage")]
    #[ts(rename = "orchestration.complete_stage")]
    OrchestrationCompleteStage,
    /// Search and load available task-tool contracts without revising the plan.
    #[serde(rename = "orchestration.search_tools")]
    #[ts(rename = "orchestration.search_tools")]
    OrchestrationSearchTools,
    /// Read bounded historical context in the current conversation.
    #[serde(rename = "orchestration.recall")]
    #[ts(rename = "orchestration.recall")]
    OrchestrationRecall,
    /// Push a bounded child stage and suspend its parent.
    #[serde(rename = "orchestration.refine_stage")]
    #[ts(rename = "orchestration.refine_stage")]
    OrchestrationRefineStage,
    /// Host-owned, non-executing correction for a safely bounded unknown call.
    #[serde(rename = "orchestration.tool_correction")]
    #[ts(rename = "orchestration.tool_correction")]
    OrchestrationToolCorrection,
    /// Legacy bounded workspace read name retained for persisted conversations.
    ReadWorkspaceFile,
    /// Stage a reviewed change inside a granted workspace.
    ProposeWorkspaceChange,
    /// Fetch from an explicitly permitted external origin.
    FetchUrl,
    /// Construct, but never sign, a transaction proposal.
    ProposeTransaction,
}

/// Tool call emitted by the model adapter after schema parsing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolRequest {
    /// Stable identifier for this invocation.
    pub call_id: ToolCallId,
    /// Allow-listed tool name.
    pub name: ToolName,
    /// JSON-compatible arguments validated again by the selected tool.
    pub arguments: BTreeMap<String, JsonValue>,
}

/// One safe content part returned by a tool broker.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolResultPart {
    /// Bounded UTF-8 content.
    Text {
        /// Tool-produced text.
        text: String,
    },
    /// Structured JSON-compatible content.
    Json {
        /// Structured value.
        value: JsonValue,
    },
    /// Reference to a managed asset.
    Asset {
        /// Opaque asset identifier.
        asset_id: AssetId,
    },
}

/// Outcome of a tool invocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolOutcome {
    /// Tool execution completed.
    Completed {
        /// Ordered frontend-safe result content.
        content: Vec<ToolResultPart>,
    },
    /// Tool execution failed without exposing internal diagnostics.
    Failed {
        /// Sanitized failure.
        error: IpcError,
    },
}

/// Result associated with one tool call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResult {
    /// Tool invocation receiving this result.
    pub call_id: ToolCallId,
    /// Successful content or sanitized failure.
    pub outcome: ToolOutcome,
}
