//! Trusted ordering and restart policy for one transaction lifecycle.

use std::fmt;

use serde::{Deserialize, Serialize};

const SNAPSHOT_VERSION: u16 = 1;
const MAX_ID_BYTES: usize = 128;
const MAX_TRANSITIONS: usize = 9;

/// Closed, presentation-safe transaction lifecycle kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStateKind {
    /// An immutable candidate has been proposed but not validated.
    Proposed,
    /// Trusted policy validated the candidate.
    Validated,
    /// The exact validated candidate was simulated.
    Simulated,
    /// The exact candidate is waiting in the trusted approval surface.
    AwaitingApproval,
    /// The user approved the exact candidate.
    Approved,
    /// The wallet boundary produced a signature.
    Signed,
    /// The network accepted the exact signed transaction.
    Broadcast,
    /// A successful on-chain receipt confirmed the transaction.
    Confirmed,
    /// The transaction terminated without confirmation.
    Failed,
}

/// Closed reasons why a transaction can terminate without confirmation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionFailureReason {
    /// Trusted candidate validation failed.
    ValidationFailed,
    /// Exact-candidate simulation could not complete acceptably.
    SimulationFailed,
    /// The trusted approval request could not be prepared.
    ApprovalPreparationFailed,
    /// The user explicitly denied the approval request.
    ApprovalDenied,
    /// The user dismissed the trusted approval surface.
    ApprovalDismissed,
    /// The pending approval was cancelled.
    ApprovalCancelled,
    /// The pending approval expired.
    ApprovalExpired,
    /// The approved candidate changed before signing.
    CandidateInvalidated,
    /// Trusted wallet signing failed or was refused.
    SigningFailed,
    /// Submission of the exact signed transaction failed.
    BroadcastFailed,
    /// Monitoring established a terminal on-chain failure.
    ConfirmationFailed,
    /// Application restart invalidated non-durable authority or work.
    ApplicationRestarted,
    /// A trusted internal invariant prevented safe progress.
    InternalInvariant,
}

/// Trusted state for one transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TransactionState {
    /// An immutable candidate has been proposed but not validated.
    Proposed,
    /// Trusted policy validated the candidate.
    Validated,
    /// The exact validated candidate was simulated.
    Simulated {
        /// Digest reported by the exact-candidate simulation.
        candidate_digest: String,
    },
    /// The exact candidate is waiting in the trusted approval surface.
    AwaitingApproval,
    /// The user approved the exact candidate.
    Approved,
    /// The wallet boundary produced a signature.
    Signed,
    /// The network accepted the exact signed transaction.
    Broadcast {
        /// Public node-accepted transaction identity.
        broadcast_reference: String,
    },
    /// A successful on-chain receipt confirmed the transaction.
    Confirmed {
        /// Public node-accepted transaction identity.
        broadcast_reference: String,
    },
    /// The transaction terminated without confirmation.
    Failed {
        /// Closed trusted reason for the terminal outcome.
        reason: TransactionFailureReason,
    },
}

impl TransactionState {
    /// Returns the closed kind without exposing state evidence.
    #[must_use]
    pub const fn kind(&self) -> TransactionStateKind {
        match self {
            Self::Proposed => TransactionStateKind::Proposed,
            Self::Validated => TransactionStateKind::Validated,
            Self::Simulated { .. } => TransactionStateKind::Simulated,
            Self::AwaitingApproval => TransactionStateKind::AwaitingApproval,
            Self::Approved => TransactionStateKind::Approved,
            Self::Signed => TransactionStateKind::Signed,
            Self::Broadcast { .. } => TransactionStateKind::Broadcast,
            Self::Confirmed { .. } => TransactionStateKind::Confirmed,
            Self::Failed { .. } => TransactionStateKind::Failed,
        }
    }

    /// Returns whether no later transition is permitted.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Confirmed { .. } | Self::Failed { .. })
    }

    /// Returns the public broadcast identity when one has been established.
    #[must_use]
    pub fn broadcast_reference(&self) -> Option<&str> {
        match self {
            Self::Broadcast {
                broadcast_reference,
            }
            | Self::Confirmed {
                broadcast_reference,
            } => Some(broadcast_reference),
            _ => None,
        }
    }

    /// Returns the terminal failure reason when the transaction failed.
    #[must_use]
    pub const fn failure_reason(&self) -> Option<TransactionFailureReason> {
        match self {
            Self::Failed { reason } => Some(*reason),
            _ => None,
        }
    }
}

