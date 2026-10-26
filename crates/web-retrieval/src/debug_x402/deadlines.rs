//! Host-owned response budgets for local x402 requests.

use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) struct ResponseDeadlines {
    pub(super) headers: Duration,
    pub(super) total: Duration,
}

impl ResponseDeadlines {
    pub(super) const fn for_request(has_payment: bool) -> Self {
        if has_payment {
            // A signed request can wait on facilitator verification, onchain
            // settlement and order completion before receiving any headers.
            // This is a transport budget, not an extension of payment validity.
            Self {
                headers: Duration::from_mins(1),
                total: Duration::from_secs(75),
            }
        } else {
            Self {
                headers: Duration::from_secs(5),
                total: Duration::from_secs(15),
            }
        }
    }
}
