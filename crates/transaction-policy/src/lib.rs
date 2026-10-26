//! Transaction candidate binding, approvals and policy decisions.

mod approval;
mod lifecycle;

pub use approval::{
    APPROVAL_POLICY_REVISION, ApprovalBindingInput, ApprovalCommitment, ApprovalCommitmentError,
    ApprovalConsumptionOutcome, ApprovalConsumptionRecord, ApprovalConsumptionStore,
    ApprovalFinalization, ApprovalInvalidationReason, ApprovalTransactionType, ConsumedApproval,
    MAX_APPROVAL_LIFETIME_SECONDS, VerifiedApproval,
};

pub use lifecycle::{
    RestartEvidence, TransactionFailureReason, TransactionLifecycleError, TransactionState,
    TransactionStateKind, TransactionStateMachine, TransactionStateSnapshot, TransactionTransition,
    TransactionTransitionReason, TransactionTransitionToken,
};
