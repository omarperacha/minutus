//! Request-local host observations, never permission or proof of durable state.
mod authorization;
mod outcome;
use crate::wallet_lifecycle::{TransferCancellationReason, WalletAuthorizationEvent};
use ipc_types::{EvmApprovalDecision, IpcError, WalletAvailability};
use serde::Serialize;
use serde_json::{Value, json};

const MAX_STEPS: usize = 16;
const MAX_AUTHORIZATION_EVENTS: usize = 8;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Lifecycle {
    wallet_readiness: Readiness,
    scope_review: Review,
    source_preflight: Progress,
    steps: Vec<StepLifecycle>,
    request_cancellation: Option<TransferCancellationReason>,
}

impl Lifecycle {
    pub(super) fn record_request_cancellation(&mut self, reason: TransferCancellationReason) {
        self.request_cancellation.get_or_insert(reason);
    }
    pub(super) fn wallet_readiness_requested(&mut self, availability: WalletAvailability) {
        self.wallet_readiness.unlock_required = Some(availability == WalletAvailability::Locked);
        self.wallet_readiness.initial_availability = Some(availability);
        self.wallet_readiness.status = Progress::Started;
    }
    pub(super) fn wallet_ready(&mut self) {
        self.wallet_readiness.status = Progress::Succeeded;
    }
    pub(super) fn scope_review_requested(&mut self) {
        self.scope_review = Review::Requested;
    }
    pub(super) fn scope_decision(&mut self, decision: EvmApprovalDecision) {
        self.scope_review = decision.into();
    }
    pub(super) fn source_preflight_started(&mut self) {
        self.source_preflight = Progress::Started;
    }
    pub(super) fn source_preflight_succeeded(&mut self) {
        self.source_preflight = Progress::Succeeded;
    }
    pub(super) fn step(&mut self, index: usize) -> Option<&mut StepLifecycle> {
        if index >= MAX_STEPS {
            return None;
        }
        if let Some(position) = self.steps.iter().position(|step| step.step_index == index) {
            return self.steps.get_mut(position);
        }
        self.steps.push(StepLifecycle {
            step_index: index,
            ..StepLifecycle::default()
        });
        self.steps.last_mut()
    }
    pub(super) fn facts(&self) -> Value {
        json!({"observationScope":"thisInvocationOnly", "observations":self,
            "promptVisibility":"notRecordedForReadinessOrTransactionReview; native password visibility is recorded only by passwordPresented events; a readiness or review request alone does not prove a prompt was displayed",
            "failureInterpretation":"failure names do not establish which earlier lifecycle events occurred",
            "durableState":"these observations do not replace transaction and nonce journal reconciliation"})
    }
    pub(super) fn project_error(
        &self,
        mut error: IpcError,
        completed_steps: usize,
        partial_facts: &Value,
    ) -> IpcError {
        // Completed steps count verified completions, independently of these
        // observations of approval, signing and transport.
        error.message = outcome::project(
            self,
            json!({"state":"stopped","message":error.message,
            "completedSteps":completed_steps,"partialFacts":partial_facts,
            "lifecycle":self.facts(),"retryAutomatically":false,"automaticCleanup":false}),
        )
        .to_string();
        error.retryable = false;
        error
    }
    pub(super) fn project_success(&self, mut value: Value) -> Value {
        if let Some(object) = value.as_object_mut() {
            object.insert("lifecycle".into(), self.facts());
            outcome::project(self, value)
        } else {
            outcome::project(self, json!({"result":value,"lifecycle":self.facts()}))
        }
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    status: Progress,
    initial_availability: Option<WalletAvailability>,
    unlock_required: Option<bool>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StepLifecycle {
    step_index: usize,
    exact_review: Review,
    authorization: Authorization,
    signing: Signing,
    presend_refresh: Progress,
    submission: Submission,
    transaction_hash: Option<String>,
    authorization_events: Vec<WalletAuthorizationEvent>,
    request_cancellation: Option<TransferCancellationReason>,
}
impl StepLifecycle {
    pub(super) fn record_authorization_event(
        &mut self,
        event: WalletAuthorizationEvent,
    ) -> Result<(), IpcError> {
        if self.authorization_events.len() >= MAX_AUTHORIZATION_EVENTS {
            return Err(IpcError::invalid_request(
                "The bounded native authentication observation limit was exceeded",
            ));
        }
        if matches!(event, WalletAuthorizationEvent::AuthenticationSucceeded) {
            self.authentication_succeeded();
        }
        self.authorization_events.push(event);
        Ok(())
    }
    pub(super) fn record_request_cancellation(&mut self, reason: TransferCancellationReason) {
        self.request_cancellation.get_or_insert(reason);
    }
    pub(super) fn exact_review_requested(&mut self) {
        self.exact_review = Review::Requested;
    }
    pub(super) fn exact_review_decision(&mut self, decision: EvmApprovalDecision) {
        self.exact_review = decision.into();
    }
    pub(super) fn authorization_requested(&mut self) {
        self.authorization = Authorization::Requested;
    }
    /// Called on entry to the wallet owner's post-authentication revalidation.
    pub(super) fn authentication_succeeded(&mut self) {
        self.authorization = Authorization::Authenticated;
    }
    /// Called only after revalidation succeeds, immediately before signing.
    /// Any subsequent error is not proof that a signature was never produced.
    pub(super) fn signing_started(&mut self) {
        self.signing = Signing::StartedOutcomeUnknown;
    }
    pub(super) fn signature_returned(&mut self) {
        self.signing = Signing::SignatureReturned;
    }
    pub(super) fn presend_refresh_started(&mut self) {
        self.presend_refresh = Progress::Started;
    }
    pub(super) fn presend_refresh_succeeded(&mut self) {
        self.presend_refresh = Progress::Succeeded;
    }
    /// Public identifier only; this observation grants no signing or send authority.
    pub(super) fn record_transaction_hash(&mut self, hash: &str) -> Result<(), IpcError> {
        if hash.len() != 66
            || !hash.starts_with("0x")
            || !hash.as_bytes()[2..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            || self
                .transaction_hash
                .as_ref()
                .is_some_and(|known| known != hash)
        {
            return Err(IpcError::invalid_request(
                "The transaction lifecycle identifier is invalid or changed",
            ));
        }
        self.transaction_hash = Some(hash.to_owned());
        Ok(())
    }
    /// Includes an admitted send or an indeterminate failure after admission began.
    pub(super) fn submission_may_have_been_sent(&mut self) {
        if self.submission != Submission::Acknowledged {
            self.submission = Submission::MayHaveBeenSent;
        }
    }
    /// Transport acknowledgement is recorded before any later journal update.
    pub(super) fn submission_acknowledged(&mut self) {
        self.submission = Submission::Acknowledged;
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Progress {
    #[default]
    NotStarted,
    Started,
    Succeeded,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Review {
    #[default]
    NotRequested,
    Requested,
    Approved,
    Denied,
    Dismissed,
}
impl From<EvmApprovalDecision> for Review {
    fn from(value: EvmApprovalDecision) -> Self {
        match value {
            EvmApprovalDecision::Approved => Self::Approved,
            EvmApprovalDecision::Denied => Self::Denied,
            EvmApprovalDecision::Dismissed => Self::Dismissed,
        }
    }
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Authorization {
    #[default]
    NotRequested,
    Requested,
    Authenticated,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
enum Signing {
    #[default]
    NotStarted,
    StartedOutcomeUnknown,
    SignatureReturned,
}
#[derive(Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Submission {
    #[default]
    NotAttempted,
    MayHaveBeenSent,
    Acknowledged,
}

/// Size-only reservation for the immutable admitted step count, including the
/// `IpcError` JSON string quoting layer. Invalid counts cannot admit a report.
pub(super) fn maximum_projection_bytes(step_count: usize) -> Option<usize> {
    if !(1..=MAX_STEPS).contains(&step_count) {
        return None;
    }
    let mut maximum = Lifecycle {
        wallet_readiness: Readiness {
            status: Progress::NotStarted,
            initial_availability: Some(WalletAvailability::Unavailable),
            unlock_required: Some(false),
        },
        scope_review: Review::NotRequested,
        source_preflight: Progress::NotStarted,
        steps: Vec::new(),
        request_cancellation: Some(TransferCancellationReason::WalletEpochChanged),
    };
    for index in 0..step_count {
        maximum.steps.push(StepLifecycle {
            step_index: index,
            exact_review: Review::NotRequested,
            authorization: Authorization::Authenticated,
            signing: Signing::StartedOutcomeUnknown,
            presend_refresh: Progress::NotStarted,
            submission: Submission::MayHaveBeenSent,
            transaction_hash: Some(format!("0x{}", "f".repeat(64))),
            authorization_events: vec![authorization::largest_event(); MAX_AUTHORIZATION_EVENTS],
            request_cancellation: Some(TransferCancellationReason::WalletEpochChanged),
        });
    }
    // The fixed margin covers the outer wrapper's keys and status strings.
    Some(
        json!({"lifecycle":maximum.facts().to_string()})
            .to_string()
            .len()
            + outcome::maximum_projection_bytes(step_count)
            + 512,
    )
}

#[cfg(test)]
mod tests;