/// Closed reason for one recorded lifecycle transition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionTransitionReason {
    /// A host-created immutable candidate began its lifecycle.
    Proposed,
    /// Trusted validation succeeded.
    ValidationSucceeded,
    /// Exact-candidate simulation completed.
    SimulationCompleted,
    /// The trusted host requested native approval.
    ApprovalRequested,
    /// The user approved in the trusted surface.
    UserApproved,
    /// The wallet boundary produced a signature.
    SignatureProduced,
    /// The network accepted the signed transaction.
    NodeAccepted,
    /// A successful receipt confirmed the transaction.
    ReceiptConfirmed,
    /// Progress terminated for a closed failure reason.
    Failed(TransactionFailureReason),
}

/// One immutable state transition suitable for non-secret persistence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionTransition {
    revision: u64,
    from: Option<TransactionStateKind>,
    state: TransactionState,
    reason: TransactionTransitionReason,
}

impl TransactionTransition {
    /// Returns the monotonically increasing transition revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the previous state kind, absent only for the proposal record.
    #[must_use]
    pub const fn from(&self) -> Option<TransactionStateKind> {
        self.from
    }

    /// Returns the state established by this transition.
    #[must_use]
    pub const fn state(&self) -> &TransactionState {
        &self.state
    }

    /// Returns the closed reason for this transition.
    #[must_use]
    pub const fn reason(&self) -> TransactionTransitionReason {
        self.reason
    }
}

/// Persistable non-secret transaction lifecycle state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionStateSnapshot {
    version: u16,
    transaction_id: String,
    candidate_digest: String,
    revision: u64,
    state: TransactionState,
    transitions: Vec<TransactionTransition>,
}

impl TransactionStateSnapshot {
    /// Returns the snapshot schema version.
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// Returns the host-created transaction identity.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Returns the immutable candidate digest.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }

    /// Returns the current state revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the current trusted state.
    #[must_use]
    pub const fn state(&self) -> &TransactionState {
        &self.state
    }

    /// Returns the complete bounded transition history.
    #[must_use]
    pub fn transitions(&self) -> &[TransactionTransition] {
        &self.transitions
    }
}

/// Public durable evidence allowed to resume post-broadcast monitoring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestartEvidence {
    candidate_digest: String,
    broadcast_reference: String,
}

impl RestartEvidence {
    /// Creates exact public evidence for a previously broadcast transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when either identity is malformed.
    pub fn broadcast(
        candidate_digest: impl Into<String>,
        broadcast_reference: impl Into<String>,
    ) -> Result<Self, TransactionLifecycleError> {
        let candidate_digest = candidate_digest.into();
        let broadcast_reference = broadcast_reference.into();
        validate_identity("candidate digest", &candidate_digest)?;
        validate_identity("broadcast reference", &broadcast_reference)?;
        Ok(Self {
            candidate_digest,
            broadcast_reference,
        })
    }
}

/// Revision-bound authority to request exactly one next state transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionTransitionToken {
    transaction_id: String,
    revision: u64,
    state: TransactionStateKind,
}

