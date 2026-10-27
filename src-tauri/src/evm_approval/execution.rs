//! Native review, fresh authentication and final network submission composition.
use super::{
    ApprovalOrigin, EvmApprovalControl,
    presentation::{self, ApprovalDisplay},
    registry::{PendingReview, STALE_SIMULATION_AFTER, unix_now},
};
use crate::{
    error_mapping::current_timestamp,
    evm_rpc_settings::resolved,
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::{
    AppCore, ApprovalAuditDecision, ApprovalFinalizationAuditOutcome,
    OwnerAuthorizationAuditOutcome, StoredEvmTransaction, StoredEvmTransactionState,
    TransactionAuditEventKind, TransactionAuditFailureCode, TransactionAuditResult,
};
use chain_evm::{assemble_signed_eip1559_transaction, broadcast_signed_transaction_with_admission};
use ipc_types::{
    EvmApprovalDecision, EvmApprovalReviewOutcome, IpcError, IpcErrorCode, TransactionProposalId,
};
use std::sync::Arc;
use tauri::State;
use transaction_policy::ApprovalCommitment;
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
    let cancelled = TransferCancellation::new(Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let reviewed = review_pending(&app, &core, &wallet, review_id, &pending, &cancelled).await?;
    if reviewed.submission_uncertain {
        return Err(IpcError {
            code: IpcErrorCode::Unsupported,
            message: format!(
                "Submission uncertain for {}. Check Wallet transaction status before another transfer.",
                reviewed
                    .outcome
                    .transaction_hash
                    .as_deref()
                    .unwrap_or("the known transaction")
            ),
            retryable: false,
        });
    }
    Ok(reviewed.outcome)
}

pub(crate) struct ReviewedTransfer {
    pub(crate) outcome: EvmApprovalReviewOutcome,
    pub(crate) submission_uncertain: bool,
}

/// A model call waits for exactly the same native decision/signing path as Wallet.
pub(crate) fn review_chat_transfer(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    control: &EvmApprovalControl,
    wallet: &WalletControl,
    review_id: TransactionProposalId,
    cancelled: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<ReviewedTransfer, IpcError> {
    let pending = match control.begin(&review_id) {
        Ok(pending) => pending,
        Err(error) => {
            control.discard(&review_id);
            return Err(error);
        }
    };
    let _active_review = ActiveReview(control);
    wallet.transfer_work(app, cancelled, Some(pending.wallet_epoch), |aborted| {
        tauri::async_runtime::block_on(review_pending(
            app, core, wallet, review_id, &pending, aborted,
        ))
    })
}

async fn review_pending(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    review_id: TransactionProposalId,
    pending: &PendingReview,
    cancelled: &TransferCancellation,
) -> Result<ReviewedTransfer, IpcError> {
    check_pending(wallet, pending, cancelled)?;
    let age = pending.prepared_at.elapsed();
    let now_unix = match unix_now() {
        Ok(now_unix) => now_unix,
        Err(error) => {
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
    let decision = match show_native_review(app, display).await {
        Ok(decision) => decision,
        Err(error) => {
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
    if unix_now()? >= pending.expires_at_unix
        || pending.prepared_at.elapsed() >= STALE_SIMULATION_AFTER
    {
        return Err(IpcError::invalid_request(
            "The exact transaction approval expired",
        ));
    }
    let submission = if decision == EvmApprovalDecision::Approved {
        Some(
            approve_sign_and_broadcast(
                core, wallet, &review_id, pending, &payload, &evidence, cancelled,
            )
            .await?,
        )
    } else {
        None
    };
    Ok(ReviewedTransfer {
        submission_uncertain: submission.as_ref().is_some_and(|value| value.uncertain),
        outcome: EvmApprovalReviewOutcome {
            review_id,
            decision,
            transaction_hash: submission
                .as_ref()
                .map(|value| value.transaction_hash.clone()),
            explorer_url: submission.map(|value| value.explorer_url),
        },
    })
}

struct ApprovedSubmission {
    transaction_hash: String,
    explorer_url: String,
    uncertain: bool,
}

async fn approve_sign_and_broadcast(
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    payload: &[u8],
    evidence: &presentation::ApprovalEvidence,
    cancelled: &TransferCancellation,
) -> Result<ApprovedSubmission, IpcError> {
    check_pending(wallet, pending, cancelled)?;
    let commitment = ApprovalCommitment::issue(evidence.binding_input(), unix_now()?)
        .map_err(commitment_error)?;
    let signature = sign_approved(
        core,
        wallet,
        review_id,
        pending,
        evidence,
        &commitment,
        cancelled,
    )
    .await?;
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
        cancelled,
    )
    .await
}

async fn sign_approved(
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    evidence: &presentation::ApprovalEvidence,
    commitment: &ApprovalCommitment,
    cancelled: &TransferCancellation,
) -> Result<crate::wallet_lifecycle::WalletSigned, IpcError> {
    let signature = wallet
        .sign_exact_transaction(core, commitment, evidence, cancelled, async {
            check_pending(wallet, pending, cancelled)?;
            crate::evm_transfers::preparation::revalidate(
                &pending.candidate,
                refresh_policy(pending.origin),
            )
            .await?;
            check_pending(wallet, pending, cancelled)
        })
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
    core: &Arc<AppCore>,
    review_id: &TransactionProposalId,
    pending: &PendingReview,
    evidence: &presentation::ApprovalEvidence,
    signed: zeroize::Zeroizing<String>,
    submission: crate::wallet_lifecycle::WalletSubmission,
    cancelled: &TransferCancellation,
) -> Result<ApprovedSubmission, IpcError> {
    crate::evm_transfers::preparation::revalidate(
        &pending.candidate,
        refresh_policy(pending.origin),
    )
    .await?;
    let network = pending.candidate.network()?;
    let endpoint = resolved(network)?;
    let transaction_hash = chain_evm::signed_transaction_hash(&signed)
        .map_err(|_| IpcError::internal())?
        .as_str()
        .to_owned();
    let timestamp = current_timestamp()?;
    let record = StoredEvmTransaction {
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
    };
    cancelled.check()?;
    let cancellation = cancelled.clone();
    let reservation_id = review_id.0.clone();
    let submission_core = Arc::clone(core);
    let admission = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_admission = Arc::clone(&admission);
    let receipt = tokio::task::spawn_blocking(move || {
        broadcast_signed_transaction_with_admission(network, &endpoint, signed, || {
            if submission_core
                .reserve_evm_transaction(&record, &reservation_id)
                .is_err()
            {
                return false;
            }
            let ready = cancellation.check().is_ok()
                && submission_core
                    .mark_evm_dispatched(&record.transaction_hash, &reservation_id)
                    .unwrap_or(false);
            let admitted = ready && cancellation.admit(|| submission.begin()).is_ok();
            worker_admission.store(admitted, std::sync::atomic::Ordering::Release);
            if !admitted {
                let _ = submission_core
                    .discard_unsent_evm_transaction(&record.transaction_hash, &reservation_id);
            }
            admitted
        })
    })
    .await
    .unwrap_or(Err(chain_evm::BroadcastError::Transport));
    let admitted = admission.load(std::sync::atomic::Ordering::Acquire);
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
    if let Err(error) = &receipt
        && !admitted
    {
        return Err(error.clone());
    }
    Ok(ApprovedSubmission {
        explorer_url: format!(
            "{}/tx/{transaction_hash}",
            network.descriptor().block_explorer_url()
        ),
        transaction_hash,
        uncertain: receipt.is_err(),
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
#[path = "submission_outcome_tests.rs"]
mod submission_outcome_tests;

struct ActiveReview<'a>(&'a EvmApprovalControl);

impl Drop for ActiveReview<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

fn record_audit(
    core: &Arc<AppCore>,
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

fn check_pending(
    wallet: &WalletControl,
    pending: &PendingReview,
    cancelled: &TransferCancellation,
) -> Result<(), IpcError> {
    if cancelled.is_cancelled() {
        return Err(IpcError {
            code: IpcErrorCode::Cancelled,
            message: "Transfer cancelled; no new approval is available".into(),
            retryable: false,
        });
    }
    if unix_now()? >= pending.expires_at_unix
        || pending.prepared_at.elapsed() >= STALE_SIMULATION_AFTER
    {
        return Err(IpcError::invalid_request(
            "The exact transaction approval expired",
        ));
    }
    wallet.check_transfer_epoch(pending.wallet_epoch)?;
    if !wallet
        .selected_address()?
        .eq_ignore_ascii_case(pending.candidate.sender())
    {
        return Err(IpcError::invalid_request(
            "The selected wallet account changed; prepare a new transfer",
        ));
    }
    Ok(())
}

fn commitment_error(_error: transaction_policy::ApprovalCommitmentError) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: "The exact transaction approval expired or changed before it could be bound"
            .to_owned(),
        retryable: false,
    }
}

#[cfg(target_os = "macos")]
async fn show_native_review(
    app: &tauri::AppHandle,
    display: ApprovalDisplay,
) -> Result<EvmApprovalDecision, IpcError> {
    super::native::show(app, display).await
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

fn refresh_policy(origin: ApprovalOrigin) -> crate::evm_transfers::preparation::RefreshPolicy {
    use crate::evm_transfers::preparation::RefreshPolicy;
    match origin {
        ApprovalOrigin::User => RefreshPolicy::Wallet,
        ApprovalOrigin::Chat { .. } => RefreshPolicy::Chat,
    }
}

#[cfg(test)]
mod refresh_policy_tests {
    use super::*;
    use chain_evm::SimulationState;
    #[test]
    fn wallet_and_chat_origins_keep_distinct_warning_policies() {
        let state = SimulationState::Inconclusive;
        assert!(refresh_policy(ApprovalOrigin::User).accepts(state, state));
        assert!(
            !refresh_policy(ApprovalOrigin::Chat {
                conversation: [0; 32],
                call: [1; 32]
            })
            .accepts(state, state)
        );
    }
}
