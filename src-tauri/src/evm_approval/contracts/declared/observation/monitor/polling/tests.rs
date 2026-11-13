//! Scripted public observations exercise the production loop, not chain proofs.
use super::*;
use crate::evm_approval::contracts::declared::lifecycle::{Lifecycle, StepLifecycle};
use ipc_types::IpcError;
use serde_json::{Value, json};
use std::collections::VecDeque;

const HASH: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone, Copy)]
enum Reply {
    Pending,
    RetryEligible,
    Malformed,
    Confirmed,
}
struct Report {
    hash: &'static str,
    lifecycle: TransactionLifecycle,
}
/// The fixture, like the production polling driver, exposes no send operation.
/// Its only transaction identity is the already acknowledged public hash.
struct Reader<'a> {
    progress: &'a mut StepLifecycle,
    replies: VecDeque<Reply>,
    reads: Vec<&'static str>,
    elapsed: Duration,
    read_cost: Duration,
    advance_on_pause: bool,
    cancel_after: Option<usize>,
    cancelled: bool,
}
impl ReadPolling for Reader<'_> {
    type Observation = Report;
    type Error = IpcError;
    fn elapsed(&self) -> Duration {
        self.elapsed
    }
    fn check(&mut self) -> Result<(), IpcError> {
        if self.cancelled {
            Err(self.stop(ObservationStop::ContextUnavailable))
        } else {
            Ok(())
        }
    }
    fn attempted(&mut self) -> Result<(), IpcError> {
        self.progress.receipt_attempted()
    }
    async fn read(&mut self) -> Result<ReadResult<Report>, IpcError> {
        self.reads.push(HASH);
        self.elapsed = self.elapsed.saturating_add(self.read_cost);
        self.cancelled = self.cancel_after == Some(self.reads.len());
        match self.replies.pop_front().unwrap_or(Reply::Pending) {
            Reply::RetryEligible => Ok(ReadResult::RetryEligible),
            Reply::Malformed => Err(self.stop(ObservationStop::MonitorRejected)),
            reply => Ok(ReadResult::Observed(Report {
                hash: HASH,
                lifecycle: match reply {
                    Reply::Confirmed => TransactionLifecycle::Confirmed,
                    _ => TransactionLifecycle::Pending,
                },
            })),
        }
    }
    fn reported(&mut self, value: &Report) -> TransactionLifecycle {
        self.progress.receipt_reported(value.lifecycle);
        value.lifecycle
    }
    async fn pause(&mut self, delay: Duration) {
        if self.advance_on_pause {
            self.elapsed = self.elapsed.saturating_add(delay);
        }
    }
    fn stop(&mut self, reason: ObservationStop) -> IpcError {
        self.progress.observation_stopped(reason);
        IpcError::invalid_request("Scripted public observation stopped")
    }
}
fn reader<'a>(lifecycle: &'a mut Lifecycle, replies: &[Reply]) -> Result<Reader<'a>, &'static str> {
    let progress = lifecycle.step(0).ok_or("step")?;
    progress.record_transaction_hash(HASH).map_err(|_| "hash")?;
    progress.submission_acknowledged();
    progress.receipt_wait_started();
    Ok(Reader {
        progress,
        replies: replies.iter().copied().collect(),
        reads: Vec::new(),
        elapsed: Duration::ZERO,
        read_cost: Duration::ZERO,
        advance_on_pause: true,
        cancel_after: None,
        cancelled: false,
    })
}
fn post(lifecycle: &Lifecycle) -> Value {
    lifecycle.facts()["observations"]["steps"][0]["postSubmission"].clone()
}
fn retained(lifecycle: &Lifecycle) {
    let step = &lifecycle.facts()["observations"]["steps"][0];
    assert_eq!(step["transactionHash"], HASH);
    assert_eq!(step["submission"], "acknowledged");
    assert_eq!(step["postSubmission"]["inclusionProof"], "notStarted");
    assert_eq!(step["postSubmission"]["completionProof"], "notStarted");
}

