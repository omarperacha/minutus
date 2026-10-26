use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ExecutionBudgets, LedgerTerminalState, ObservationFact, PermissionBinding, ProjectCapability,
    ToolClassification, ValidatedToolRequest,
};

/// Host-configured limits not represented directly by remaining budgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopLimits {
    /// Maximum consecutive rounds that may execute a tool.
    pub consecutive_tool_rounds: u32,
    /// Maximum failures permitted for one equivalent call.
    pub repeated_failures: u32,
}

impl LoopLimits {
    /// Creates bounded loop limits.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or unreasonable policy values.
    pub fn new(
        consecutive_tool_rounds: u32,
        repeated_failures: u32,
    ) -> Result<Self, LoopControlError> {
        if consecutive_tool_rounds == 0
            || consecutive_tool_rounds > 64
            || repeated_failures == 0
            || repeated_failures > 16
        {
            return Err(LoopControlError::InvalidUsage);
        }
        Ok(Self {
            consecutive_tool_rounds,
            repeated_failures,
        })
    }
}

/// Remaining budgets supplied to one inference round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoundAllowance {
    /// Exact remaining host-owned budgets after reserving this round.
    pub remaining: ExecutionBudgets,
    /// Whether tools are permitted in this round.
    pub tools_enabled: bool,
}

/// Trusted generation resource usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GenerationUsage {
    /// Generated tokens charged against the context/generation budget.
    pub generated_tokens: u64,
    /// Wall-clock time charged in milliseconds.
    pub elapsed_ms: u64,
}

/// Trusted tool execution resource usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolExecutionUsage {
    /// Broker-measured tool execution time in milliseconds.
    pub elapsed_ms: u64,
}

/// Whether a proposed call is new or retries one exact failed call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolCallIntent {
    /// A new logical tool operation.
    New,
    /// Retry the exact scope and arguments of one earlier failed call.
    Retry {
        /// Exact prior failed call ID.
        failed_call_id: String,
    },
}

/// Why the controller entered its bounded final-answer-only phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FinalAnswerReason {
    /// A round, call, time, token, or output budget was exhausted.
    BudgetExhausted,
    /// A successful mutation should now be explained without further tools.
    MutationApplied,
    /// Equivalent failures reached the retry ceiling.
    RepeatedFailure,
}

/// Trusted disposition for a proposed tool call.
#[derive(Debug, Eq, PartialEq)]
pub enum ToolCallPlan<'a> {
    /// The exact call may proceed to permission/capability enforcement.
    Execute,
    /// Return a still-valid authoritative observation instead of executing.
    ReuseObservation(&'a ObservationFact),
}

/// Next bounded loop action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopDirective {
    /// Start one inference round with these budgets.
    Generate(RoundAllowance),
    /// Run a final-answer-only recovery or completion pass.
    PrepareFinalAnswer(FinalAnswerReason),
    /// The run is already terminal.
    Terminal(LedgerTerminalState),
}

/// Fail-closed loop policy errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoopControlError {
    /// Usage or limits were malformed or exceeded structural bounds.
    InvalidUsage,
    /// The tool call is equivalent to completed work and must not execute.
    DuplicateCall,
    /// A failed call can only be retried through an explicit retry intent.
    RetryRequired,
    /// Retry changed arguments, project, capability, permission, or resource scope.
    RetryScopeMismatch,
    /// The referenced failure was not transient.
    NonTransientFailure,
    /// The call/result pair does not match active trusted state.
    ResultMismatch,
    /// Tools are disabled in the final-answer-only phase.
    ToolsDisabled,
    /// The run has been cancelled or completed.
    Terminal,
}

impl fmt::Display for LoopControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for LoopControlError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EquivalentCall {
    pub(super) tool_name: String,
    pub(super) arguments_sha256: String,
    pub(super) capability: ProjectCapability,
    pub(super) binding: PermissionBinding,
}

impl EquivalentCall {
    pub(super) fn new(request: &ValidatedToolRequest, binding: &PermissionBinding) -> Self {
        Self {
            tool_name: request.tool_name().to_owned(),
            arguments_sha256: binding.arguments_sha256().to_owned(),
            capability: request.definition().capability(),
            binding: binding.clone(),
        }
    }

    pub(super) fn equivalent(&self, other: &Self) -> bool {
        self.tool_name == other.tool_name
            && self.arguments_sha256 == other.arguments_sha256
            && self.capability == other.capability
            && self.binding.resource_scope() == other.binding.resource_scope()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ActiveCall {
    pub(super) key: EquivalentCall,
    pub(super) classification: ToolClassification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FailedCall {
    pub(super) key: EquivalentCall,
    pub(super) transient: bool,
    pub(super) count: u32,
}

/// Persistable host-owned loop policy state; remaining budgets live in the ledger.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentLoopSnapshot {
    pub(super) version: u16,
    pub(super) limits: LoopLimitsSnapshot,
    pub(super) consecutive_tool_rounds: u32,
    pub(super) active: BTreeMap<String, ActiveCall>,
    pub(super) successful: Vec<EquivalentCall>,
    pub(super) failures: BTreeMap<String, FailedCall>,
    pub(super) final_answer_reason: Option<FinalAnswerReason>,
    pub(super) terminal: Option<LedgerTerminalState>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LoopLimitsSnapshot {
    pub(super) consecutive_tool_rounds: u32,
    pub(super) repeated_failures: u32,
}
