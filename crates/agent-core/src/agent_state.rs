use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    ApprovalRequirement, ProjectCapability, RiskLevel, ToolClassification, ValidatedToolRequest,
    ledger_types::validate_digest, permission::request_arguments_sha256,
};

const MAX_ID_BYTES: usize = 160;
const MAX_FAILURE_CODE_BYTES: usize = 80;
const MAX_FAILURE_MESSAGE_BYTES: usize = 1_024;

/// Stable names for every trusted agent runtime state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStateKind {
    /// The model is generating reasoning, content, or a tool request.
    Generating,
    /// A model tool request passed registry validation.
    ToolRequested,
    /// The exact tool request is waiting for trusted permission mediation.
    AwaitingPermission,
    /// The trusted broker is executing one tool request.
    ExecutingTool,
    /// A bounded terminal tool result is ready to return to the model.
    ReturningToolResult,
    /// Tool use is disabled while the final response is prepared.
    PreparingFinalAnswer,
    /// The run completed successfully.
    Completed,
    /// The run terminated with a sanitized failure.
    Failed,
    /// The run was cancelled by trusted host logic.
    Cancelled,
}

/// Trusted metadata that identifies one validated tool request without arguments.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolRequestIdentity {
    call_id: String,
    tool_name: String,
    classification: ToolClassification,
    capability: ProjectCapability,
    approval: ApprovalRequirement,
    risk: RiskLevel,
    arguments_sha256: String,
}

impl ToolRequestIdentity {
    pub(crate) fn from_validated(request: &ValidatedToolRequest) -> Self {
        let definition = request.definition();
        Self {
            call_id: request.call_id().to_owned(),
            tool_name: request.tool_name().to_owned(),
            classification: definition.classification(),
            capability: definition.capability(),
            approval: definition.approval(),
            risk: definition.risk(),
            arguments_sha256: request_arguments_sha256(request.arguments()),
        }
    }

    /// Returns the adapter-scoped invocation ID.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the trusted registry name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the tool effect classification.
    #[must_use]
    pub const fn classification(&self) -> ToolClassification {
        self.classification
    }

    /// Returns the durable capability required by the tool.
    #[must_use]
    pub const fn capability(&self) -> ProjectCapability {
        self.capability
    }

    /// Returns whether exact interactive approval is required.
    #[must_use]
    pub const fn approval(&self) -> ApprovalRequirement {
        self.approval
    }

    /// Returns the trusted risk level.
    #[must_use]
    pub const fn risk(&self) -> RiskLevel {
        self.risk
    }

    /// Returns the canonical digest of the exact validated arguments.
    #[must_use]
    pub fn arguments_sha256(&self) -> &str {
        &self.arguments_sha256
    }

    pub(crate) fn validate(&self) -> Result<(), AgentStateMachineError> {
        validate_id("tool call ID", &self.call_id)?;
        validate_tool_name(&self.tool_name)?;
        validate_digest(&self.arguments_sha256).map_err(|_| {
            AgentStateMachineError::InvalidSnapshot(
                "tool request argument digest is malformed".to_owned(),
            )
        })
    }
}

/// Sanitized terminal tool status retained without result content.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolTerminalStatus {
    /// The tool returned supported bounded content.
    Completed,
    /// The tool returned a sanitized failure.
    Failed,
    /// The tool was cancelled before completing.
    Cancelled,
}

/// Sanitized terminal agent failure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFailure {
    code: String,
    message: String,
}

impl AgentFailure {
    /// Creates a bounded failure suitable for persistence and UI projection.
    ///
    /// # Errors
    ///
    /// Returns an error when the code or message is malformed or oversized.
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, AgentStateMachineError> {
        let failure = Self {
            code: code.into(),
            message: message.into(),
        };
        failure.validate()?;
        Ok(failure)
    }

