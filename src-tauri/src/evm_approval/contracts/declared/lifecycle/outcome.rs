//! Compact host facts beside the full lifecycle, never an error-message diagnosis.
use super::{
    Authorization, Lifecycle, Progress, Review, Signing, StepLifecycle, Submission, authorization,
};
use crate::wallet_lifecycle::{WalletAuthorizationEvent, WalletPasswordDecision};
use serde::Serialize;
use serde_json::{Value, json};
use std::fmt::Write as _;

const READINESS: [&str; 5] = [
    "Initial wallet readiness was not requested.",
    "Initial wallet readiness was requested; no successful completion was recorded.",
    "Initial wallet unlocking was requested; no successful readiness completion was recorded.",
    "Initial wallet readiness succeeded.",
    "Initial wallet unlock succeeded; the initial unlock was not cancelled.",
];
const SCOPE: [&str; 5] = [
    "Outbound data-access consent was not requested.",
    "Outbound data-access consent was requested; no decision was recorded.",
    "Outbound data-access consent was approved.",
    "Outbound data-access consent was denied.",
    "Outbound data-access consent was dismissed.",
];
const SOURCE: [&str; 3] = [
    "Source preflight was not started.",
    "Source preflight started; no successful completion was recorded.",
    "Source preflight succeeded.",
];
const EXACT: [&str; 5] = [
    "Exact transaction review was not requested.",
    "Exact transaction review was requested; no decision was recorded.",
    "Exact transaction review was approved.",
    "Exact transaction review was denied.",
    "Exact transaction review was dismissed.",
];
const AUTH: [&str; 3] = [
    "Fresh signing authentication was not requested.",
    "Fresh signing authentication was requested; no successful authentication was recorded. These facts do not establish why it stopped or whether the password was wrong.",
    "Fresh signing authentication succeeded.",
];
const SIGN: [&str; 3] = [
    "Signing was not started.",
    "Signing started; whether a signature was produced is unknown.",
    "A signature was returned to the host.",
];
const PRESEND: [&str; 3] = [
    "Pre-send refresh was not started.",
    "Pre-send refresh started; no successful completion was recorded.",
    "Pre-send refresh succeeded.",
];
const SUBMISSION: [&str; 3] = [
    "Submission was not attempted.",
    "Submission may have been sent; delivery is uncertain.",
    "Submission was acknowledged; acknowledgement alone does not establish inclusion or goal completion.",
];
const END: &str = "These facts describe this invocation only. Current wallet lock state was not observed. A generic error code does not establish the cause. Do not retry or clean up automatically.";

pub(super) fn project(lifecycle: &Lifecycle, mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("hostOutcomeSummary".into(), json!(summary(lifecycle)));
        object.insert("latestObservedStage".into(), json!(latest(lifecycle)));
        object.insert("currentWalletState".into(), json!("notObserved"));
    }
    value
}

fn summary(lifecycle: &Lifecycle) -> String {
    let readiness = match (
        &lifecycle.wallet_readiness.status,
        lifecycle.wallet_readiness.unlock_required,
    ) {
        (Progress::NotStarted, _) => READINESS[0],
        (Progress::Started, Some(true)) => READINESS[2],
        (Progress::Started, _) => READINESS[1],
        (Progress::Succeeded, Some(true)) => READINESS[4],
        (Progress::Succeeded, _) => READINESS[3],
    };
    let mut text = format!(
        "{readiness} {} {}",
        review(&lifecycle.scope_review, &SCOPE),
        progress(&lifecycle.source_preflight, &SOURCE)
    );
    for step in &lifecycle.steps {
        let _ = write!(text, " Step {}:", step.step_index + 1);
        for fact in step_facts(step) {
            text.push(' ');
            text.push_str(fact);
        }
        for fact in authorization::event_facts(step).into_iter().flatten() {
            text.push(' ');
            text.push_str(fact);
        }
        if let Some(reason) = step.request_cancellation {
            text.push(' ');
            text.push_str(authorization::cancellation_fact(reason));
        }
    }
    if let Some(reason) = lifecycle.request_cancellation {
        text.push(' ');
        text.push_str(authorization::cancellation_fact(reason));
    }
    text.push(' ');
    text.push_str(END);
    text
}

fn step_facts(step: &StepLifecycle) -> [&'static str; 5] {
    [
        review(&step.exact_review, &EXACT),
        match step.authorization {
            Authorization::NotRequested => AUTH[0],
            Authorization::Requested if step.authorization_events.is_empty() => AUTH[1],
            Authorization::Requested => {
                "Fresh signing authentication was requested; no successful authentication was recorded."
            }
            Authorization::Authenticated => AUTH[2],
        },
        match step.signing {
            Signing::NotStarted => SIGN[0],
            Signing::StartedOutcomeUnknown => SIGN[1],
            Signing::SignatureReturned => SIGN[2],
        },
        progress(&step.presend_refresh, &PRESEND),
        match step.submission {
            Submission::NotAttempted => SUBMISSION[0],
            Submission::MayHaveBeenSent => SUBMISSION[1],
            Submission::Acknowledged => SUBMISSION[2],
        },
    ]
}

