use serde::{Deserialize, Serialize};

use crate::{AgentState, ToolTerminalStatus};

/// Host-owned activity categories derived from state transitions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentActivityKind {
    /// A model generation round.
    Generation,
    /// A validated tool request.
    ToolRequest,
    /// Trusted permission mediation.
    Permission,
    /// Tool broker execution.
    ToolExecution,
    /// A bounded terminal tool result.
    ToolResult,
    /// Final-answer preparation or publication.
    FinalAnswer,
    /// A terminal agent failure.
    Failure,
    /// Terminal cancellation.
    Cancellation,
}

/// Sanitized host-owned activity status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentActivityStatus {
    /// Work is queued behind a trusted decision.
    Pending,
    /// Work currently owns the run.
    InProgress,
    /// Work completed successfully.
    Completed,
    /// Work failed.
    Failed,
    /// Work was cancelled.
    Cancelled,
}

/// One frontend-safe activity item derived solely from trusted state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentActivity {
    /// State revision that produced this item.
    pub revision: u64,
    /// Trusted activity category.
    pub kind: AgentActivityKind,
    /// Trusted lifecycle status.
    pub status: AgentActivityStatus,
    /// Static host-authored label.
    pub title: String,
    /// Optional trusted registry name; arguments are never included.
    pub tool_name: Option<String>,
}

pub(crate) fn activity_from_state(revision: u64, state: &AgentState) -> AgentActivity {
    let (kind, status, title, tool_name) = match state {
        AgentState::Generating { .. } => (
            AgentActivityKind::Generation,
            AgentActivityStatus::InProgress,
            "Generating response",
            None,
        ),
        AgentState::ToolRequested { tool } => (
            AgentActivityKind::ToolRequest,
            AgentActivityStatus::Pending,
            "Tool requested",
            Some(tool.tool_name().to_owned()),
        ),
        AgentState::AwaitingPermission { tool } => (
            AgentActivityKind::Permission,
            AgentActivityStatus::Pending,
            "Awaiting permission",
            Some(tool.tool_name().to_owned()),
        ),
        AgentState::ExecutingTool { tool, .. } => (
            AgentActivityKind::ToolExecution,
            AgentActivityStatus::InProgress,
            "Executing tool",
            Some(tool.tool_name().to_owned()),
        ),
        AgentState::ReturningToolResult { tool, outcome } => (
            AgentActivityKind::ToolResult,
            match outcome {
                ToolTerminalStatus::Completed => AgentActivityStatus::Completed,
                ToolTerminalStatus::Failed => AgentActivityStatus::Failed,
                ToolTerminalStatus::Cancelled => AgentActivityStatus::Cancelled,
            },
            match outcome {
                ToolTerminalStatus::Completed => "Tool completed",
                ToolTerminalStatus::Failed => "Tool failed",
                ToolTerminalStatus::Cancelled => "Tool cancelled",
            },
            Some(tool.tool_name().to_owned()),
        ),
        AgentState::PreparingFinalAnswer => (
            AgentActivityKind::FinalAnswer,
            AgentActivityStatus::InProgress,
            "Preparing final answer",
            None,
        ),
        AgentState::Completed => (
            AgentActivityKind::FinalAnswer,
            AgentActivityStatus::Completed,
            "Task completed",
            None,
        ),
        AgentState::Failed { .. } => (
            AgentActivityKind::Failure,
            AgentActivityStatus::Failed,
            "Task failed",
            None,
        ),
        AgentState::Cancelled { .. } => (
            AgentActivityKind::Cancellation,
            AgentActivityStatus::Cancelled,
            "Task cancelled",
            None,
        ),
    };
    AgentActivity {
        revision,
        kind,
        status,
        title: title.to_owned(),
        tool_name,
    }
}
