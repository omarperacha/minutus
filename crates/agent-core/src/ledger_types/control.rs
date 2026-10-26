use serde::{Deserialize, Serialize};

use super::{ExecutionLedgerError, FactOrigin, validate_id};
use crate::PermissionBinding;

/// Trusted permission lifecycle decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerPermissionDecision {
    /// The exact request is still waiting for a decision.
    Pending,
    /// Trusted UI approved the exact request.
    Approved,
    /// Trusted UI explicitly denied the request.
    Denied,
    /// The approval surface was dismissed without a decision.
    Dismissed,
    /// The decision window expired.
    Expired,
    /// The request was cancelled.
    Cancelled,
}

/// Persisted permission fact bound to one exact tool call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionFact {
    fact_id: String,
    permission_id: String,
    sequence: u64,
    origin: FactOrigin,
    binding: PermissionBinding,
    decision: LedgerPermissionDecision,
}

impl PermissionFact {
    pub(crate) fn new(
        fact_id: String,
        permission_id: String,
        sequence: u64,
        origin: FactOrigin,
        binding: PermissionBinding,
        decision: LedgerPermissionDecision,
    ) -> Self {
        Self {
            fact_id,
            permission_id,
            sequence,
            origin,
            binding,
            decision,
        }
    }
    /// Returns the unique append-only ledger event ID.
    #[must_use]
    pub fn fact_id(&self) -> &str {
        &self.fact_id
    }
    #[must_use]
    /// Returns the permission record ID.
    pub fn permission_id(&self) -> &str {
        &self.permission_id
    }
    #[must_use]
    /// Returns the exact request provenance.
    pub const fn origin(&self) -> &FactOrigin {
        &self.origin
    }
    #[must_use]
    /// Returns the trusted tool name.
    pub fn tool_name(&self) -> &str {
        self.binding.tool_name()
    }
    /// Returns the exact arguments, capability and resource binding.
    #[must_use]
    pub const fn binding(&self) -> &PermissionBinding {
        &self.binding
    }
    #[must_use]
    /// Returns the trusted decision.
    pub const fn decision(&self) -> LedgerPermissionDecision {
        self.decision
    }
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// Remaining runtime budgets owned by the host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBudgets {
    /// Remaining inference/tool rounds.
    pub rounds: u32,
    /// Remaining total tool calls.
    pub tool_calls: u32,
    /// Remaining wall-clock milliseconds.
    pub elapsed_ms: u64,
    /// Remaining tool execution milliseconds.
    pub tool_ms: u64,
    /// Remaining model context tokens.
    pub context_tokens: u64,
    /// Tokens reserved for a final response.
    pub final_response_tokens: u64,
    /// Remaining aggregate serialized tool-output bytes.
    pub tool_output_bytes: u64,
}

/// Required next host action or terminal response state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LedgerNextAction {
    /// Start or continue one inference round.
    Generate,
    /// Validate a newly parsed tool request.
    ValidateToolRequest,
    /// Wait for trusted permission mediation.
    AwaitPermission {
        /// Exact pending call ID.
        call_id: String,
    },
    /// Execute one exact validated tool request.
    ExecuteTool {
        /// Exact call ID.
        call_id: String,
    },
    /// Return one recorded terminal result to the model.
    ReturnToolResult {
        /// Exact call ID.
        call_id: String,
    },
    /// Run a bounded final-answer-only pass.
    PrepareFinalAnswer,
    /// No further runtime action is permitted.
    Terminal {
        /// Trusted terminal response state.
        state: LedgerTerminalState,
    },
}

impl LedgerNextAction {
    pub(crate) fn validate(&self) -> Result<(), ExecutionLedgerError> {
        match self {
            Self::AwaitPermission { call_id }
            | Self::ExecuteTool { call_id }
            | Self::ReturnToolResult { call_id } => validate_id("next-action call ID", call_id),
            Self::Generate
            | Self::ValidateToolRequest
            | Self::PrepareFinalAnswer
            | Self::Terminal { .. } => Ok(()),
        }
    }
}

/// Trusted terminal response states retained in the ledger.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerTerminalState {
    /// The task completed successfully, with or without a mutation.
    CompleteSuccess,
    /// The task ended without applying a requested change.
    NoChangeApplied,
    /// The run was cancelled.
    Cancelled,
    /// Some work completed but unresolved outcomes remain.
    PartialOutcome,
    /// A mutation succeeded but the final response could not complete.
    ChangeAppliedResponseIncomplete,
}