fn review(value: &Review, choices: &'static [&str; 5]) -> &'static str {
    choices[match value {
        Review::NotRequested => 0,
        Review::Requested => 1,
        Review::Approved => 2,
        Review::Denied => 3,
        Review::Dismissed => 4,
    }]
}
fn progress(value: &Progress, choices: &'static [&str; 3]) -> &'static str {
    choices[match value {
        Progress::NotStarted => 0,
        Progress::Started => 1,
        Progress::Succeeded => 2,
    }]
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StageObservation {
    stage: Stage,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_index: Option<usize>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum Stage {
    NotStarted,
    WalletReadinessRequested,
    WalletReadinessSucceeded,
    ScopeReviewRequested,
    ScopeReviewApproved,
    ScopeReviewDenied,
    ScopeReviewDismissed,
    SourcePreflightStarted,
    SourcePreflightSucceeded,
    StepRecorded,
    ExactReviewRequested,
    ExactReviewApproved,
    ExactReviewDenied,
    ExactReviewDismissed,
    SigningAuthenticationRequested,
    SigningAuthenticationSucceeded,
    PasswordRequested,
    PasswordPresented,
    PasswordAccepted,
    PasswordCancelButton,
    PasswordHostAbort,
    PasswordOtherDismissal,
    AuthorizationJobInvalidated,
    SigningAuthenticationStarted,
    SigningAuthenticationFailed,
    SigningStartedOutcomeUnknown,
    SignatureReturned,
    PresendRefreshStarted,
    PresendRefreshSucceeded,
    SubmissionMayHaveBeenSent,
    SubmissionAcknowledged,
}

fn latest(lifecycle: &Lifecycle) -> StageObservation {
    if let Some(step) = lifecycle.steps.iter().max_by_key(|step| step.step_index) {
        return StageObservation {
            stage: step_stage(step),
            step_index: Some(step.step_index),
        };
    }
    let stage = match lifecycle.source_preflight {
        Progress::Succeeded => Stage::SourcePreflightSucceeded,
        Progress::Started => Stage::SourcePreflightStarted,
        Progress::NotStarted => match lifecycle.scope_review {
            Review::Approved => Stage::ScopeReviewApproved,
            Review::Denied => Stage::ScopeReviewDenied,
            Review::Dismissed => Stage::ScopeReviewDismissed,
            Review::Requested => Stage::ScopeReviewRequested,
            Review::NotRequested => match lifecycle.wallet_readiness.status {
                Progress::Succeeded => Stage::WalletReadinessSucceeded,
                Progress::Started => Stage::WalletReadinessRequested,
                Progress::NotStarted => Stage::NotStarted,
            },
        },
    };
    StageObservation {
        stage,
        step_index: None,
    }
}

fn step_stage(step: &StepLifecycle) -> Stage {
    match step.submission {
        Submission::Acknowledged => return Stage::SubmissionAcknowledged,
        Submission::MayHaveBeenSent => return Stage::SubmissionMayHaveBeenSent,
        Submission::NotAttempted => {}
    }
    match step.presend_refresh {
        Progress::Succeeded => return Stage::PresendRefreshSucceeded,
        Progress::Started => return Stage::PresendRefreshStarted,
        Progress::NotStarted => {}
    }
    match step.signing {
        Signing::SignatureReturned => return Stage::SignatureReturned,
        Signing::StartedOutcomeUnknown => return Stage::SigningStartedOutcomeUnknown,
        Signing::NotStarted => {}
    }
    if let Some(event) = step.authorization_events.last() {
        return authorization_stage(*event);
    }
    match step.authorization {
        Authorization::Authenticated => return Stage::SigningAuthenticationSucceeded,
        Authorization::Requested => return Stage::SigningAuthenticationRequested,
        Authorization::NotRequested => {}
    }
    match step.exact_review {
        Review::Approved => Stage::ExactReviewApproved,
        Review::Denied => Stage::ExactReviewDenied,
        Review::Dismissed => Stage::ExactReviewDismissed,
        Review::Requested => Stage::ExactReviewRequested,
        Review::NotRequested => Stage::StepRecorded,
    }
}

fn authorization_stage(event: WalletAuthorizationEvent) -> Stage {
    match event {
        WalletAuthorizationEvent::PasswordRequested { .. } => Stage::PasswordRequested,
        WalletAuthorizationEvent::PasswordPresented { .. } => Stage::PasswordPresented,
        WalletAuthorizationEvent::PasswordDecision { decision, .. } => match decision {
            WalletPasswordDecision::Accepted => Stage::PasswordAccepted,
            WalletPasswordDecision::CancelButton => Stage::PasswordCancelButton,
            WalletPasswordDecision::HostAbort => Stage::PasswordHostAbort,
            WalletPasswordDecision::OtherDismissal => Stage::PasswordOtherDismissal,
        },
        WalletAuthorizationEvent::JobInvalidated { .. } => Stage::AuthorizationJobInvalidated,
        WalletAuthorizationEvent::AuthenticationStarted => Stage::SigningAuthenticationStarted,
        WalletAuthorizationEvent::AuthenticationSucceeded => Stage::SigningAuthenticationSucceeded,
        WalletAuthorizationEvent::AuthenticationFailed { .. } => Stage::SigningAuthenticationFailed,
    }
}

/// Charge every finite sentence variant plus the error-message JSON quoting layer.
pub(super) fn maximum_projection_bytes(step_count: usize) -> usize {
    let longest = |choices: &[&str]| choices.iter().map(|value| value.len()).max().unwrap_or(0);
    let per_step = " Step 16:".len()
        + 5
        + longest(&EXACT)
        + longest(&AUTH)
        + longest(&SIGN)
        + longest(&PRESEND)
        + longest(&SUBMISSION)
        + 3
        + 2 * authorization::maximum_fact_bytes()
        + authorization::maximum_cancellation_bytes();
    let maximum = longest(&READINESS)
        + longest(&SCOPE)
        + longest(&SOURCE)
        + 3
        + step_count * per_step
        + END.len()
        + 1
        + authorization::maximum_cancellation_bytes();
    let projection = json!({"hostOutcomeSummary":"x".repeat(maximum),
        "latestObservedStage":{"stage":"x".repeat(64),"stepIndex":15},
        "currentWalletState":"notObserved"});
    json!({"message":projection.to_string()}).to_string().len()
}

#[cfg(test)]
mod tests;
