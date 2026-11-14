//! Closed monitoring diagnostics contain no provider text, URL or credential.
use crate::{BroadcastError, RpcChainIdError, rpc::RpcError};
use serde::Serialize;
use std::{error::Error, fmt, time::Duration};

/// The fixed public observation operation that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorStage {
    /// Persisted public sender and nonce admission.
    InputValidation,
    /// Read-only RPC client construction.
    ClientSetup,
    /// RPC chain identity acquisition.
    ChainIdRpc,
    /// Exact selected chain identity validation.
    ChainIdValidation,
    /// Receipt acquisition; a null result is normal pending state.
    ReceiptRpc,
    /// Strict receipt hash, inclusion coordinates and status validation.
    ReceiptValidation,
    /// Public transaction lookup after a null receipt.
    TransactionRpc,
    /// Exact returned transaction hash validation.
    TransactionValidation,
    /// Public sender nonce acquisition after both lookups are null.
    NonceRpc,
    /// Canonical nonce quantity validation.
    NonceValidation,
}

/// A bounded monitoring failure, never evidence that a transaction was unsent.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MonitorCause {
    /// Persisted sender is malformed.
    InvalidSender,
    /// Persisted submitted nonce is malformed.
    InvalidSubmittedNonce,
    /// A returned transaction hash is malformed.
    InvalidTransactionHash,
    /// The RPC transport is unavailable; no provider text is retained.
    Transport,
    /// The HTTP library positively reported a request or body timeout.
    TimedOut,
    /// Native HTTP response status only.
    HttpStatus {
        /// Bounded native status code.
        status: u16,
    },
    /// The existing response size limit was exceeded.
    ResponseTooLarge,
    /// Native JSON-RPC numeric error code only.
    RpcRejected {
        /// Bounded native error code, without its message or data.
        code: i64,
    },
    /// Envelope or typed response fields are malformed or ambiguous.
    MalformedResponse,
    /// Returned public hash differs from the requested transaction hash.
    TransactionHashMismatch,
    /// Chain ID is not a canonical supported quantity.
    ChainIdMalformed,
    /// Endpoint chain differs from the host-selected chain.
    ChainIdMismatch {
        /// Host-selected public chain ID.
        expected: u64,
        /// Public chain ID returned by the endpoint.
        actual: u64,
    },
    /// The actual caller cancellation flag was observed.
    Cancelled,
    /// The remaining whole-observation allowance was exhausted.
    BudgetElapsed,
}

/// One data-only observation failure. Construction confers no retry authority,
/// signing capability, submission access or canonical inclusion evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorFailure {
    stage: MonitorStage,
    cause: MonitorCause,
    elapsed_ms: u64,
}
impl MonitorFailure {
    pub(super) fn new(stage: MonitorStage, cause: MonitorCause, elapsed: Duration) -> Self {
        Self {
            stage,
            cause,
            elapsed_ms: u64::from(u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX)),
        }
    }
    /// Fixed operation whose observation failed.
    #[must_use]
    pub const fn stage(self) -> MonitorStage {
        self.stage
    }
    /// Closed cause, never provider prose.
    #[must_use]
    pub const fn cause(self) -> MonitorCause {
        self.cause
    }
    /// Elapsed time inside this one observation attempt, capped at `u32::MAX` ms.
    #[must_use]
    pub const fn elapsed_ms(self) -> u64 {
        self.elapsed_ms
    }
    /// Whether another read-only observation may be attempted within an
    /// independently retained receipt budget. This does not prove transience
    /// and never permits signing, submission, replacement or rebroadcast.
    #[must_use]
    pub const fn retryable_read(self) -> bool {
        matches!(
            self.cause,
            MonitorCause::Transport
                | MonitorCause::TimedOut
                | MonitorCause::HttpStatus {
                    status: 408 | 429 | 500..=599
                }
        )
    }
    pub(super) const fn legacy_error(self) -> BroadcastError {
        match self.cause {
            MonitorCause::InvalidSender => BroadcastError::InvalidSender,
            MonitorCause::InvalidSubmittedNonce => BroadcastError::InvalidSubmittedNonce,
            MonitorCause::InvalidTransactionHash => BroadcastError::InvalidTransactionHash,
            MonitorCause::Transport
            | MonitorCause::TimedOut
            | MonitorCause::Cancelled
            | MonitorCause::BudgetElapsed => BroadcastError::Transport,
            MonitorCause::HttpStatus { status } => BroadcastError::HttpStatus(status),
            MonitorCause::ResponseTooLarge => BroadcastError::ResponseTooLarge,
            MonitorCause::RpcRejected { code } => BroadcastError::RpcRejected(code),
            MonitorCause::MalformedResponse => BroadcastError::MalformedRpcResponse,
            MonitorCause::TransactionHashMismatch => BroadcastError::TransactionHashMismatch,
            MonitorCause::ChainIdMalformed => {
                BroadcastError::ChainId(RpcChainIdError::MalformedResponse)
            }
            MonitorCause::ChainIdMismatch { expected, actual } => {
                BroadcastError::ChainId(RpcChainIdError::Mismatch { expected, actual })
            }
        }
    }
}
impl fmt::Display for MonitorFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every formatted value below is a closed enum or bounded integer.
        write!(
            f,
            "Transaction observation unavailable: stage={:?} cause={:?} elapsedMs={}",
            self.stage, self.cause, self.elapsed_ms
        )
    }
}
impl Error for MonitorFailure {}

impl From<RpcError> for MonitorCause {
    fn from(error: RpcError) -> Self {
        match error {
            RpcError::Transport => Self::Transport,
            RpcError::TimedOut => Self::TimedOut,
            RpcError::HttpStatus(status) => Self::HttpStatus { status },
            RpcError::ResponseTooLarge => Self::ResponseTooLarge,
            RpcError::MalformedResponse => Self::MalformedResponse,
            RpcError::Rejected(code) => Self::RpcRejected { code },
        }
    }
}
impl From<RpcChainIdError> for MonitorCause {
    fn from(error: RpcChainIdError) -> Self {
        match error {
            RpcChainIdError::MalformedResponse => Self::ChainIdMalformed,
            RpcChainIdError::Mismatch { expected, actual } => {
                Self::ChainIdMismatch { expected, actual }
            }
        }
    }
}
impl From<BroadcastError> for MonitorCause {
    fn from(error: BroadcastError) -> Self {
        match error {
            BroadcastError::TransactionHashMismatch => Self::TransactionHashMismatch,
            BroadcastError::InvalidTransactionHash => Self::InvalidTransactionHash,
            BroadcastError::ChainId(error) => Self::from(error),
            BroadcastError::InvalidSender => Self::InvalidSender,
            BroadcastError::InvalidSubmittedNonce => Self::InvalidSubmittedNonce,
            BroadcastError::Transport => Self::Transport,
            BroadcastError::HttpStatus(status) => Self::HttpStatus { status },
            BroadcastError::ResponseTooLarge => Self::ResponseTooLarge,
            BroadcastError::RpcRejected(code) => Self::RpcRejected { code },
            BroadcastError::SubmissionCancelled
            | BroadcastError::InvalidSignedTransaction
            | BroadcastError::MalformedRpcResponse => Self::MalformedResponse,
        }
    }
}
