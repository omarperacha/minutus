//! Host-owned transaction review registry and native approval boundary.

pub(crate) mod presentation;

#[cfg(target_os = "macos")]
mod native;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::{
    AppCore, ApprovalAuditDecision, ApprovalFinalizationAuditOutcome,
    OwnerAuthorizationAuditOutcome, StoredEvmTransaction, StoredEvmTransactionState,
    TransactionAuditEventKind, TransactionAuditFailureCode, TransactionAuditResult,
};
use chain_evm::{
    NativeTransferCandidate, TokenTransferCandidate, assemble_signed_eip1559_transaction,
    broadcast_signed_transaction_with_admission,
};
use ipc_types::{
    EvmApprovalDecision, EvmApprovalReviewOutcome, IpcError, IpcErrorCode, TransactionProposalId,
};
use tauri::State;
use transaction_policy::{ApprovalCommitment, MAX_APPROVAL_LIFETIME_SECONDS};

use self::presentation::ApprovalDisplay;
use crate::{
    error_mapping::{app_core_error, current_timestamp},
    evm_rpc_settings::resolved,
    wallet_lifecycle::WalletControl,
};

const MAX_PENDING_REVIEWS: usize = 32;
const STALE_SIMULATION_AFTER: Duration = Duration::from_mins(2);
const APPROVAL_LIFETIME_SECONDS: u64 = 2 * 60;

/// Trusted request origin selected by host routing, never by request prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApprovalOrigin {
    User,
}

impl ApprovalOrigin {
    const fn display_name(self) -> &'static str {
        match self {
            Self::User => "User · transfer preparation screen",
        }
    }
}

pub(crate) enum ApprovalCandidate {
    Native(NativeTransferCandidate),
    Token(TokenTransferCandidate),
}

struct PendingReview {
    candidate: ApprovalCandidate,
    origin: ApprovalOrigin,
    prepared_at: Instant,
    session_id: String,
    expires_at_unix: u64,
}

#[derive(Default)]
struct ReviewRegistry {
    pending: HashMap<String, PendingReview>,
    active: bool,
}

/// Process-local owner of prepared candidates awaiting native review.
///
/// Only this registry can resolve a frontend-visible opaque review reference
/// into the exact candidate retained by the host.
#[derive(Default)]
pub(crate) struct EvmApprovalControl {
    registry: Mutex<ReviewRegistry>,
}

impl EvmApprovalControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(
        &self,
        candidate: ApprovalCandidate,
        origin: ApprovalOrigin,
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
        let session_id = uuid::Uuid::now_v7().to_string();
        registry.pending.insert(
            review_id.0.clone(),
            PendingReview {
                candidate,
                origin,
                prepared_at: Instant::now(),
                session_id,
                expires_at_unix: now_unix + APPROVAL_LIFETIME_SECONDS,
            },
        );
        Ok(review_id)
    }

    fn begin(&self, review_id: &TransactionProposalId) -> Result<PendingReview, IpcError> {
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

    fn finish(&self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.active = false;
        }
    }
}

#[tauri::command]
pub(crate) async fn review_evm_transaction(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, EvmApprovalControl>,
    wallet: State<'_, WalletControl>,
    review_id: TransactionProposalId,
) -> Result<EvmApprovalReviewOutcome, IpcError> {
    let pending = control.begin(&review_id)?;
    let _active_review = ActiveReview(&control);
    let age = pending.prepared_at.elapsed();
    let now_unix = match unix_now() {
        Ok(now_unix) => now_unix,
        Err(error) => {
            control.finish();
            return Err(error);
        }
    };
    let display = ApprovalDisplay::from_candidate(
        &pending.candidate,
        pending.origin,
        age >= STALE_SIMULATION_AFTER,
        &pending.session_id,
        pending.expires_at_unix,
        now_unix,
    );
    let prepared = match display {
        Ok(prepared) => prepared,
        Err(error) => {
            control.finish();
            return Err(error);
        }
    };

    let presentation::PreparedApproval {
        display,
        payload,
        evidence,
        audit_warning_codes,
    } = prepared;
    if let Ok(event) = evidence.audit_event(
        &review_id.0,
        pending.origin,
        TransactionAuditEventKind::ApprovalPresented {
            warning_codes: audit_warning_codes,
        },
    ) {
        let _ = core.record_transaction_audit(event);
    }
    let decision = match show_native_review(&app, display).await {
        Ok(decision) => decision,
        Err(error) => {
            control.finish();
            return Err(error);
        }
    };
    let audit_decision = match decision {
        EvmApprovalDecision::Approved => ApprovalAuditDecision::Approved,
        EvmApprovalDecision::Denied => ApprovalAuditDecision::Denied,
        EvmApprovalDecision::Dismissed => ApprovalAuditDecision::Dismissed,
    };
    if let Ok(event) = evidence.audit_event(
        &review_id.0,
        pending.origin,
        TransactionAuditEventKind::ApprovalDecision {
            decision: audit_decision,
        },
    ) {
        let _ = core.record_transaction_audit(event);
    }
    let submission = if decision == EvmApprovalDecision::Approved {
        Some(
            approve_sign_and_broadcast(&core, &wallet, &review_id, &pending, &payload, &evidence)
                .await?,
        )
    } else {
        None
    };
    Ok(EvmApprovalReviewOutcome {
        review_id,
        decision,
        transaction_hash: submission
            .as_ref()
            .map(|value| value.transaction_hash.clone()),
        explorer_url: submission.map(|value| value.explorer_url),
    })
}