    /// Returns the stable host-owned failure code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the sanitized user-safe message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    fn validate(&self) -> Result<(), AgentStateMachineError> {
        if self.code.is_empty()
            || self.code.len() > MAX_FAILURE_CODE_BYTES
            || !self
                .code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || self.message.is_empty()
            || self.message.len() > MAX_FAILURE_MESSAGE_BYTES
            || self.message.chars().any(char::is_control)
        {
            return Err(AgentStateMachineError::InvalidSnapshot(
                "agent failure is malformed".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Trusted reasons an agent run can be cancelled.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunCancellationReason {
    /// The user explicitly stopped the run.
    UserRequested,
    /// A trusted runtime deadline expired.
    DeadlineExceeded,
    /// Required authority was revoked.
    AuthorityRevoked,
    /// The host application is shutting down.
    HostShutdown,
}

/// Complete trusted state for one agent run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentState {
    /// One model generation owns the run.
    Generating {
        /// Host-generated operation ID for this inference round.
        generation_id: String,
    },
    /// One validated tool request owns the run.
    ToolRequested {
        /// Trusted request identity.
        tool: ToolRequestIdentity,
    },
    /// One exact tool request is waiting for permission.
    AwaitingPermission {
        /// Trusted request identity.
        tool: ToolRequestIdentity,
    },
    /// One tool broker invocation owns the run.
    ExecutingTool {
        /// Trusted request identity.
        tool: ToolRequestIdentity,
        /// Whether the broker supports prompt cancellation.
        cancellable: bool,
    },
    /// One terminal tool result is ready for the next inference round.
    ReturningToolResult {
        /// Trusted request identity.
        tool: ToolRequestIdentity,
        /// Sanitized result status; content remains in the result envelope or ledger.
        outcome: ToolTerminalStatus,
    },
    /// The host is preparing the model's final response for publication.
    PreparingFinalAnswer,
    /// The run completed successfully.
    Completed,
    /// The run ended with a sanitized failure.
    Failed {
        /// Trusted failure details.
        failure: AgentFailure,
    },
    /// The run ended by cancellation.
    Cancelled {
        /// Trusted cancellation reason.
        reason: RunCancellationReason,
    },
}

impl AgentState {
    /// Returns the stable state name.
    #[must_use]
    pub const fn kind(&self) -> AgentStateKind {
        match self {
            Self::Generating { .. } => AgentStateKind::Generating,
            Self::ToolRequested { .. } => AgentStateKind::ToolRequested,
            Self::AwaitingPermission { .. } => AgentStateKind::AwaitingPermission,
            Self::ExecutingTool { .. } => AgentStateKind::ExecutingTool,
            Self::ReturningToolResult { .. } => AgentStateKind::ReturningToolResult,
            Self::PreparingFinalAnswer => AgentStateKind::PreparingFinalAnswer,
            Self::Completed => AgentStateKind::Completed,
            Self::Failed { .. } => AgentStateKind::Failed,
            Self::Cancelled { .. } => AgentStateKind::Cancelled,
        }
    }

    /// Returns whether no further transition is permitted.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            Self::kind(self),
            AgentStateKind::Completed | AgentStateKind::Failed | AgentStateKind::Cancelled
        )
    }

    pub(crate) fn validate(&self) -> Result<(), AgentStateMachineError> {
        match self {
            Self::Generating { generation_id } => validate_id("generation ID", generation_id),
            Self::ToolRequested { tool }
            | Self::AwaitingPermission { tool }
            | Self::ExecutingTool { tool, .. }
            | Self::ReturningToolResult { tool, .. } => tool.validate(),
            Self::Failed { failure } => failure.validate(),
            Self::PreparingFinalAnswer | Self::Completed | Self::Cancelled { .. } => Ok(()),
        }
    }
}

/// Token proving which exact revision and operation a transition expects to own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionToken {
    pub(crate) run_id: String,
    pub(crate) revision: u64,
    pub(crate) owner: TransitionOwner,
}

impl TransitionToken {
    /// Returns the monotonically increasing state revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TransitionOwner {
    Generation(String),
    Tool(String),
    FinalAnswer,
    Terminal,
}

impl TransitionOwner {
    pub(crate) fn from_state(state: &AgentState) -> Self {
        match state {
            AgentState::Generating { generation_id } => Self::Generation(generation_id.clone()),
            AgentState::ToolRequested { tool }
            | AgentState::AwaitingPermission { tool }
            | AgentState::ExecutingTool { tool, .. }
            | AgentState::ReturningToolResult { tool, .. } => Self::Tool(tool.call_id.clone()),
            AgentState::PreparingFinalAnswer => Self::FinalAnswer,
            AgentState::Completed | AgentState::Failed { .. } | AgentState::Cancelled { .. } => {
                Self::Terminal
            }
        }
    }
}

/// Errors raised when a transition would weaken trusted runtime ordering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentStateMachineError {
    /// A caller used a token from an older revision or different operation.
    StaleTransition,
    /// The requested state edge is not part of the closed transition graph.
    InvalidTransition {
        /// Current trusted state.
        from: AgentStateKind,
        /// Rejected destination state.
        to: AgentStateKind,
    },
    /// A terminal run received another transition request.
    TerminalState,
    /// Cancellation is unavailable in the current state.
    CancellationUnavailable,
    /// The executing tool declared that it cannot be cancelled safely.
    ToolNotCancellable,
    /// A tool result did not match the exact executing request.
    MismatchedToolResult,
    /// An authorization receipt did not match the exact pending request.
    MismatchedAuthorization,
    /// The bounded transition history is full.
    TransitionLimitExceeded,
    /// A persisted snapshot is malformed or internally inconsistent.
    InvalidSnapshot(String),
}

impl fmt::Display for AgentStateMachineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleTransition => formatter.write_str("agent transition token is stale"),
            Self::InvalidTransition { from, to } => {
                write!(
                    formatter,
                    "invalid agent transition from {from:?} to {to:?}"
                )
            }
            Self::TerminalState => formatter.write_str("agent run is already terminal"),
            Self::CancellationUnavailable => {
                formatter.write_str("agent run cannot be cancelled in its current state")
            }
            Self::ToolNotCancellable => formatter.write_str("executing tool is not cancellable"),
            Self::MismatchedToolResult => {
                formatter.write_str("tool result does not match the executing request")
            }
            Self::MismatchedAuthorization => {
                formatter.write_str("authorization does not match the pending request")
            }
            Self::TransitionLimitExceeded => {
                formatter.write_str("agent transition history limit was reached")
            }
            Self::InvalidSnapshot(message) => {
                write!(formatter, "invalid agent snapshot: {message}")
            }
        }
    }
}

impl std::error::Error for AgentStateMachineError {}

pub(crate) fn validate_id(label: &str, value: &str) -> Result<(), AgentStateMachineError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(AgentStateMachineError::InvalidSnapshot(format!(
            "{label} is malformed"
        )));
    }
    Ok(())
}

fn validate_tool_name(value: &str) -> Result<(), AgentStateMachineError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES {
        return Err(AgentStateMachineError::InvalidSnapshot(
            "tool name is malformed".to_owned(),
        ));
    }
    for segment in value.split('.') {
        let mut bytes = segment.bytes();
        if bytes.next().is_none_or(|byte| !byte.is_ascii_lowercase())
            || bytes
                .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        {
            return Err(AgentStateMachineError::InvalidSnapshot(
                "tool name is malformed".to_owned(),
            ));
        }
    }
    Ok(())
}
