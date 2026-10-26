use serde::{Deserialize, Serialize};

use crate::{
    AgentActivity, AgentFailure, AgentState, AgentStateKind, AgentStateMachineError,
    ApprovalRequirement, ExecutionAuthorization, RunCancellationReason, ToolOutcome,
    ToolRequestIdentity, ToolResultEnvelope, ToolTerminalStatus, TransitionToken,
    ValidatedToolRequest,
    activity::activity_from_state,
    agent_state::{TransitionOwner, validate_id},
};

const SNAPSHOT_VERSION: u16 = 2;
const MAX_TRANSITIONS: usize = 256;

/// One immutable trusted state transition retained for recovery and UI projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateTransitionRecord {
    /// Monotonically increasing revision after this transition.
    pub revision: u64,
    /// Previous state, absent only for the initial generating state.
    pub from: Option<AgentStateKind>,
    /// Complete trusted state after the transition.
    pub state: AgentState,
}

/// Persistable trusted state for one agent run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStateSnapshot {
    version: u16,
    run_id: String,
    revision: u64,
    state: AgentState,
    transitions: Vec<StateTransitionRecord>,
}

impl AgentStateSnapshot {
    /// Returns the stable run ID.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the current revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the current trusted state.
    #[must_use]
    pub const fn state(&self) -> &AgentState {
        &self.state
    }
}

/// Safe action required to continue a nonterminal run after application restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    /// Restart an inference round that had not produced a tool or final answer.
    RestartGeneration,
    /// Revalidate the retained exact tool request before doing anything else.
    RevalidateToolRequest,
    /// Reissue trusted permission UI for the same exact request.
    ReissuePermission,
    /// Load the already-recorded terminal result and return it to the model.
    ReturnRecordedToolResult,
    /// Resume host-owned final-answer preparation.
    ResumeFinalAnswer,
}

/// Unsafe interruption that must not be replayed automatically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsafeInterruption {
    /// A non-cancellable or effectful tool may have produced an outcome before restart.
    ToolOutcomeUnknown,
}

/// Restart classification derived from the current trusted state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartDisposition {
    /// The run is terminal and needs no recovery.
    Terminal,
    /// The run has one bounded safe recovery action.
    Recoverable(RecoveryAction),
    /// Automatic replay could duplicate or misrepresent an effect.
    Unsafe(UnsafeInterruption),
}

/// Trusted work item that the host must ask to stop after accepting cancellation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancellationTarget {
    /// Stop one active inference generation.
    Generation(String),
    /// Withdraw one pending permission request.
    Permission(String),
    /// Stop one active cancellable tool broker call.
    Tool(String),
}

/// Receipt proving cancellation changed the state before side-effect cleanup runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancellationReceipt {
    /// Fresh terminal transition token.
    pub token: TransitionToken,
    /// Exact outstanding work the host must stop or withdraw.
    pub target: CancellationTarget,
}

/// Trusted, revision-checked state machine for one agent run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentStateMachine {
    run_id: String,
    revision: u64,
    state: AgentState,
    transitions: Vec<StateTransitionRecord>,
}

impl AgentStateMachine {
    /// Starts one run with exactly one active generation owner.
    ///
    /// # Errors
    ///
    /// Returns an error when either host-generated ID is malformed.
    pub fn start(
        run_id: impl Into<String>,
        generation_id: impl Into<String>,
    ) -> Result<Self, AgentStateMachineError> {
        let run_id = run_id.into();
        let generation_id = generation_id.into();
        validate_id("run ID", &run_id)?;
        validate_id("generation ID", &generation_id)?;
        let state = AgentState::Generating { generation_id };
        Ok(Self {
            run_id,
            revision: 0,
            transitions: vec![StateTransitionRecord {
                revision: 0,
                from: None,
                state: state.clone(),
            }],
            state,
        })
    }

    /// Restores a snapshot only after validating its complete transition chain.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, malformed state, gaps,
    /// invalid edges, or a current state inconsistent with its history.
    pub fn restore(snapshot: AgentStateSnapshot) -> Result<Self, AgentStateMachineError> {
        validate_snapshot(&snapshot)?;
        Ok(Self {
            run_id: snapshot.run_id,
            revision: snapshot.revision,
            state: snapshot.state,
            transitions: snapshot.transitions,
        })
    }

    /// Returns a persistable copy of trusted runtime state.
    #[must_use]
    pub fn snapshot(&self) -> AgentStateSnapshot {
        AgentStateSnapshot {
            version: SNAPSHOT_VERSION,
            run_id: self.run_id.clone(),
            revision: self.revision,
            state: self.state.clone(),
            transitions: self.transitions.clone(),
        }
    }