impl TransactionTransitionToken {
    /// Returns the revision this token owns.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

/// Errors raised when lifecycle ordering or restart evidence is unsafe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionLifecycleError {
    /// A host identity or public evidence reference was malformed.
    InvalidIdentity(&'static str),
    /// A token was reused or belongs to another transaction or state.
    StaleTransition,
    /// The requested edge is absent from the closed lifecycle graph.
    InvalidTransition {
        /// Current trusted state.
        from: TransactionStateKind,
        /// Rejected destination state.
        to: TransactionStateKind,
    },
    /// A terminal transaction received another transition request.
    TerminalState,
    /// Simulation evidence belongs to a different candidate.
    MismatchedCandidate,
    /// A receipt or confirmation named a different broadcast transaction.
    MismatchedBroadcastReference,
    /// A failure reason cannot terminate the current stage.
    InvalidFailureReason {
        /// Current trusted state.
        state: TransactionStateKind,
        /// Rejected failure reason.
        reason: TransactionFailureReason,
    },
    /// A persisted snapshot was malformed or internally inconsistent.
    InvalidSnapshot(&'static str),
    /// Broadcast monitoring had no matching durable public evidence.
    MissingRestartEvidence,
    /// Supplied durable public evidence did not match persisted state.
    MismatchedRestartEvidence,
    /// The bounded transition history cannot accept another record.
    TransitionLimitExceeded,
}

impl fmt::Display for TransactionLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(label) => write!(formatter, "{label} is malformed"),
            Self::StaleTransition => formatter.write_str("transaction transition token is stale"),
            Self::InvalidTransition { from, to } => {
                write!(
                    formatter,
                    "invalid transaction transition from {from:?} to {to:?}"
                )
            }
            Self::TerminalState => formatter.write_str("transaction is already terminal"),
            Self::MismatchedCandidate => {
                formatter.write_str("simulation evidence belongs to another candidate")
            }
            Self::MismatchedBroadcastReference => {
                formatter.write_str("confirmation belongs to another broadcast transaction")
            }
            Self::InvalidFailureReason { state, reason } => {
                write!(
                    formatter,
                    "failure reason {reason:?} is invalid from {state:?}"
                )
            }
            Self::InvalidSnapshot(reason) => {
                write!(formatter, "invalid transaction snapshot: {reason}")
            }
            Self::MissingRestartEvidence => {
                formatter.write_str("broadcast restart evidence is required")
            }
            Self::MismatchedRestartEvidence => {
                formatter.write_str("restart evidence does not match the broadcast transaction")
            }
            Self::TransitionLimitExceeded => {
                formatter.write_str("transaction transition history limit was reached")
            }
        }
    }
}

impl std::error::Error for TransactionLifecycleError {}

/// Trusted, revision-checked lifecycle for one exact transaction candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionStateMachine {
    transaction_id: String,
    candidate_digest: String,
    revision: u64,
    state: TransactionState,
    transitions: Vec<TransactionTransition>,
}

impl TransactionStateMachine {
    /// Starts one lifecycle for one immutable transaction candidate.
    ///
    /// # Errors
    ///
    /// Returns an error when a host-created identity or digest is malformed.
    pub fn propose(
        transaction_id: impl Into<String>,
        candidate_digest: impl Into<String>,
    ) -> Result<Self, TransactionLifecycleError> {
        let transaction_id = transaction_id.into();
        let candidate_digest = candidate_digest.into();
        validate_identity("transaction ID", &transaction_id)?;
        validate_identity("candidate digest", &candidate_digest)?;
        let state = TransactionState::Proposed;
        Ok(Self {
            transaction_id,
            candidate_digest,
            revision: 0,
            transitions: vec![TransactionTransition {
                revision: 0,
                from: None,
                state: state.clone(),
                reason: TransactionTransitionReason::Proposed,
            }],
            state,
        })
    }

    /// Restores and validates a complete snapshot.
    ///
    /// Proposed through signed work is failed on restart because snapshot status
    /// cannot recreate a candidate, approval, or signature capability. Broadcast
    /// monitoring resumes only with matching public durable evidence.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed history or missing/mismatched broadcast
    /// evidence.
    pub fn restore(
        snapshot: TransactionStateSnapshot,
        restart_evidence: Option<&RestartEvidence>,
    ) -> Result<Self, TransactionLifecycleError> {
        validate_snapshot(&snapshot)?;
        let mut machine = Self {
            transaction_id: snapshot.transaction_id,
            candidate_digest: snapshot.candidate_digest,
            revision: snapshot.revision,
            state: snapshot.state,
            transitions: snapshot.transitions,
        };
        match &machine.state {
            TransactionState::Broadcast {
                broadcast_reference,
            } => {
                let evidence =
                    restart_evidence.ok_or(TransactionLifecycleError::MissingRestartEvidence)?;
                if evidence.candidate_digest != machine.candidate_digest
                    || evidence.broadcast_reference != *broadcast_reference
                {
                    return Err(TransactionLifecycleError::MismatchedRestartEvidence);
                }
            }
            TransactionState::Confirmed { .. } | TransactionState::Failed { .. } => {}
            TransactionState::Proposed
            | TransactionState::Validated
            | TransactionState::Simulated { .. }
            | TransactionState::AwaitingApproval
            | TransactionState::Approved
            | TransactionState::Signed => {
                let token = machine.transition_token();
                machine.fail(&token, TransactionFailureReason::ApplicationRestarted)?;
            }
        }
        Ok(machine)
    }

