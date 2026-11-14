//! Receipt reports and independently verified proofs have distinct provenance.
use super::{StepLifecycle, Submission};
use chain_evm::{MonitorFailure, TransactionLifecycle};
use ipc_types::IpcError;
use serde::Serialize;
use serde_json::{Value, json};

pub(in crate::evm_approval::contracts::declared) const MAX_MONITOR_ATTEMPTS: u8 = 120;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Observation {
    receipt: Receipt,
    monitor_attempts: u8,
    last_monitor_failure: Option<MonitorFailure>,
    inclusion_proof: Proof,
    completion_proof: Completion,
    stop_reason: Option<ObservationStop>,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Receipt {
    #[default]
    NotStarted,
    Waiting,
    PendingReported,
    SuccessReported,
    RevertReported,
    NotFoundNonceAdvancedReported,
    Unavailable,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Proof {
    #[default]
    NotStarted,
    Checking,
    Verified,
    Unavailable,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Completion {
    #[default]
    NotStarted,
    Checking,
    Verified,
    Unsatisfied,
    Unavailable,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::evm_approval::contracts::declared) enum ObservationStop {
    BudgetElapsed,
    AttemptsExhausted,
    ContextUnavailable,
    WorkerInterrupted,
    MonitorRejected,
    PersistenceUnavailable,
    InclusionUnavailable,
    CompletionUnavailable,
}

impl StepLifecycle {
    pub(in crate::evm_approval::contracts::declared) fn receipt_wait_started(&mut self) {
        self.post_submission.receipt = Receipt::Waiting;
    }
    pub(in crate::evm_approval::contracts::declared) fn receipt_attempted(
        &mut self,
    ) -> Result<(), IpcError> {
        if self.post_submission.monitor_attempts >= MAX_MONITOR_ATTEMPTS {
            return Err(IpcError::invalid_request(
                "The bounded receipt monitoring attempt limit was reached",
            ));
        }
        self.post_submission.monitor_attempts += 1;
        Ok(())
    }
    pub(in crate::evm_approval::contracts::declared) fn receipt_monitor_failed(
        &mut self,
        failure: MonitorFailure,
    ) {
        self.post_submission.last_monitor_failure = Some(failure);
    }
    pub(in crate::evm_approval::contracts::declared) fn receipt_reported(
        &mut self,
        lifecycle: TransactionLifecycle,
    ) {
        self.post_submission.receipt = match lifecycle {
            TransactionLifecycle::Pending => Receipt::PendingReported,
            TransactionLifecycle::Confirmed => Receipt::SuccessReported,
            TransactionLifecycle::Failed => Receipt::RevertReported,
            TransactionLifecycle::DroppedOrReplaced => Receipt::NotFoundNonceAdvancedReported,
        };
        self.post_submission.last_monitor_failure = None;
    }
    pub(in crate::evm_approval::contracts::declared) fn observation_stopped(
        &mut self,
        reason: ObservationStop,
    ) {
        self.post_submission.stop_reason = Some(reason);
        match reason {
            ObservationStop::InclusionUnavailable => {
                self.post_submission.inclusion_proof = Proof::Unavailable;
            }
            ObservationStop::CompletionUnavailable => {
                self.post_submission.completion_proof = Completion::Unavailable;
            }
            ObservationStop::PersistenceUnavailable => {}
            _ => {
                if matches!(
                    self.post_submission.receipt,
                    Receipt::NotStarted
                        | Receipt::Waiting
                        | Receipt::PendingReported
                        | Receipt::Unavailable
                ) {
                    self.post_submission.receipt = Receipt::Unavailable;
                }
            }
        }
    }
    pub(in crate::evm_approval::contracts::declared) fn inclusion_started(&mut self) {
        self.post_submission.inclusion_proof = Proof::Checking;
    }
    pub(in crate::evm_approval::contracts::declared) fn inclusion_verified(&mut self) {
        self.post_submission.inclusion_proof = Proof::Verified;
    }
    pub(in crate::evm_approval::contracts::declared) fn completion_started(&mut self) {
        self.post_submission.completion_proof = Completion::Checking;
    }
    pub(in crate::evm_approval::contracts::declared) fn completion_verified(&mut self) {
        self.post_submission.completion_proof = Completion::Verified;
    }
    pub(in crate::evm_approval::contracts::declared) fn completion_unsatisfied(&mut self) {
        self.post_submission.completion_proof = Completion::Unsatisfied;
    }
    pub(in crate::evm_approval::contracts::declared) fn submitted_verification_unavailable(
        &self,
    ) -> bool {
        matches!(
            self.submission,
            Submission::Acknowledged | Submission::MayHaveBeenSent
        ) && self.post_submission.stop_reason.is_some()
    }
}

const RECEIPT_FACTS: [&str; 7] = [
    "Receipt monitoring was not started.",
    "Receipt monitoring started.",
    "RPC reports the transaction pending; no receipt was returned.",
    "RPC receipt reports success; canonical inclusion requires separate proof.",
    "RPC receipt reports a revert; this is a provider observation, not an independent inclusion proof.",
    "RPC reports no transaction or receipt and an advanced sender nonce; replacement is not independently proved.",
    "Receipt observation is unavailable; the transaction outcome is not established.",
];
const INCLUSION_FACTS: [&str; 4] = [
    "Canonical inclusion proof was not started.",
    "Canonical inclusion proof checking started.",
    "Canonical transaction and receipt inclusion proof verified.",
    "Usable canonical inclusion proof was unavailable.",
];
const COMPLETION_FACTS: [&str; 5] = [
    "Declared completion proof was not started.",
    "Declared completion proof checking started.",
    "Declared per-step completion conditions were verified and recorded; final task goals are separate.",
    "Declared completion conditions were unsatisfied at the proved observation.",
    "Declared completion verification was unavailable; no task completion is established.",
];
pub(super) fn facts(step: &StepLifecycle) -> [&'static str; 3] {
    let value = &step.post_submission;
    [
        RECEIPT_FACTS[match value.receipt {
            Receipt::NotStarted => 0,
            Receipt::Waiting => 1,
            Receipt::PendingReported => 2,
            Receipt::SuccessReported => 3,
            Receipt::RevertReported => 4,
            Receipt::NotFoundNonceAdvancedReported => 5,
            Receipt::Unavailable => 6,
        }],
        INCLUSION_FACTS[match value.inclusion_proof {
            Proof::NotStarted => 0,
            Proof::Checking => 1,
            Proof::Verified => 2,
            Proof::Unavailable => 3,
        }],
        COMPLETION_FACTS[match value.completion_proof {
            Completion::NotStarted => 0,
            Completion::Checking => 1,
            Completion::Verified => 2,
            Completion::Unsatisfied => 3,
            Completion::Unavailable => 4,
        }],
    ]
}
pub(super) fn maximum_fact_bytes() -> usize {
    [
        &RECEIPT_FACTS[..],
        &INCLUSION_FACTS[..],
        &COMPLETION_FACTS[..],
    ]
    .iter()
    .map(|choices| choices.iter().map(|fact| fact.len()).max().unwrap_or(0))
    .sum::<usize>()
        + 3
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ObservedStage {
    ReceiptWaitStarted,
    ReceiptPendingReported,
    ReceiptSuccessReported,
    ReceiptRevertReported,
    ReceiptNotFoundNonceAdvancedReported,
    ReceiptUnavailable,
    InclusionProofChecking,
    InclusionProofVerified,
    InclusionProofUnavailable,
    CompletionProofChecking,
    CompletionProofVerified,
    CompletionConditionsUnsatisfied,
    CompletionProofUnavailable,
    VerificationPersistenceUnavailable,
}
pub(super) fn latest_stage(step: &StepLifecycle) -> Option<ObservedStage> {
    let value = &step.post_submission;
    if matches!(
        value.stop_reason,
        Some(ObservationStop::PersistenceUnavailable)
    ) {
        return Some(ObservedStage::VerificationPersistenceUnavailable);
    }
    match value.completion_proof {
        Completion::Checking => return Some(ObservedStage::CompletionProofChecking),
        Completion::Verified => return Some(ObservedStage::CompletionProofVerified),
        Completion::Unsatisfied => return Some(ObservedStage::CompletionConditionsUnsatisfied),
        Completion::Unavailable => return Some(ObservedStage::CompletionProofUnavailable),
        Completion::NotStarted => {}
    }
    match value.inclusion_proof {
        Proof::Checking => return Some(ObservedStage::InclusionProofChecking),
        Proof::Verified => return Some(ObservedStage::InclusionProofVerified),
        Proof::Unavailable => return Some(ObservedStage::InclusionProofUnavailable),
        Proof::NotStarted => {}
    }
    match value.receipt {
        Receipt::NotStarted => None,
        Receipt::Waiting => Some(ObservedStage::ReceiptWaitStarted),
        Receipt::PendingReported => Some(ObservedStage::ReceiptPendingReported),
        Receipt::SuccessReported => Some(ObservedStage::ReceiptSuccessReported),
        Receipt::RevertReported => Some(ObservedStage::ReceiptRevertReported),
        Receipt::NotFoundNonceAdvancedReported => {
            Some(ObservedStage::ReceiptNotFoundNonceAdvancedReported)
        }
        Receipt::Unavailable => Some(ObservedStage::ReceiptUnavailable),
    }
}
/// A conservative data-only size fixture, never a monitoring result.
pub(super) fn maximum_observation() -> Value {
    json!({"receipt":"notFoundNonceAdvancedReported","monitorAttempts":MAX_MONITOR_ATTEMPTS,
        "lastMonitorFailure":{"stage":"transactionValidation",
            "cause":{"kind":"chainIdMismatch","expected":u64::MAX,"actual":u64::MAX},"elapsedMs":u64::MAX},
        "inclusionProof":"unavailable","completionProof":"unsatisfied",
        "stopReason":"persistenceUnavailable"})
}

#[cfg(test)]
mod tests;