    /// Returns the current trusted state.
    #[must_use]
    pub const fn state(&self) -> &AgentState {
        &self.state
    }

    /// Returns the only token capable of owning the next transition.
    #[must_use]
    pub fn transition_token(&self) -> TransitionToken {
        TransitionToken {
            run_id: self.run_id.clone(),
            revision: self.revision,
            owner: TransitionOwner::from_state(&self.state),
        }
    }

    /// Returns a frontend-safe timeline derived from trusted transition states.
    #[must_use]
    pub fn activity_timeline(&self) -> Vec<AgentActivity> {
        self.transitions
            .iter()
            .map(|transition| activity_from_state(transition.revision, &transition.state))
            .collect()
    }

    /// Classifies restart behavior without replaying work.
    #[must_use]
    pub const fn restart_disposition(&self) -> RestartDisposition {
        match &self.state {
            AgentState::Generating { .. } => {
                RestartDisposition::Recoverable(RecoveryAction::RestartGeneration)
            }
            AgentState::ToolRequested { .. } => {
                RestartDisposition::Recoverable(RecoveryAction::RevalidateToolRequest)
            }
            AgentState::AwaitingPermission { .. } => {
                RestartDisposition::Recoverable(RecoveryAction::ReissuePermission)
            }
            AgentState::ExecutingTool { .. } => {
                RestartDisposition::Unsafe(UnsafeInterruption::ToolOutcomeUnknown)
            }
            AgentState::ReturningToolResult { .. } => {
                RestartDisposition::Recoverable(RecoveryAction::ReturnRecordedToolResult)
            }
            AgentState::PreparingFinalAnswer => {
                RestartDisposition::Recoverable(RecoveryAction::ResumeFinalAnswer)
            }
            AgentState::Completed | AgentState::Failed { .. } | AgentState::Cancelled { .. } => {
                RestartDisposition::Terminal
            }
        }
    }

    /// Transfers ownership from generation to one validated tool request.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale owner or a request outside `Generating`.
    pub fn request_tool(
        &mut self,
        expected: &TransitionToken,
        request: &ValidatedToolRequest,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.commit(
            expected,
            AgentState::ToolRequested {
                tool: ToolRequestIdentity::from_validated(request),
            },
        )
    }

    /// Moves an interactive tool request into permission mediation.
    ///
    /// # Errors
    ///
    /// Returns an error unless the current exact tool request requires approval.
    pub fn await_permission(
        &mut self,
        expected: &TransitionToken,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.check_expected(expected)?;
        let AgentState::ToolRequested { tool } = &self.state else {
            return Err(self.invalid_to(AgentStateKind::AwaitingPermission));
        };
        if tool.approval() != ApprovalRequirement::Interactive {
            return Err(self.invalid_to(AgentStateKind::AwaitingPermission));
        }
        self.commit(
            expected,
            AgentState::AwaitingPermission { tool: tool.clone() },
        )
    }

    /// Starts a tool covered by an existing durable capability.
    ///
    /// # Errors
    ///
    /// Returns an error unless the exact requested tool does not require
    /// interactive approval.
    pub fn start_capability_tool(
        &mut self,
        expected: &TransitionToken,
        authorization: ExecutionAuthorization,
        cancellable: bool,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.check_expected(expected)?;
        let AgentState::ToolRequested { tool } = &self.state else {
            return Err(self.invalid_to(AgentStateKind::ExecutingTool));
        };
        if tool.approval() != ApprovalRequirement::CapabilityOnly {
            return Err(self.invalid_to(AgentStateKind::ExecutingTool));
        }
        let (binding, interactive) = authorization.into_parts();
        if interactive || !authorization_matches(tool, &binding) {
            return Err(AgentStateMachineError::MismatchedAuthorization);
        }
        self.commit(
            expected,
            AgentState::ExecutingTool {
                tool: tool.clone(),
                cancellable,
            },
        )
    }

    /// Records trusted approval and starts the exact pending tool request.
    ///
    /// # Errors
    ///
    /// Returns an error unless one interactively approved request is pending.
    pub fn approve_and_start_tool(
        &mut self,
        expected: &TransitionToken,
        authorization: ExecutionAuthorization,
        cancellable: bool,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.check_expected(expected)?;
        let AgentState::AwaitingPermission { tool } = &self.state else {
            return Err(self.invalid_to(AgentStateKind::ExecutingTool));
        };
        let (binding, interactive) = authorization.into_parts();
        if !interactive || !authorization_matches(tool, &binding) {
            return Err(AgentStateMachineError::MismatchedAuthorization);
        }
        self.commit(
            expected,
            AgentState::ExecutingTool {
                tool: tool.clone(),
                cancellable,
            },
        )
    }