struct ApprovedSubmission {
    transaction_hash: String,
    explorer_url: String,
}

async fn approve_sign_and_broadcast(
    core: &AppCore,
    wallet: &WalletControl,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    payload: &[u8],
    evidence: &presentation::ApprovalEvidence,
) -> Result<ApprovedSubmission, IpcError> {
    let commitment = ApprovalCommitment::issue(evidence.binding_input(), unix_now()?)
        .map_err(commitment_error)?;
    let signature = sign_approved(core, wallet, review_id, pending, evidence, &commitment).await?;
    let signed = assemble_signed_eip1559_transaction(
        payload,
        signature.signature.r(),
        signature.signature.s(),
        signature.signature.y_parity(),
    )
    .map_err(|_| IpcError::internal())?;
    broadcast_approved(
        core,
        review_id,
        pending,
        evidence,
        signed,
        signature.submission,
    )
    .await
}

async fn sign_approved(
    core: &AppCore,
    wallet: &WalletControl,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    evidence: &presentation::ApprovalEvidence,
    commitment: &ApprovalCommitment,
) -> Result<crate::wallet_lifecycle::WalletSigned, IpcError> {
    let signature = wallet
        .sign_exact_transaction(core, commitment, evidence)
        .await;
    let outcome = match &signature {
        Ok(_) => OwnerAuthorizationAuditOutcome::Succeeded,
        Err(error) => authorization_audit_outcome(error),
    };
    record_audit(
        core,
        evidence,
        review_id,
        pending.origin,
        TransactionAuditEventKind::OwnerAuthorization { outcome },
    );
    record_audit(
        core,
        evidence,
        review_id,
        pending.origin,
        TransactionAuditEventKind::ApprovalFinalization {
            outcome: if signature.is_ok() {
                ApprovalFinalizationAuditOutcome::Consumed
            } else {
                ApprovalFinalizationAuditOutcome::Invalidated
            },
        },
    );
    record_audit(
        core,
        evidence,
        review_id,
        pending.origin,
        TransactionAuditEventKind::Signing {
            result: signature.as_ref().map_or_else(
                |_| TransactionAuditResult::Failed(TransactionAuditFailureCode::SigningFailed),
                |_| TransactionAuditResult::Succeeded,
            ),
        },
    );
    signature
}

async fn broadcast_approved(
    core: &AppCore,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    evidence: &presentation::ApprovalEvidence,
    signed: zeroize::Zeroizing<String>,
    submission: crate::wallet_lifecycle::WalletSubmission,
) -> Result<ApprovedSubmission, IpcError> {
    let network = pending.candidate.network()?;
    let endpoint = resolved(network)?;
    let (receipt, admitted) = tokio::task::spawn_blocking(move || {
        let mut admitted = false;
        let result =
            broadcast_signed_transaction_with_admission(network, &endpoint, signed, || {
                admitted = submission.begin().is_ok();
                admitted
            });
        (result, admitted)
    })
    .await
    .map_err(|_| IpcError::internal())?;
    let receipt = receipt.map_err(|error| broadcast_failure(error, admitted));
    let result = receipt.as_ref().map_or_else(
        |_| TransactionAuditResult::Failed(TransactionAuditFailureCode::BroadcastFailed),
        |_| TransactionAuditResult::Succeeded,
    );
    record_audit(
        core,
        evidence,
        review_id,
        pending.origin,
        TransactionAuditEventKind::Broadcast { result },
    );
    let receipt = receipt?;
    let timestamp = current_timestamp()?;
    let transaction_hash = receipt.transaction_hash().as_str().to_owned();
    core.save_evm_transaction(&StoredEvmTransaction {
        transaction_hash: transaction_hash.clone(),
        network: pending.candidate.caip2_id().to_owned(),
        sender: pending.candidate.sender().to_owned(),
        nonce: pending.candidate.nonce().to_owned(),
        candidate_digest: pending.candidate.candidate_digest().to_owned(),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: timestamp.clone(),
        updated_at: timestamp,
    })
    .map_err(app_core_error)?;
    Ok(ApprovedSubmission {
        transaction_hash,
        explorer_url: receipt.explorer_url().to_owned(),
    })
}

