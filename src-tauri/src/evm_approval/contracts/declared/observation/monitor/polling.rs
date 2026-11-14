//! The polling state machine has read-only operations and no dispatch seam.
use super::{MAX_MONITOR_ATTEMPTS, POLL_DELAY, remaining};
use crate::evm_approval::contracts::declared::lifecycle::ObservationStop;
use chain_evm::TransactionLifecycle;
use std::time::Duration;

pub(super) enum ReadResult<T> {
    Observed(T),
    RetryEligible,
}

/// Production owns context checks, closed RPC classification and the journal.
/// Tests can exercise scheduling without constructing chain proofs or wallets.
pub(super) trait ReadPolling {
    type Observation;
    type Error;
    fn elapsed(&self) -> Duration;
    fn check(&mut self) -> Result<(), Self::Error>;
    fn attempted(&mut self) -> Result<(), Self::Error>;
    async fn read(&mut self) -> Result<ReadResult<Self::Observation>, Self::Error>;
    fn reported(&mut self, value: &Self::Observation) -> TransactionLifecycle;
    async fn pause(&mut self, delay: Duration);
    fn stop(&mut self, reason: ObservationStop) -> Self::Error;
}

pub(super) async fn until_terminal<R: ReadPolling>(
    reader: &mut R,
) -> Result<R::Observation, R::Error> {
    for _ in 0..MAX_MONITOR_ATTEMPTS {
        if remaining(reader.elapsed()).is_none() {
            return Err(reader.stop(ObservationStop::BudgetElapsed));
        }
        reader.check()?;
        reader.attempted()?;
        let terminal = match reader.read().await? {
            ReadResult::Observed(value) => {
                // A returned observation survives a subsequent context failure.
                let lifecycle = reader.reported(&value);
                (lifecycle != TransactionLifecycle::Pending).then_some(value)
            }
            ReadResult::RetryEligible => None,
        };
        reader.check()?;
        // The RPC adapter enforces its own shrinking allowance. This check also
        // charges queue/host scheduling after that adapter returned a result.
        if remaining(reader.elapsed()).is_none() {
            return Err(reader.stop(ObservationStop::BudgetElapsed));
        }
        if let Some(value) = terminal {
            return Ok(value);
        }
        if let Some(delay) = remaining(reader.elapsed()).map(|left| left.min(POLL_DELAY)) {
            reader.pause(delay).await;
        }
    }
    Err(reader.stop(ObservationStop::AttemptsExhausted))
}

#[cfg(test)]
mod tests;