    /// Binds one terminal result to the exact executing request.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ownership, a non-executing state, or a result
    /// with a different call ID or tool name.
    pub fn return_tool_result(
        &mut self,
        expected: &TransitionToken,
        result: &ToolResultEnvelope,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.check_expected(expected)?;
        let AgentState::ExecutingTool { tool, .. } = &self.state else {
            return Err(self.invalid_to(AgentStateKind::ReturningToolResult));
        };
        if result.call_id() != tool.call_id() || result.tool_name() != tool.tool_name() {
            return Err(AgentStateMachineError::MismatchedToolResult);
        }
        let outcome = match result.outcome() {
            ToolOutcome::Completed { .. } => ToolTerminalStatus::Completed,
            ToolOutcome::Failed { .. } => ToolTerminalStatus::Failed,
            ToolOutcome::Cancelled { .. } => ToolTerminalStatus::Cancelled,
        };
        self.commit(
            expected,
            AgentState::ReturningToolResult {
                tool: tool.clone(),
                outcome,
            },
        )
    }

    /// Starts the next inference round after a terminal tool result.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale owner, malformed generation ID, or a state
    /// other than `ReturningToolResult`.
    pub fn continue_generation(
        &mut self,
        expected: &TransitionToken,
        generation_id: impl Into<String>,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        let generation_id = generation_id.into();
        validate_id("generation ID", &generation_id)?;
        self.commit(expected, AgentState::Generating { generation_id })
    }

    /// Moves a generation into host-owned final-answer preparation.
    ///
    /// # Errors
    ///
    /// Returns an error unless one generation currently owns the run.
    pub fn prepare_final_answer(
        &mut self,
        expected: &TransitionToken,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.commit(expected, AgentState::PreparingFinalAnswer)
    }

    /// Marks final-answer publication complete.
    ///
    /// # Errors
    ///
    /// Returns an error unless final-answer preparation owns the run.
    pub fn complete(
        &mut self,
        expected: &TransitionToken,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.commit(expected, AgentState::Completed)
    }

    /// Terminates any nonterminal state with a sanitized trusted failure.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ownership or an already terminal run.
    pub fn fail(
        &mut self,
        expected: &TransitionToken,
        failure: AgentFailure,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.commit(expected, AgentState::Failed { failure })
    }

    /// Accepts cancellation only in explicitly cancellable states.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ownership, unsupported states, or a tool that
    /// declared itself non-cancellable.
    pub fn cancel(
        &mut self,
        expected: &TransitionToken,
        reason: RunCancellationReason,
    ) -> Result<CancellationReceipt, AgentStateMachineError> {
        self.check_expected(expected)?;
        let target = match &self.state {
            AgentState::Generating { generation_id } => {
                CancellationTarget::Generation(generation_id.clone())
            }
            AgentState::AwaitingPermission { tool } => {
                CancellationTarget::Permission(tool.call_id().to_owned())
            }
            AgentState::ExecutingTool {
                tool,
                cancellable: true,
            } => CancellationTarget::Tool(tool.call_id().to_owned()),
            AgentState::ExecutingTool {
                cancellable: false, ..
            } => return Err(AgentStateMachineError::ToolNotCancellable),
            _ => return Err(AgentStateMachineError::CancellationUnavailable),
        };
        let token = self.commit(expected, AgentState::Cancelled { reason })?;
        Ok(CancellationReceipt { token, target })
    }

    fn commit(
        &mut self,
        expected: &TransitionToken,
        next: AgentState,
    ) -> Result<TransitionToken, AgentStateMachineError> {
        self.check_expected(expected)?;
        if self.state.is_terminal() {
            return Err(AgentStateMachineError::TerminalState);
        }
        if self.transitions.len() >= MAX_TRANSITIONS {
            return Err(AgentStateMachineError::TransitionLimitExceeded);
        }
        let from = self.state.kind();
        let to = next.kind();
        if !valid_state_edge(&self.state, &next) {
            return Err(AgentStateMachineError::InvalidTransition { from, to });
        }
        next.validate()?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(AgentStateMachineError::TransitionLimitExceeded)?;
        self.state = next.clone();
        self.transitions.push(StateTransitionRecord {
            revision: self.revision,
            from: Some(from),
            state: next,
        });
        Ok(self.transition_token())
    }

