//! Consumed approval and exact signed validation precede durable send admission.
use super::{
    context::{Context, error as chain_error},
    lifecycle::StepLifecycle,
    observation::CompletedStep,
};
use crate::{
    evm_approval::{
        execution::show_native_review, presentation::generic_approval, registry::unix_now,
    },
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::{
    AppCore, ContractStepState, PreparedDeclaredContractStep, StoredEvmTransaction,
    StoredEvmTransactionState,
};
use chain_evm::{
    assemble_signed_eip1559_transaction, contract::generic::RefreshedGenericCandidate,
};
use ipc_types::{EvmApprovalDecision, IpcError};
use std::sync::Arc;
use transaction_policy::ApprovalCommitment;

#[expect(
    clippy::too_many_arguments,
    reason = "Native owners and authority-free lifecycle reporting stay explicit"
)]
pub(super) async fn review_and_send(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    wallet: &WalletControl,
    context: &Arc<Context>,
    step: &Arc<PreparedDeclaredContractStep>,
    completed: &[CompletedStep],
    cancel: &TransferCancellation,
    progress: &mut StepLifecycle,
    summary: Option<&str>,
) -> Result<Option<StoredEvmTransaction>, IpcError> {
    context.check(wallet, cancel)?;
    let session = uuid::Uuid::now_v7().to_string();
    let expiry = unix_now()? + 120;
    let prepared = generic_approval(
        step.candidate(),
        step.declaration(),
        step.context(),
        step.index(),
        &session,
        expiry,
    )?;
    let crate::evm_approval::presentation::PreparedApproval {
        display,
        payload,
        evidence,
        audit_warning_codes,
    } = prepared;
    let display = display.with_model_summary(summary)?;
    let audit = super::super::audit::Audit {
        core,
        evidence: &evidence,
        session: &session,
        origin: context.origin,
    };
    audit.presented(audit_warning_codes);
    progress.exact_review_requested();
    let decision = show_native_review(app, display).await?;
    progress.exact_review_decision(decision);
    audit.decision(decision);
    if decision != EvmApprovalDecision::Approved {
        stop_declined_review(step, decision)?;
        return Ok(None);
    }
    context.check(wallet, cancel)?;
    let candidate = step.candidate();
    let conditions = Authorization {
        core,
        wallet,
        context,
        step,
        completed,
        cancel,
        session: &session,
        expiry,
    };
    let authorized = authorize(&conditions, &evidence, progress).await;
    audit.authorization(&authorized);
    let (signed, after_auth, nonce) = authorized?;
    let raw = assemble_signed_eip1559_transaction(
        &payload,
        signed.signature.r(),
        signed.signature.s(),
        signed.signature.y_parity(),
    )
    .map_err(chain_error)?;
    let bytes = signed_bytes(&raw)?;
    // The post-password certificate is consumed by the signed-envelope check.
    let verified = validate(context, after_auth, bytes.clone()).await?;
    if verified.approved_candidate_digest() != candidate.candidate_digest() {
        return Err(IpcError::internal());
    }
    context.check(wallet, cancel)?;
    // Refresh a second time immediately before actual transport, retaining the
    // exact approved body; no caller may replace fees/nonce or widen authority.
    progress.presend_refresh_started();
    let (fresh, proofs) = refresh(context, step, completed).await?;
    progress.presend_refresh_succeeded();
    let final_check = validate(context, fresh, bytes).await?;
    context.check(wallet, cancel)?;
    if final_check.evidence_block_hash() != proofs.conditions().block_hash() {
        return Err(IpcError::invalid_request(
            "Execution and prerequisites require one fresh common state block",
        ));
    }
    let hash = final_check.transaction_hash().to_owned();
    if final_check.approved_candidate_digest() != candidate.candidate_digest() {
        return Err(IpcError::internal());
    }
    let record = monitoring_record(context, candidate, &hash)?;
    let outcome = super::submission::broadcast(
        Arc::clone(context),
        wallet.clone(),
        cancel.clone(),
        Arc::clone(step),
        super::submission::ReadySubmission {
            record,
            raw,
            nonce,
            submission: signed.submission,
            validated: final_check,
            proofs,
            expiry,
        },
        progress,
    )
    .await;
    audit.broadcast(&outcome);
    outcome.map(Some)
}

fn stop_declined_review(
    step: &PreparedDeclaredContractStep,
    decision: EvmApprovalDecision,
) -> Result<(), IpcError> {
    let state = if decision == EvmApprovalDecision::Denied {
        ContractStepState::Denied
    } else {
        ContractStepState::Cancelled
    };
    step.stop_before_send(state)
        .map(|_| ())
        .map_err(crate::error_mapping::app_core_error)
}

async fn refresh(
    context: &Context,
    step: &PreparedDeclaredContractStep,
    completed: &[CompletedStep],
) -> Result<
    (
        RefreshedGenericCandidate,
        chain_evm::contract::generic::VerifiedGenericPrerequisites,
    ),
    IpcError,