    /// Returns a persistable copy of non-secret lifecycle state.
    #[must_use]
    pub fn snapshot(&self) -> TransactionStateSnapshot {
        TransactionStateSnapshot {
            version: SNAPSHOT_VERSION,
            transaction_id: self.transaction_id.clone(),
            candidate_digest: self.candidate_digest.clone(),
            revision: self.revision,
            state: self.state.clone(),
            transitions: self.transitions.clone(),
        }
    }

    /// Returns the immutable candidate digest owned by this lifecycle.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }

    /// Returns the current trusted state.
    #[must_use]
    pub const fn state(&self) -> &TransactionState {
        &self.state
    }

    /// Returns the only token that can own the next transition.
    #[must_use]
    pub fn transition_token(&self) -> TransactionTransitionToken {
        TransactionTransitionToken {
            transaction_id: self.transaction_id.clone(),
            revision: self.revision,
            state: self.state.kind(),
        }
    }

    /// Records successful trusted validation.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale token or a transition from another stage.
    pub fn validate(
        &mut self,
        expected: &TransactionTransitionToken,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.commit(
            expected,
            TransactionState::Validated,
            TransactionTransitionReason::ValidationSucceeded,
        )
    }

    /// Records simulation of the exact validated candidate.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ordering, an invalid stage, or evidence bound
    /// to another candidate.
    pub fn simulate(
        &mut self,
        expected: &TransactionTransitionToken,
        simulated_candidate_digest: &str,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.check_expected(expected)?;
        if simulated_candidate_digest != self.candidate_digest {
            return Err(TransactionLifecycleError::MismatchedCandidate);
        }
        self.commit(
            expected,
            TransactionState::Simulated {
                candidate_digest: simulated_candidate_digest.to_owned(),
            },
            TransactionTransitionReason::SimulationCompleted,
        )
    }

    /// Moves the exact simulated candidate into trusted native approval.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale token or a transition from another stage.
    pub fn await_approval(
        &mut self,
        expected: &TransactionTransitionToken,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.commit(
            expected,
            TransactionState::AwaitingApproval,
            TransactionTransitionReason::ApprovalRequested,
        )
    }

    /// Records an approval decision from the trusted native surface.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale token or a transition from another stage.
    pub fn approve(
        &mut self,
        expected: &TransactionTransitionToken,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.commit(
            expected,
            TransactionState::Approved,
            TransactionTransitionReason::UserApproved,
        )
    }

    /// Records successful signing inside the wallet boundary.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale token or a transition from another stage.
    pub fn sign(
        &mut self,
        expected: &TransactionTransitionToken,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.commit(
            expected,
            TransactionState::Signed,
            TransactionTransitionReason::SignatureProduced,
        )
    }

    /// Records network acceptance of the exact signed transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ordering, an invalid stage, or a malformed
    /// public broadcast identity.
    pub fn broadcast(
        &mut self,
        expected: &TransactionTransitionToken,
        broadcast_reference: impl Into<String>,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.check_expected(expected)?;
        let broadcast_reference = broadcast_reference.into();
        validate_identity("broadcast reference", &broadcast_reference)?;
        self.commit(
            expected,
            TransactionState::Broadcast {
                broadcast_reference,
            },
            TransactionTransitionReason::NodeAccepted,
        )
    }

    /// Records a successful receipt for the exact broadcast transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ordering, an invalid stage, or a mismatched
    /// public broadcast identity.
    pub fn confirm(
        &mut self,
        expected: &TransactionTransitionToken,
        broadcast_reference: &str,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.check_expected(expected)?;
        let TransactionState::Broadcast {
            broadcast_reference: current,
        } = &self.state
        else {
            return Err(self.invalid_to(TransactionStateKind::Confirmed));
        };
        if current != broadcast_reference {
            return Err(TransactionLifecycleError::MismatchedBroadcastReference);
        }
        self.commit(
            expected,
            TransactionState::Confirmed {
                broadcast_reference: broadcast_reference.to_owned(),
            },
            TransactionTransitionReason::ReceiptConfirmed,
        )
    }

    /// Terminates the current fallible stage with a closed trusted reason.
    ///
    /// # Errors
    ///
    /// Returns an error for stale ordering, a terminal state, or a reason that
    /// does not apply to the current stage.
    pub fn fail(
        &mut self,
        expected: &TransactionTransitionToken,
        reason: TransactionFailureReason,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.check_expected(expected)?;
        if self.state.is_terminal() {
            return Err(TransactionLifecycleError::TerminalState);
        }
        if !failure_allowed(self.state.kind(), reason) {
            return Err(TransactionLifecycleError::InvalidFailureReason {
                state: self.state.kind(),
                reason,
            });
        }
        self.commit(
            expected,
            TransactionState::Failed { reason },
            TransactionTransitionReason::Failed(reason),
        )
    }