    fn check_expected(&self, expected: &TransitionToken) -> Result<(), AgentStateMachineError> {
        if expected != &self.transition_token() {
            return Err(AgentStateMachineError::StaleTransition);
        }
        Ok(())
    }

    fn invalid_to(&self, to: AgentStateKind) -> AgentStateMachineError {
        AgentStateMachineError::InvalidTransition {
            from: self.state.kind(),
            to,
        }
    }
}

fn validate_snapshot(snapshot: &AgentStateSnapshot) -> Result<(), AgentStateMachineError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(AgentStateMachineError::InvalidSnapshot(
            "unsupported version".to_owned(),
        ));
    }
    validate_id("run ID", &snapshot.run_id)?;
    snapshot.state.validate()?;
    if snapshot.transitions.is_empty() || snapshot.transitions.len() > MAX_TRANSITIONS {
        return Err(AgentStateMachineError::InvalidSnapshot(
            "transition history length is invalid".to_owned(),
        ));
    }
    let mut previous_kind = None;
    let mut previous_state: Option<&AgentState> = None;
    for (index, transition) in snapshot.transitions.iter().enumerate() {
        transition.state.validate()?;
        let expected_revision = u64::try_from(index).map_err(|_| {
            AgentStateMachineError::InvalidSnapshot("transition revision overflow".to_owned())
        })?;
        if transition.revision != expected_revision || transition.from != previous_kind {
            return Err(AgentStateMachineError::InvalidSnapshot(
                "transition history is not contiguous".to_owned(),
            ));
        }
        if let Some(previous) = previous_state {
            if !valid_state_edge(previous, &transition.state) {
                return Err(AgentStateMachineError::InvalidSnapshot(
                    "transition history contains an invalid edge".to_owned(),
                ));
            }
        } else if transition.state.kind() != AgentStateKind::Generating {
            return Err(AgentStateMachineError::InvalidSnapshot(
                "initial state must be generating".to_owned(),
            ));
        }
        previous_kind = Some(transition.state.kind());
        previous_state = Some(&transition.state);
    }
    let last = snapshot.transitions.last().ok_or_else(|| {
        AgentStateMachineError::InvalidSnapshot("transition history is empty".to_owned())
    })?;
    if snapshot.revision != last.revision || snapshot.state != last.state {
        return Err(AgentStateMachineError::InvalidSnapshot(
            "current state does not match transition history".to_owned(),
        ));
    }
    Ok(())
}

const fn valid_edge(from: AgentStateKind, to: AgentStateKind) -> bool {
    matches!(
        (from, to),
        (
            AgentStateKind::Generating,
            AgentStateKind::ToolRequested
                | AgentStateKind::PreparingFinalAnswer
                | AgentStateKind::Failed
                | AgentStateKind::Cancelled
        ) | (
            AgentStateKind::ToolRequested,
            AgentStateKind::AwaitingPermission
                | AgentStateKind::ExecutingTool
                | AgentStateKind::Failed
        ) | (
            AgentStateKind::AwaitingPermission,
            AgentStateKind::ExecutingTool | AgentStateKind::Failed | AgentStateKind::Cancelled
        ) | (
            AgentStateKind::ExecutingTool,
            AgentStateKind::ReturningToolResult
                | AgentStateKind::Failed
                | AgentStateKind::Cancelled
        ) | (
            AgentStateKind::ReturningToolResult,
            AgentStateKind::Generating | AgentStateKind::Failed
        ) | (
            AgentStateKind::PreparingFinalAnswer,
            AgentStateKind::Completed | AgentStateKind::Failed
        )
    )
}

fn valid_state_edge(from: &AgentState, to: &AgentState) -> bool {
    if !valid_edge(from.kind(), to.kind()) {
        return false;
    }
    match (from, to) {
        (
            AgentState::ToolRequested { tool: current },
            AgentState::AwaitingPermission { tool: next }
            | AgentState::ExecutingTool { tool: next, .. },
        )
        | (
            AgentState::AwaitingPermission { tool: current },
            AgentState::ExecutingTool { tool: next, .. },
        )
        | (
            AgentState::ExecutingTool { tool: current, .. },
            AgentState::ReturningToolResult { tool: next, .. },
        ) => current == next,
        _ => true,
    }
}

fn authorization_matches(tool: &ToolRequestIdentity, binding: &crate::PermissionBinding) -> bool {
    binding.call_id() == tool.call_id()
        && binding.tool_name() == tool.tool_name()
        && binding.arguments_sha256() == tool.arguments_sha256()
        && binding.capability() == tool.capability()
}

#[cfg(test)]
#[path = "state_machine_tests.rs"]
mod tests;