fn broadcast_failure(error: chain_evm::BroadcastError, admitted: bool) -> IpcError {
    IpcError {
        code: if matches!(error, chain_evm::BroadcastError::SubmissionCancelled) {
            IpcErrorCode::Cancelled
        } else {
            IpcErrorCode::Unsupported
        },
        message: if admitted {
            "Submission was not confirmed. The transaction may have been sent; check its status before creating another transfer.".to_owned()
        } else {
            error.to_string()
        },
        retryable: false,
    }
}
#[cfg(test)]
mod submission_outcome_tests;

struct ActiveReview<'a>(&'a EvmApprovalControl);

impl Drop for ActiveReview<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

impl ApprovalCandidate {
    fn network(&self) -> Result<chain_evm::EvmNetwork, IpcError> {
        let chain_id = match self {
            Self::Native(candidate) => candidate.chain_id(),
            Self::Token(candidate) => candidate.chain_id(),
        };
        chain_evm::EvmNetwork::ALL
            .into_iter()
            .find(|network| network.descriptor().chain_id() == chain_id)
            .ok_or_else(IpcError::internal)
    }

    fn caip2_id(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.caip2_id(),
            Self::Token(candidate) => candidate.caip2_id(),
        }
    }

    fn sender(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.sender(),
            Self::Token(candidate) => candidate.sender(),
        }
    }

    fn nonce(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.nonce(),
            Self::Token(candidate) => candidate.nonce(),
        }
    }

    fn candidate_digest(&self) -> &str {
        match self {
            Self::Native(candidate) => candidate.simulation().candidate_digest(),
            Self::Token(candidate) => candidate.simulation().candidate_digest(),
        }
    }
}

fn record_audit(
    core: &AppCore,
    evidence: &presentation::ApprovalEvidence,
    review_id: &TransactionProposalId,
    origin: ApprovalOrigin,
    event: TransactionAuditEventKind,
) {
    if let Ok(event) = evidence.audit_event(&review_id.0, origin, event) {
        let _ = core.record_transaction_audit(event);
    }
}

fn authorization_audit_outcome(error: &IpcError) -> OwnerAuthorizationAuditOutcome {
    if error.code == IpcErrorCode::Cancelled {
        OwnerAuthorizationAuditOutcome::Cancelled
    } else if error.message.contains("denied") {
        OwnerAuthorizationAuditOutcome::Denied
    } else {
        OwnerAuthorizationAuditOutcome::Unavailable
    }
}

fn unix_now() -> Result<u64, IpcError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| IpcError::internal())
}

fn commitment_error(_error: transaction_policy::ApprovalCommitmentError) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: "The exact transaction approval expired or changed before it could be bound"
            .to_owned(),
        retryable: false,
    }
}

const _: () = assert!(APPROVAL_LIFETIME_SECONDS <= MAX_APPROVAL_LIFETIME_SECONDS);

#[cfg(target_os = "macos")]
async fn show_native_review(
    app: &tauri::AppHandle,
    display: ApprovalDisplay,
) -> Result<EvmApprovalDecision, IpcError> {
    native::show(app, display).await
}

#[cfg(not(target_os = "macos"))]
async fn show_native_review(
    _app: &tauri::AppHandle,
    _display: ApprovalDisplay,
) -> Result<EvmApprovalDecision, IpcError> {
    Err(IpcError {
        code: IpcErrorCode::Unsupported,
        message: "Trusted transaction review is currently available only on macOS".to_owned(),
        retryable: false,
    })
}

#[cfg(test)]
mod tests;