    fn commit(
        &mut self,
        expected: &TransactionTransitionToken,
        next: TransactionState,
        reason: TransactionTransitionReason,
    ) -> Result<TransactionTransitionToken, TransactionLifecycleError> {
        self.check_expected(expected)?;
        if self.state.is_terminal() {
            return Err(TransactionLifecycleError::TerminalState);
        }
        if self.transitions.len() >= MAX_TRANSITIONS {
            return Err(TransactionLifecycleError::TransitionLimitExceeded);
        }
        let from = self.state.kind();
        let to = next.kind();
        if !valid_edge(&self.state, &next) || !reason_matches(from, &next, reason) {
            return Err(TransactionLifecycleError::InvalidTransition { from, to });
        }
        validate_state(&next, &self.candidate_digest)?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(TransactionLifecycleError::TransitionLimitExceeded)?;
        self.state = next.clone();
        self.transitions.push(TransactionTransition {
            revision: self.revision,
            from: Some(from),
            state: next,
            reason,
        });
        Ok(self.transition_token())
    }

    fn check_expected(
        &self,
        expected: &TransactionTransitionToken,
    ) -> Result<(), TransactionLifecycleError> {
        if expected != &self.transition_token() {
            return Err(TransactionLifecycleError::StaleTransition);
        }
        Ok(())
    }

    fn invalid_to(&self, to: TransactionStateKind) -> TransactionLifecycleError {
        TransactionLifecycleError::InvalidTransition {
            from: self.state.kind(),
            to,
        }
    }
}

fn validate_snapshot(snapshot: &TransactionStateSnapshot) -> Result<(), TransactionLifecycleError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(TransactionLifecycleError::InvalidSnapshot(
            "unsupported version",
        ));
    }
    validate_identity("transaction ID", &snapshot.transaction_id)?;
    validate_identity("candidate digest", &snapshot.candidate_digest)?;
    if snapshot.transitions.is_empty() || snapshot.transitions.len() > MAX_TRANSITIONS {
        return Err(TransactionLifecycleError::InvalidSnapshot(
            "transition history length is invalid",
        ));
    }
    let mut previous_state: Option<&TransactionState> = None;
    for (index, transition) in snapshot.transitions.iter().enumerate() {
        let expected_revision = u64::try_from(index).map_err(|_| {
            TransactionLifecycleError::InvalidSnapshot("transition revision overflow")
        })?;
        if transition.revision != expected_revision {
            return Err(TransactionLifecycleError::InvalidSnapshot(
                "transition revisions are not contiguous",
            ));
        }
        validate_state(&transition.state, &snapshot.candidate_digest)?;
        match previous_state {
            None => {
                if transition.from.is_some()
                    || transition.state != TransactionState::Proposed
                    || transition.reason != TransactionTransitionReason::Proposed
                {
                    return Err(TransactionLifecycleError::InvalidSnapshot(
                        "initial proposal record is invalid",
                    ));
                }
            }
            Some(previous) => {
                if transition.from != Some(previous.kind())
                    || !valid_edge(previous, &transition.state)
                    || !reason_matches(previous.kind(), &transition.state, transition.reason)
                {
                    return Err(TransactionLifecycleError::InvalidSnapshot(
                        "transition edge or reason is invalid",
                    ));
                }
            }
        }
        previous_state = Some(&transition.state);
    }
    let last = snapshot
        .transitions
        .last()
        .ok_or(TransactionLifecycleError::InvalidSnapshot(
            "transition history is empty",
        ))?;
    if snapshot.revision != last.revision || snapshot.state != last.state {
        return Err(TransactionLifecycleError::InvalidSnapshot(
            "current state does not match transition history",
        ));
    }
    Ok(())
}

fn validate_state(
    state: &TransactionState,
    candidate_digest: &str,
) -> Result<(), TransactionLifecycleError> {
    match state {
        TransactionState::Simulated {
            candidate_digest: simulated,
        } if simulated != candidate_digest => Err(TransactionLifecycleError::MismatchedCandidate),
        TransactionState::Broadcast {
            broadcast_reference,
        }
        | TransactionState::Confirmed {
            broadcast_reference,
        } => validate_identity("broadcast reference", broadcast_reference),
        _ => Ok(()),
    }
}