> {
    context
        .refresh(
            step,
            &completed
                .iter()
                .map(|step| Arc::clone(&step.completion))
                .collect::<Vec<_>>(),
        )
        .await
}
async fn validate(
    context: &Context,
    fresh: RefreshedGenericCandidate,
    bytes: zeroize::Zeroizing<Vec<u8>>,
) -> Result<chain_evm::contract::generic::ValidatedGenericExecution, IpcError> {
    let workers = Arc::clone(&context.workers);
    let cancelled = Arc::clone(&context.cancelled);
    tokio::task::spawn_blocking(move || fresh.validate_signed(&workers, &bytes, &cancelled))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(chain_error)
}
fn signed_bytes(value: &str) -> Result<zeroize::Zeroizing<Vec<u8>>, IpcError> {
    let hex = value.strip_prefix("0x").ok_or_else(IpcError::internal)?;
    if hex.len() > 16_384 || !hex.len().is_multiple_of(2) {
        return Err(IpcError::internal());
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or_else(IpcError::internal)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(zeroize::Zeroizing::new)
}

struct Authorization<'a> {
    core: &'a Arc<AppCore>,
    wallet: &'a WalletControl,
    context: &'a Arc<Context>,
    step: &'a Arc<PreparedDeclaredContractStep>,
    completed: &'a [CompletedStep],
    cancel: &'a TransferCancellation,
    session: &'a str,
    expiry: u64,
}
async fn authorize(
    conditions: &Authorization<'_>,
    evidence: &crate::evm_approval::presentation::ApprovalEvidence,
    progress: &mut StepLifecycle,
) -> Result<
    (
        crate::wallet_lifecycle::WalletSigned,
        RefreshedGenericCandidate,
        app_core::EvmNonceReservation,
    ),
    IpcError,
> {
    let Authorization {
        core,
        wallet,
        context,
        step,
        completed,
        cancel,
        session,
        expiry,
    } = *conditions;
    let candidate = step.candidate();
    let nonce = core
        .claim_evm_nonce(
            candidate.intent().network().descriptor().caip2_id(),
            candidate.intent().account(),
            candidate.decoded().nonce(),
            candidate.candidate_digest(),
            session,
        )
        .map_err(crate::error_mapping::app_core_error)?
        .ok_or_else(|| {
            IpcError::invalid_request(
                "The nonce is owned or may already have been sent; the workflow is stopped",
            )
        })?;
    let commitment =
        ApprovalCommitment::issue(evidence.binding_input(), unix_now()?).map_err(chain_error)?;
    let mut after_auth = None;
    progress.authorization_requested();
    let signed = wallet
        .sign_exact_transaction(core, &commitment, evidence, cancel, async {
            // This future is polled only after native fresh authentication succeeds.
            // A failure below is therefore after authentication, before signing.
            progress.authentication_succeeded();
            context.check(wallet, cancel)?;
            if !nonce
                .is_current()
                .map_err(crate::error_mapping::app_core_error)?
            {
                return Err(IpcError::invalid_request("Nonce ownership was retired"));
            }
            let (fresh, proofs) = refresh(context, step, completed).await?;
            context.check(wallet, cancel)?;
            if unix_now()? >= expiry || !fresh.is_fresh() {
                return Err(IpcError::invalid_request(
                    "The approved contract evidence expired",
                ));
            }
            if !nonce
                .is_current()
                .map_err(crate::error_mapping::app_core_error)?
            {
                return Err(IpcError::invalid_request(
                    "Nonce owner changed after refresh",
                ));
            }
            if fresh.block_hash() != proofs.conditions().block_hash() {
                return Err(IpcError::invalid_request(
                    "Execution and prerequisites changed state cohorts",
                ));
            }
            step.revalidate_preparation(proofs)
                .map_err(crate::error_mapping::app_core_error)?;
            if !fresh.is_fresh() {
                return Err(IpcError::invalid_request(
                    "Qualified evidence expired during the final workflow check",
                ));
            }
            after_auth = Some(fresh);
            progress.signing_started();
            Ok(())
        })
        .await?;
    progress.signature_returned();
    Ok((signed, after_auth.ok_or_else(IpcError::internal)?, nonce))
}

fn monitoring_record(
    context: &Context,
    candidate: &chain_evm::contract::generic::GenericContractCandidate,
    hash: &str,
) -> Result<StoredEvmTransaction, IpcError> {
    let timestamp = crate::error_mapping::current_timestamp()?;
    Ok(StoredEvmTransaction {
        transaction_hash: hash.into(),
        network: context.network.descriptor().caip2_id().into(),
        sender: context.account.clone(),
        nonce: candidate.decoded().nonce().into(),
        candidate_digest: candidate.candidate_digest().into(),
        state: StoredEvmTransactionState::Pending,
        block_number: None,
        block_hash: None,
        submitted_at: timestamp.clone(),
        updated_at: timestamp,
    })
}
