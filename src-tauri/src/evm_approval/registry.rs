//! Process-local immutable candidates and single-use native review identities.
use chain_evm::{NativeTransferCandidate, TokenTransferCandidate};
use ipc_types::{IpcError, IpcErrorCode, TransactionProposalId};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use transaction_policy::MAX_APPROVAL_LIFETIME_SECONDS;
const MAX_PENDING_REVIEWS: usize = 32;
pub(super) const STALE_SIMULATION_AFTER: Duration = Duration::from_mins(2);
const APPROVAL_LIFETIME_SECONDS: u64 = 2 * 60;

/// Trusted request origin selected by host routing, never by request prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApprovalOrigin {
    User,
    Chat {
        conversation: [u8; 32],
        call: [u8; 32],
    },
}

impl ApprovalOrigin {
    pub(super) fn display_name(self) -> String {
        match self {
            Self::User => "User · transfer preparation screen".to_owned(),
            Self::Chat { conversation, call } => format!(
                "Chat agent · conversation {} · call {}",
                hex_identity(&conversation),
                hex_identity(&call)
            ),
        }
    }
}

pub(crate) enum ApprovalCandidate {
    Native(NativeTransferCandidate),
    Token(TokenTransferCandidate),
}

pub(super) struct PendingReview {
    pub(super) candidate: ApprovalCandidate,
    pub(super) origin: ApprovalOrigin,
    pub(super) prepared_at: Instant,
    pub(super) session_id: String,
    pub(super) expires_at_unix: u64,
    pub(super) wallet_epoch: u64,
}

#[derive(Default)]
pub(super) struct ReviewRegistry {
    pub(super) pending: HashMap<String, PendingReview>,
    pub(super) active: bool,
}

/// Process-local owner of prepared candidates awaiting native review.
///
/// Only this registry can resolve a frontend-visible opaque review reference
/// into the exact candidate retained by the host.
#[derive(Default)]
pub(crate) struct EvmApprovalControl {
    pub(super) registry: Mutex<ReviewRegistry>,
}

impl EvmApprovalControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(
        &self,
        candidate: ApprovalCandidate,
        origin: ApprovalOrigin,
        wallet_epoch: u64,
    ) -> Result<TransactionProposalId, IpcError> {
        let now_unix = unix_now()?;
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.pending.len() >= MAX_PENDING_REVIEWS {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "Too many transactions are waiting for trusted review".to_owned(),
                retryable: true,
            });
        }
        let review_id = TransactionProposalId(uuid::Uuid::now_v7().to_string());
        let session_id = {
            use sha2::{Digest as _, Sha256};
            hex_identity(
                &Sha256::digest(format!(
                    "{}:{}",
                    uuid::Uuid::now_v7(),
                    origin.display_name()
                ))
                .into(),
            )
        };
        registry.pending.insert(
            review_id.0.clone(),
            PendingReview {
                candidate,
                origin,
                prepared_at: Instant::now(),
                session_id,
                expires_at_unix: now_unix + APPROVAL_LIFETIME_SECONDS,
                wallet_epoch,
            },
        );
        Ok(review_id)
    }

    pub(super) fn begin(
        &self,
        review_id: &TransactionProposalId,
    ) -> Result<PendingReview, IpcError> {
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.active {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "Another transaction is already open for trusted review".to_owned(),
                retryable: true,
            });
        }
        let pending = registry
            .pending
            .remove(&review_id.0)
            .ok_or_else(|| IpcError {
                code: IpcErrorCode::InvalidRequest,
                message: "The transaction review reference is unknown or has already been used"
                    .to_owned(),
                retryable: false,
            })?;
        registry.active = true;
        Ok(pending)
    }

    pub(super) fn begin_reconciliation(&self) -> Result<ReconciliationLease<'_>, IpcError> {
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.active {
            return Err(IpcError::invalid_request(
                "Finish the active native transaction review before reconciling another transfer",
            ));
        }
        registry.active = true;
        Ok(ReconciliationLease(self))
    }
    pub(super) fn discard(&self, id: &TransactionProposalId) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.pending.remove(&id.0);
        }
    }
    pub(super) fn finish(&self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.active = false;
        }
    }
}

impl ApprovalCandidate {
    pub(crate) fn network(&self) -> Result<chain_evm::EvmNetwork, IpcError> {
        let chain_id = match self {
            Self::Native(candidate) => candidate.chain_id(),
            Self::Token(candidate) => candidate.chain_id(),
        };
        chain_evm::EvmNetwork::ALL
            .into_iter()
            .find(|network| network.descriptor().chain_id() == chain_id)
            .ok_or_else(IpcError::internal)
    }

    pub(super) fn caip2_id(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.caip2_id(),
            Self::Token(candidate) => candidate.caip2_id(),
        }
    }

    pub(super) fn sender(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.sender(),
            Self::Token(candidate) => candidate.sender(),
        }
    }

    pub(super) fn nonce(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.nonce(),
            Self::Token(candidate) => candidate.nonce(),
        }
    }

    pub(super) fn candidate_digest(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.simulation().candidate_digest(),
            Self::Token(candidate) => candidate.simulation().candidate_digest(),
        }
    }
}

pub(super) fn unix_now() -> Result<u64, IpcError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| IpcError::internal())
}

fn hex_identity(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

const _: () = assert!(APPROVAL_LIFETIME_SECONDS <= MAX_APPROVAL_LIFETIME_SECONDS);

pub(super) struct ReconciliationLease<'a>(&'a EvmApprovalControl);
impl Drop for ReconciliationLease<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}
