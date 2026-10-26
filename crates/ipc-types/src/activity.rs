//! Frontend-safe task activity timeline records.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Closed set of activity categories rendered by the chat timeline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TaskActivityKind {
    /// Model-authored reasoning kept separate from the final answer.
    ModelReasoning,
    /// Host-observed model pass with content-free phase timing.
    ModelPass,
    /// Sandboxed computation with no project data authority.
    PureComputation,
    /// Sandboxed computation over explicitly bound project files.
    ProjectComputation,
    /// Bounded read from a granted workspace file.
    FileRead,
    /// Direct bounded write inside a read-and-write project folder.
    FileWrite,
    /// Local lookup or explicitly permitted external search.
    Search,
    /// Staged workspace change that has not been applied.
    ProposedWrite,
    /// Human permission outcome for an exact capability request.
    PermissionDecision,
    /// Construction and trusted decoding of a transaction candidate.
    TransactionPreparation,
    /// Lifecycle update for a reviewed transaction.
    TransactionStatus,
}

/// Presentation-safe lifecycle state for one activity record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TaskActivityStatus {
    /// Work is waiting to begin or awaiting a decision.
    Pending,
    /// Work is currently active.
    InProgress,
    /// Work completed successfully.
    Completed,
    /// A still-valid equivalent host result was reused without another effect.
    Reused,
    /// A bounded compact representation replaced a larger completed result.
    Compacted,
    /// Some scheduled work completed while another bounded operation did not.
    Partial,
    /// Work failed without exposing internal diagnostics.
    Failed,
    /// Work stopped before completion at the user's request.
    Cancelled,
    /// The exact permission or transaction request was approved.
    Approved,
    /// The exact permission or transaction request was denied or dismissed.
    Denied,
}

/// One updateable, frontend-safe activity timeline record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskActivity {
    /// Stable identifier used to update the same timeline row.
    pub id: String,
    /// Closed activity category.
    pub kind: TaskActivityKind,
    /// Trusted concise label safe for ordinary chat UI.
    pub title: String,
    /// Optional bounded detail without raw payloads or secrets.
    pub detail: Option<String>,
    /// Current lifecycle state.
    pub status: TaskActivityStatus,
}
