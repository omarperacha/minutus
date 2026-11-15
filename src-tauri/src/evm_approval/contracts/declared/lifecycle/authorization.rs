//! Typed native observations, independent of public error codes or prose.
use super::StepLifecycle;
use crate::wallet_lifecycle::{
    TransferCancellationReason, WalletAuthenticationFailure, WalletAuthorizationEvent,
    WalletAuthorizationStage, WalletPasswordDecision, WalletPasswordPurpose,
};

const FACTS: [&str; 12] = [
    "The native password was requested; presentation was not recorded.",
    "The native password dialog was presented.",
    "The native password dialog was accepted; acceptance alone does not establish successful authentication.",
    "The native password Cancel button was selected.",
    "The host aborted the native password dialog; this is not an observed user Cancel click.",
    "The native password dialog had another dismissal; its cause was not established.",
    "The native authentication job was invalidated before a native password result was received.",
    "The native authentication job was invalidated after the native password dialog returned.",
    "Native password authentication started.",
    "Native password authentication succeeded.",
    "Native password authentication was cancelled; this does not establish a user Cancel click.",
    "Native authentication reported credentials rejected; the specific password or vault cause was not established.",
];
const UNAVAILABLE: &str =
    "Native password authentication was unavailable; no specific underlying cause was established.";
const CANCELLATION: [&str; 4] = [
    "The host observed request cancellation.",
    "The host observed the request deadline elapse.",
    "The host observed interruption; no more specific cause was established.",
    "The host observed a wallet epoch change; this alone does not identify the security event or current wallet lock state.",
];

pub(super) fn event_facts(step: &StepLifecycle) -> [Option<&'static str>; 2] {
    let events = &step.authorization_events;
    let decision = events
        .iter()
        .rposition(|event| matches!(event, WalletAuthorizationEvent::PasswordDecision { .. }));
    let prior_decision = decision
        .filter(|index| Some(*index) != events.len().checked_sub(1))
        .and_then(|index| events.get(index))
        .map(|event| event_fact(*event));
    [
        prior_decision,
        events.last().map(|event| event_fact(*event)),
    ]
}

fn event_fact(event: WalletAuthorizationEvent) -> &'static str {
    match event {
        WalletAuthorizationEvent::PasswordRequested { .. } => FACTS[0],
        WalletAuthorizationEvent::PasswordPresented { .. } => FACTS[1],
        WalletAuthorizationEvent::PasswordDecision { decision, .. } => match decision {
            WalletPasswordDecision::Accepted => FACTS[2],
            WalletPasswordDecision::CancelButton => FACTS[3],
            WalletPasswordDecision::HostAbort => FACTS[4],
            WalletPasswordDecision::OtherDismissal => FACTS[5],
        },
        WalletAuthorizationEvent::JobInvalidated { stage } => match stage {
            WalletAuthorizationStage::BeforePassword => FACTS[6],
            WalletAuthorizationStage::AfterPassword => FACTS[7],
        },
        WalletAuthorizationEvent::AuthenticationStarted => FACTS[8],
        WalletAuthorizationEvent::AuthenticationSucceeded => FACTS[9],
        WalletAuthorizationEvent::AuthenticationFailed { failure } => match failure {
            WalletAuthenticationFailure::Cancelled => FACTS[10],
            WalletAuthenticationFailure::CredentialsRejected => FACTS[11],
            WalletAuthenticationFailure::Unavailable => UNAVAILABLE,
        },
    }
}

pub(super) fn cancellation_fact(reason: TransferCancellationReason) -> &'static str {
    match reason {
        TransferCancellationReason::RequestCancelled => CANCELLATION[0],
        TransferCancellationReason::DeadlineElapsed => CANCELLATION[1],
        TransferCancellationReason::Interrupted => CANCELLATION[2],
        TransferCancellationReason::WalletEpochChanged => CANCELLATION[3],
    }
}

pub(super) fn maximum_fact_bytes() -> usize {
    FACTS
        .iter()
        .map(|fact| fact.len())
        .max()
        .unwrap_or(0)
        .max(UNAVAILABLE.len())
}
pub(super) fn maximum_cancellation_bytes() -> usize {
    CANCELLATION
        .iter()
        .map(|fact| fact.len())
        .max()
        .unwrap_or(0)
}

/// Size fixture only; never a runtime observation or authority constructor.
pub(super) fn largest_event() -> WalletAuthorizationEvent {
    WalletAuthorizationEvent::PasswordDecision {
        purpose: WalletPasswordPurpose::OtherWalletOperation,
        decision: WalletPasswordDecision::OtherDismissal,
    }
}

#[cfg(test)]
mod tests;