#[tokio::test]
async fn pending_then_retryable_read_failure_then_confirmed_never_reenters_send()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let mut reader = reader(
        &mut lifecycle,
        &[Reply::Pending, Reply::RetryEligible, Reply::Confirmed],
    )?;
    reader.read_cost = Duration::from_secs(12);
    let report = until_terminal(&mut reader)
        .await
        .map_err(|error| error.message)?;
    assert_eq!(report.hash, HASH);
    assert_eq!(report.lifecycle, TransactionLifecycle::Confirmed);
    assert_eq!(reader.reads, [HASH; 3]);
    assert_eq!(reader.elapsed, Duration::from_secs(38));
    retained(&lifecycle);
    assert_eq!(post(&lifecycle)["monitorAttempts"], 3);
    assert_eq!(post(&lifecycle)["receipt"], "successReported");
    assert_eq!(post(&lifecycle)["stopReason"], Value::Null);
    assert!(!lifecycle.submitted_verification_unavailable());
    Ok(())
}

#[tokio::test]
async fn malformed_read_stops_without_consuming_a_later_confirmed_reply()
-> Result<(), Box<dyn std::error::Error>> {
    let mut lifecycle = Lifecycle::default();
    let mut reader = reader(&mut lifecycle, &[Reply::Malformed, Reply::Confirmed])?;
    assert!(until_terminal(&mut reader).await.is_err());
    assert_eq!(reader.reads, [HASH]);
    assert_eq!(reader.replies.len(), 1);
    retained(&lifecycle);
    assert_eq!(post(&lifecycle)["stopReason"], "monitorRejected");
    let partial = lifecycle
        .project_verification_unavailable(&IpcError::internal(), 0, &json!({}))
        .ok_or("known submission lost")?;
    assert_eq!(partial["state"], "submittedVerificationUnavailable");
    assert_eq!(partial["completedSteps"], 0);
    assert_eq!(partial["retryAutomatically"], false);
    Ok(())
}

#[tokio::test]
async fn cancellation_stops_before_read_and_after_return_preserving_positive_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    for before in [true, false] {
        let mut lifecycle = Lifecycle::default();
        let mut reader = reader(&mut lifecycle, &[Reply::Confirmed])?;
        reader.cancelled = before;
        reader.cancel_after = Some(1);
        assert!(until_terminal(&mut reader).await.is_err());
        assert_eq!(reader.reads.len(), usize::from(!before));
        retained(&lifecycle);
        assert_eq!(post(&lifecycle)["stopReason"], "contextUnavailable");
        assert_eq!(
            post(&lifecycle)["receipt"],
            if before {
                "unavailable"
            } else {
                "successReported"
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn late_confirmed_read_retains_the_report_without_exceeding_total_allowance()
-> Result<(), Box<dyn std::error::Error>> {
    for (elapsed, read_cost) in [(0, 120), (119, 2)] {
        let mut lifecycle = Lifecycle::default();
        let mut reader = reader(&mut lifecycle, &[Reply::Confirmed])?;
        reader.elapsed = Duration::from_secs(elapsed);
        reader.read_cost = Duration::from_secs(read_cost);
        assert!(until_terminal(&mut reader).await.is_err());
        assert_eq!(reader.reads, [HASH]);
        retained(&lifecycle);
        assert_eq!(post(&lifecycle)["receipt"], "successReported");
        assert_eq!(post(&lifecycle)["stopReason"], "budgetElapsed");
        let partial = lifecycle
            .project_verification_unavailable(&IpcError::internal(), 0, &Value::Null)
            .ok_or("known submission lost")?;
        assert_eq!(partial["completedSteps"], 0);
        assert_eq!(partial["state"], "submittedVerificationUnavailable");
    }
    Ok(())
}

#[tokio::test]
async fn pending_and_retryable_failures_share_deadline_and_attempt_ceiling()
-> Result<(), Box<dyn std::error::Error>> {
    for attempts_exhausted in [false, true] {
        let mut lifecycle = Lifecycle::default();
        let mut reader = reader(&mut lifecycle, &[Reply::RetryEligible])?;
        reader.advance_on_pause = !attempts_exhausted;
        reader.read_cost = if attempts_exhausted {
            Duration::ZERO
        } else {
            Duration::from_mins(1)
        };
        assert!(until_terminal(&mut reader).await.is_err());
        assert_eq!(reader.reads.len(), if attempts_exhausted { 120 } else { 2 });
        retained(&lifecycle);
        assert_eq!(
            post(&lifecycle)["stopReason"],
            if attempts_exhausted {
                "attemptsExhausted"
            } else {
                "budgetElapsed"
            }
        );
        assert!(lifecycle.submitted_verification_unavailable());
    }
    Ok(())
}