fn valid_edge(from: &TransactionState, to: &TransactionState) -> bool {
    if from.is_terminal() {
        return false;
    }
    match (from, to) {
        (TransactionState::Proposed, TransactionState::Validated)
        | (TransactionState::Validated, TransactionState::Simulated { .. })
        | (TransactionState::Simulated { .. }, TransactionState::AwaitingApproval)
        | (TransactionState::AwaitingApproval, TransactionState::Approved)
        | (TransactionState::Approved, TransactionState::Signed)
        | (TransactionState::Signed, TransactionState::Broadcast { .. })
        | (_, TransactionState::Failed { .. }) => true,
        (
            TransactionState::Broadcast {
                broadcast_reference: broadcast,
            },
            TransactionState::Confirmed {
                broadcast_reference: confirmed,
            },
        ) => broadcast == confirmed,
        _ => false,
    }
}

fn reason_matches(
    from: TransactionStateKind,
    to: &TransactionState,
    reason: TransactionTransitionReason,
) -> bool {
    match (from, to, reason) {
        (
            TransactionStateKind::Proposed,
            TransactionState::Validated,
            TransactionTransitionReason::ValidationSucceeded,
        )
        | (
            TransactionStateKind::Validated,
            TransactionState::Simulated { .. },
            TransactionTransitionReason::SimulationCompleted,
        )
        | (
            TransactionStateKind::Simulated,
            TransactionState::AwaitingApproval,
            TransactionTransitionReason::ApprovalRequested,
        )
        | (
            TransactionStateKind::AwaitingApproval,
            TransactionState::Approved,
            TransactionTransitionReason::UserApproved,
        )
        | (
            TransactionStateKind::Approved,
            TransactionState::Signed,
            TransactionTransitionReason::SignatureProduced,
        )
        | (
            TransactionStateKind::Signed,
            TransactionState::Broadcast { .. },
            TransactionTransitionReason::NodeAccepted,
        )
        | (
            TransactionStateKind::Broadcast,
            TransactionState::Confirmed { .. },
            TransactionTransitionReason::ReceiptConfirmed,
        ) => true,
        (
            _,
            TransactionState::Failed {
                reason: state_reason,
            },
            TransactionTransitionReason::Failed(reason),
        ) => *state_reason == reason && failure_allowed(from, reason),
        _ => false,
    }
}

fn failure_allowed(state: TransactionStateKind, reason: TransactionFailureReason) -> bool {
    match reason {
        TransactionFailureReason::ValidationFailed => state == TransactionStateKind::Proposed,
        TransactionFailureReason::SimulationFailed => state == TransactionStateKind::Validated,
        TransactionFailureReason::ApprovalPreparationFailed => {
            state == TransactionStateKind::Simulated
        }
        TransactionFailureReason::ApprovalDenied
        | TransactionFailureReason::ApprovalDismissed
        | TransactionFailureReason::ApprovalCancelled
        | TransactionFailureReason::ApprovalExpired => {
            state == TransactionStateKind::AwaitingApproval
        }
        TransactionFailureReason::CandidateInvalidated => matches!(
            state,
            TransactionStateKind::Proposed
                | TransactionStateKind::Validated
                | TransactionStateKind::Simulated
                | TransactionStateKind::AwaitingApproval
                | TransactionStateKind::Approved
        ),
        TransactionFailureReason::SigningFailed => state == TransactionStateKind::Approved,
        TransactionFailureReason::BroadcastFailed => state == TransactionStateKind::Signed,
        TransactionFailureReason::ConfirmationFailed => state == TransactionStateKind::Broadcast,
        TransactionFailureReason::ApplicationRestarted => matches!(
            state,
            TransactionStateKind::Proposed
                | TransactionStateKind::Validated
                | TransactionStateKind::Simulated
                | TransactionStateKind::AwaitingApproval
                | TransactionStateKind::Approved
                | TransactionStateKind::Signed
        ),
        TransactionFailureReason::InternalInvariant => !matches!(
            state,
            TransactionStateKind::Confirmed | TransactionStateKind::Failed
        ),
    }
}

fn validate_identity(label: &'static str, value: &str) -> Result<(), TransactionLifecycleError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(TransactionLifecycleError::InvalidIdentity(label));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
