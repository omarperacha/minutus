//! Process-local x402 native review and approval-commitment boundary.

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
mod public_request;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use app_core::AppCore;
#[cfg(target_os = "macos")]
use app_core::{
    ApprovalAuditDecision, TransactionAuditEvent, TransactionAuditEventKind,
    TransactionAuditFailureCode, TransactionAuditOrigin, TransactionAuditResult,
    TransactionAuditTransactionType, TransactionAuditWarningCode, X402AuditStage,
};
use ipc_types::{
    IpcError, IpcErrorCode, TransactionProposalId, X402ApprovalDecision, X402ApprovalReviewOutcome,
};
use tauri::State;
use transaction_policy::ApprovalCommitment;
#[cfg(target_os = "macos")]
use transaction_policy::ApprovalInvalidationReason;
use x402_client::PreparedX402Approval;
#[cfg(target_os = "macos")]
use x402_client::{
    PaymentRequiredChallenge, ValidatedX402PaymentResponse, X402ApprovalWarning, X402PaymentError,
    X402PreparationError, create_x402_payment_signature_header, parse_payment_required_response,
    revalidate_prepared_x402_authorization, validate_x402_payment_response,
};

#[cfg(target_os = "macos")]
use crate::wallet_lifecycle::WalletControl;
#[cfg(all(target_os = "macos", debug_assertions))]
use web_retrieval::{DebugX402Broker, DebugX402Method, DebugX402Request};
#[cfg(target_os = "macos")]
use web_retrieval::{
    HostResolver, WebBroker, WebBrokerError, WebBrokerResponse, WebCancellation, WebExtraction,
    WebPaymentSignature, WebRequest, extract_web_response,
};
#[cfg(all(target_os = "macos", debug_assertions))]
use x402_client::X402HttpMethod;

const MAX_PENDING_X402_REVIEWS: usize = 32;

#[derive(Default)]
struct Registry {
    pending: HashMap<String, PreparedX402Approval>,
    approved: HashMap<String, ApprovedX402Payment>,
    active: bool,
}

/// Exact payment plus opaque commitment retained for single-use signing.
pub(crate) struct ApprovedX402Payment {
    approval: PreparedX402Approval,
    commitment: ApprovalCommitment,
}

impl ApprovedX402Payment {
    pub(crate) const fn approval(&self) -> &PreparedX402Approval {
        &self.approval
    }

    pub(crate) const fn commitment(&self) -> &ApprovalCommitment {
        &self.commitment
    }
}

/// Host-owned registry; opaque IDs cannot carry a candidate or decision.
#[derive(Default)]
pub(crate) struct X402ApprovalControl {
    registry: Mutex<Registry>,
}

impl X402ApprovalControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a candidate from a host-validated retained-request adapter.
    /// Registration remains host-only so chat and generic web retrieval cannot
    /// manufacture native review candidates.
    pub(crate) fn register(
        &self,
        approval: PreparedX402Approval,
    ) -> Result<TransactionProposalId, IpcError> {
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.pending.len() >= MAX_PENDING_X402_REVIEWS {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "Too many x402 payments are waiting for trusted review".to_owned(),
                retryable: true,
            });
        }
        let review_id = TransactionProposalId(uuid::Uuid::now_v7().to_string());
        registry.pending.insert(review_id.0.clone(), approval);
        Ok(review_id)
    }

    fn begin(&self, review_id: &TransactionProposalId) -> Result<PreparedX402Approval, IpcError> {
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.active {
            return Err(IpcError {
                code: IpcErrorCode::Unsupported,
                message: "Another x402 payment is already open for trusted review".to_owned(),
                retryable: true,
            });
        }
        let approval = registry
            .pending
            .remove(&review_id.0)
            .ok_or_else(|| IpcError {
                code: IpcErrorCode::InvalidRequest,
                message: "The x402 review reference is unknown or has already been used".to_owned(),
                retryable: false,
            })?;
        registry.active = true;
        Ok(approval)
    }

    fn finish(&self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.active = false;
        }
    }

    fn retain_approved(
        &self,
        review_id: &TransactionProposalId,
        approval: PreparedX402Approval,
        commitment: ApprovalCommitment,
    ) -> Result<(), IpcError> {
        let mut registry = self.registry.lock().map_err(|_| IpcError::internal())?;
        if registry.approved.contains_key(&review_id.0) {
            return Err(IpcError::internal());
        }
        registry.approved.insert(
            review_id.0.clone(),
            ApprovedX402Payment {
                approval,
                commitment,
            },
        );
        Ok(())
    }

    /// Removes the approved value exactly once immediately before signing.
    pub(crate) fn take_approved(
        &self,
        review_id: &TransactionProposalId,
    ) -> Result<ApprovedX402Payment, IpcError> {
        self.registry
            .lock()
            .map_err(|_| IpcError::internal())?
            .approved
            .remove(&review_id.0)
            .ok_or_else(|| IpcError {
                code: IpcErrorCode::InvalidRequest,
                message: "The approved x402 payment is unavailable or already consumed".to_owned(),
                retryable: false,
            })
    }

    /// Consumes one approval through the configured debug-only loopback transport.
    #[cfg(all(target_os = "macos", debug_assertions))]
    #[expect(
        clippy::too_many_lines,
        reason = "single-use approval consumption, audit stages and terminal ambiguity classification remain visible as one signing boundary"
    )]
    pub(crate) async fn execute_debug_approved_once(
        &self,
        core: &AppCore,
        wallet: &WalletControl,
        review_id: &TransactionProposalId,
        request: &DebugX402Request,
        bearer: Option<&str>,
        cancellation: &WebCancellation,
    ) -> Result<PurchasedDebugX402Content, X402ExecutionError> {
        let approved = self
            .take_approved(review_id)
            .map_err(|_| X402ExecutionError::ApprovalUnavailable)?;
        let approval = approved.approval();
        revalidate_approved(core, &approved, review_id)?;
        let method_matches = matches!(
            (approval.candidate().method(), request.method()),
            (X402HttpMethod::Get, DebugX402Method::Get)
                | (X402HttpMethod::Post, DebugX402Method::Post)
        );
        if approval.candidate().resource().canonical_url() != request.url()
            || !method_matches
            || &request.identity_sha256(bearer) != approval.candidate().request_identity_sha256()
        {
            return Err(X402ExecutionError::RequestBindingMismatch);
        }
        if cancellation.is_cancelled() {
            let failure = X402ExecutionError::CancelledBeforeSubmission;
            record_execution_failure(
                core,
                approval,
                review_id,
                X402AuditStage::PaidRetry,
                &failure,
            );
            return Err(failure);
        }
        let (payment, submission) =
            sign_and_build_payment(core, wallet, &approved, review_id, cancellation).await?;
        let response = match DebugX402Broker
            .execute_request_with_admission(request, bearer, &payment, cancellation, move || {
                submission.begin().is_ok()
            })
            .await
        {
            Ok(response) => {
                record_x402_audit(
                    core,
                    approval,
                    review_id,
                    X402AuditStage::PaidRetry,
                    TransactionAuditResult::Succeeded,
                    false,
                );
                response
            }
            Err(error) => {
                let failure = classify_retry_failure(error);
                record_execution_failure(
                    core,
                    approval,
                    review_id,
                    X402AuditStage::PaidRetry,
                    &failure,
                );
                return Err(failure);
            }
        };
        let status = response.status();
        if status == 402 {
            let headers = response
                .payment_required_headers()
                .iter()
                .map(Vec::as_slice)
                .collect::<Vec<_>>();
            let changed = parse_payment_required_response(
                status,
                &headers,
                request.url(),
                unix_now_for_execution()?,
            )
            .map_err(|_| {
                let failure =
                    X402ExecutionError::Ambiguous(X402AmbiguousFailure::MalformedResponse);
                record_execution_failure(
                    core,
                    approval,
                    review_id,
                    X402AuditStage::SettlementValidation,
                    &failure,
                );
                failure
            })?;
            let failure = X402ExecutionError::RequirementsChanged(changed);
            record_execution_failure(
                core,
                approval,
                review_id,
                X402AuditStage::SettlementValidation,
                &failure,
            );
            return Err(failure);
        }
        let headers = response
            .payment_response_headers()
            .iter()
            .map(Vec::as_slice)
            .collect::<Vec<_>>();
        let settlement = validate_x402_payment_response(status, &headers, approval.candidate())
            .map_err(|error| {
                let failure = classify_payment_response_failure(error);
                record_execution_failure(
                    core,
                    approval,
                    review_id,
                    X402AuditStage::SettlementValidation,
                    &failure,
                );
                failure
            })?;
        record_x402_audit(
            core,
            approval,
            review_id,
            X402AuditStage::SettlementValidation,
            TransactionAuditResult::Succeeded,
            false,
        );
        let content = std::str::from_utf8(response.body()).map_err(|_| {
            let failure = X402ExecutionError::PaymentConfirmedContentUnavailable;
            record_execution_failure(
                core,
                approval,
                review_id,
                X402AuditStage::ContentDelivery,
                &failure,
            );
            failure
        })?;
        record_x402_audit(
            core,
            approval,
            review_id,
            X402AuditStage::ContentDelivery,
            TransactionAuditResult::Succeeded,
            false,
        );
        Ok(PurchasedDebugX402Content {
            settlement,
            content: content.to_owned(),
        })
    }
}

#[cfg(target_os = "macos")]
fn revalidate_approved(
    core: &AppCore,
    approved: &ApprovedX402Payment,
    review_id: &TransactionProposalId,
) -> Result<(), X402ExecutionError> {
    let approval = approved.approval();
    let now_unix = unix_now_for_execution()?;
    if let Err(error) = revalidate_prepared_x402_authorization(approval.candidate(), now_unix) {
        let (failure, reason) = if error == X402PreparationError::ExpiredRequirement {
            (
                X402ExecutionError::Expired,
                ApprovalInvalidationReason::Expired,
            )
        } else {
            (
                X402ExecutionError::CandidateChanged,
                ApprovalInvalidationReason::Superseded,
            )
        };
        invalidate_changed_approval(core, approved, review_id, now_unix, reason)?;
        return Err(failure);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn invalidate_changed_approval(
    core: &AppCore,
    approved: &ApprovedX402Payment,
    review_id: &TransactionProposalId,
    now_unix: u64,
    reason: ApprovalInvalidationReason,
) -> Result<(), X402ExecutionError> {
    approved
        .commitment()
        .invalidate(reason, now_unix, core)
        .map_err(|_| X402ExecutionError::ApprovalUnavailable)?;
    record_x402_audit(
        core,
        approved.approval(),
        review_id,
        X402AuditStage::Signing,
        TransactionAuditResult::Failed(TransactionAuditFailureCode::CandidateInvalidated),
        false,
    );
    Ok(())
}

#[cfg(target_os = "macos")]
async fn sign_and_build_payment(
    core: &AppCore,
    wallet: &WalletControl,
    approved: &ApprovedX402Payment,
    review_id: &TransactionProposalId,
    cancellation: &WebCancellation,
) -> Result<
    (
        WebPaymentSignature,
        crate::wallet_lifecycle::WalletSubmission,
    ),
    X402ExecutionError,
> {
    let approval = approved.approval();
    let Ok(signed) = wallet
        .sign_exact_x402_payment(core, approved.commitment(), approval, || {
            cancellation.is_cancelled()
        })
        .await
    else {
        record_x402_audit(
            core,
            approval,
            review_id,
            X402AuditStage::Signing,
            TransactionAuditResult::Failed(TransactionAuditFailureCode::SigningFailed),
            false,
        );
        return Err(X402ExecutionError::SigningFailed);
    };
    record_x402_audit(
        core,
        approval,
        review_id,
        X402AuditStage::Signing,
        TransactionAuditResult::Succeeded,
        false,
    );
    let candidate = approval.candidate();
    let header = create_x402_payment_signature_header(
        candidate,
        signed.signature.r(),
        signed.signature.s(),
        signed.signature.y_parity(),
    )
    .map_err(|_| payload_failure(core, approval, review_id))?;
    let header = WebPaymentSignature::new(header.as_str())
        .map_err(|_| payload_failure(core, approval, review_id))?;
    Ok((header, signed.submission))
}

#[cfg(target_os = "macos")]
fn payload_failure(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
) -> X402ExecutionError {
    record_x402_audit(
        core,
        approval,
        review_id,
        X402AuditStage::PaidRetry,
        TransactionAuditResult::Failed(TransactionAuditFailureCode::InternalInvariant),
        false,
    );
    X402ExecutionError::PayloadRejected
}

#[cfg(target_os = "macos")]
async fn execute_paid_retry(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    request: &WebRequest,
    payment: (
        WebPaymentSignature,
        crate::wallet_lifecycle::WalletSubmission,
    ),
    resolver: &impl HostResolver,
    cancellation: &WebCancellation,
) -> Result<WebBrokerResponse, X402ExecutionError> {
    let (payment, submission) = payment;
    if cancellation.is_cancelled() {
        let failure = X402ExecutionError::CancelledBeforeSubmission;
        record_execution_failure(
            core,
            approval,
            review_id,
            X402AuditStage::PaidRetry,
            &failure,
        );
        return Err(failure);
    }
    match WebBroker
        .execute_paid_once_with_admission(request, &payment, resolver, cancellation, move || {
            submission.begin().is_ok()
        })
        .await
    {
        Ok(response) => {
            record_x402_audit(
                core,
                approval,
                review_id,
                X402AuditStage::PaidRetry,
                TransactionAuditResult::Succeeded,
                false,
            );
            Ok(response)
        }
        Err(error) => {
            let failure = classify_retry_failure(error);
            record_execution_failure(
                core,
                approval,
                review_id,
                X402AuditStage::PaidRetry,
                &failure,
            );
            Err(failure)
        }
    }
}

#[cfg(target_os = "macos")]
fn validate_paid_response(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    response: &WebBrokerResponse,
) -> Result<PurchasedX402Content, X402ExecutionError> {
    if response.status() == 402 {
        let headers = response
            .payment_required_headers()
            .iter()
            .map(Vec::as_slice)
            .collect::<Vec<_>>();
        let failure = parse_payment_required_response(
            response.status(),
            &headers,
            approval.candidate().resource().canonical_url(),
            unix_now_for_execution()?,
        )
        .map_or_else(
            |_| X402ExecutionError::Ambiguous(X402AmbiguousFailure::MalformedResponse),
            X402ExecutionError::RequirementsChanged,
        );
        record_execution_failure(
            core,
            approval,
            review_id,
            X402AuditStage::SettlementValidation,
            &failure,
        );
        return Err(failure);
    }
    let headers = response
        .payment_response_headers()
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    let settlement =
        validate_x402_payment_response(response.status(), &headers, approval.candidate()).map_err(
            |error| {
                let failure = classify_payment_response_failure(error);
                record_execution_failure(
                    core,
                    approval,
                    review_id,
                    X402AuditStage::SettlementValidation,
                    &failure,
                );
                failure
            },
        )?;
    record_x402_audit(
        core,
        approval,
        review_id,
        X402AuditStage::SettlementValidation,
        TransactionAuditResult::Succeeded,
        false,
    );
    let content = extract_web_response(response).map_err(|_| {
        let failure = X402ExecutionError::PaymentConfirmedContentUnavailable;
        record_execution_failure(
            core,
            approval,
            review_id,
            X402AuditStage::ContentDelivery,
            &failure,
        );
        failure
    })?;
    record_x402_audit(
        core,
        approval,
        review_id,
        X402AuditStage::ContentDelivery,
        TransactionAuditResult::Succeeded,
        false,
    );
    Ok(PurchasedX402Content {
        settlement,
        content,
    })
}

#[cfg(target_os = "macos")]
fn record_execution_failure(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    stage: X402AuditStage,
    failure: &X402ExecutionError,
) {
    record_x402_audit(
        core,
        approval,
        review_id,
        stage,
        TransactionAuditResult::Failed(failure.audit_failure_code()),
        failure.payment_may_have_occurred(),
    );
}

/// Confirmed settlement evidence paired with bounded untrusted remote content.
#[cfg(target_os = "macos")]
pub(crate) struct PurchasedX402Content {
    settlement: ValidatedX402PaymentResponse,
    content: WebExtraction,
}

#[cfg(target_os = "macos")]
impl PurchasedX402Content {
    pub(crate) const fn settlement(&self) -> &ValidatedX402PaymentResponse {
        &self.settlement
    }

    pub(crate) const fn untrusted_content(&self) -> &WebExtraction {
        &self.content
    }
}

/// Confirmed debug-emulator settlement paired with bounded UTF-8 content.
#[cfg(all(target_os = "macos", debug_assertions))]
pub(crate) struct PurchasedDebugX402Content {
    settlement: ValidatedX402PaymentResponse,
    content: String,
}

#[cfg(all(target_os = "macos", debug_assertions))]
impl PurchasedDebugX402Content {
    pub(crate) const fn settlement(&self) -> &ValidatedX402PaymentResponse {
        &self.settlement
    }

    pub(crate) fn untrusted_content(&self) -> &str {
        &self.content
    }
}

/// Closed terminal result for one consumed approved paid-request attempt.
#[cfg(target_os = "macos")]
pub(crate) enum X402ExecutionError {
    ApprovalUnavailable,
    CandidateChanged,
    Expired,
    RequestBindingMismatch,
    SigningFailed,
    PayloadRejected,
    CancelledBeforeSubmission,
    #[allow(
        dead_code,
        reason = "fresh Flowzed proposal consumption is introduced by X402-007"
    )]
    RequirementsChanged(PaymentRequiredChallenge),
    ConfirmedUnpaid(X402UnpaidFailure),
    Ambiguous(X402AmbiguousFailure),
    PaymentConfirmedContentUnavailable,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum X402UnpaidFailure {
    DnsTimeout,
    TargetRejected,
    ClientUnavailable,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum X402AmbiguousFailure {
    Cancelled,
    ConnectionOrHeaderTimeout,
    BodyTimeout,
    TotalTimeout,
    ResponseRejected,
    TransportLoss,
    UnexpectedStatus,
    MissingPaymentResponse,
    DuplicatePaymentResponse,
    MalformedResponse,
    SettlementRejected,
    UnsupportedSettlementExtension,
    SettlementMismatch,
}

#[cfg(target_os = "macos")]
impl X402ExecutionError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::ApprovalUnavailable => "approval_unavailable",
            Self::CandidateChanged => "candidate_changed",
            Self::Expired => "expired",
            Self::RequestBindingMismatch => "request_binding_mismatch",
            Self::SigningFailed => "signing_failed",
            Self::PayloadRejected => "payload_rejected",
            Self::CancelledBeforeSubmission => "cancelled_before_submission",
            Self::RequirementsChanged(_) => "requirements_changed",
            Self::ConfirmedUnpaid(X402UnpaidFailure::DnsTimeout) => "dns_timeout_unpaid",
            Self::ConfirmedUnpaid(X402UnpaidFailure::TargetRejected) => "target_rejected_unpaid",
            Self::ConfirmedUnpaid(X402UnpaidFailure::ClientUnavailable) => {
                "client_unavailable_unpaid"
            }
            Self::Ambiguous(X402AmbiguousFailure::Cancelled) => "cancelled_ambiguous",
            Self::Ambiguous(X402AmbiguousFailure::ConnectionOrHeaderTimeout) => {
                "connection_or_header_timeout_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::BodyTimeout) => "body_timeout_ambiguous",
            Self::Ambiguous(X402AmbiguousFailure::TotalTimeout) => "total_timeout_ambiguous",
            Self::Ambiguous(X402AmbiguousFailure::ResponseRejected) => {
                "response_rejected_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::TransportLoss) => "transport_loss_ambiguous",
            Self::Ambiguous(X402AmbiguousFailure::UnexpectedStatus) => {
                "unexpected_status_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::MissingPaymentResponse) => {
                "missing_payment_response_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::DuplicatePaymentResponse) => {
                "duplicate_payment_response_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::MalformedResponse) => {
                "malformed_payment_response_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::SettlementRejected) => {
                "settlement_rejected_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::UnsupportedSettlementExtension) => {
                "settlement_extension_ambiguous"
            }
            Self::Ambiguous(X402AmbiguousFailure::SettlementMismatch) => {
                "settlement_mismatch_ambiguous"
            }
            Self::PaymentConfirmedContentUnavailable => "payment_confirmed_content_unavailable",
        }
    }

    pub(crate) const fn payment_may_have_occurred(&self) -> bool {
        matches!(self, Self::RequirementsChanged(_) | Self::Ambiguous(_))
    }

    #[allow(
        dead_code,
        reason = "Flowzed result projection is introduced by X402-007"
    )]
    pub(crate) const fn payment_confirmed(&self) -> bool {
        matches!(self, Self::PaymentConfirmedContentUnavailable)
    }

    #[allow(
        dead_code,
        clippy::unused_self,
        reason = "X402-007 will consume the per-failure policy; every X402-006 class is currently terminal"
    )]
    pub(crate) const fn automatic_retry_allowed(&self) -> bool {
        false
    }

    #[allow(
        dead_code,
        reason = "fresh Flowzed proposal consumption is introduced by X402-007"
    )]
    pub(crate) const fn requires_fresh_approval(&self) -> bool {
        matches!(self, Self::RequirementsChanged(_))
    }

    #[allow(
        dead_code,
        reason = "fresh Flowzed proposal consumption is introduced by X402-007"
    )]
    pub(crate) const fn fresh_challenge(&self) -> Option<&PaymentRequiredChallenge> {
        match self {
            Self::RequirementsChanged(challenge) => Some(challenge),
            _ => None,
        }
    }

    const fn audit_failure_code(&self) -> TransactionAuditFailureCode {
        match self {
            Self::RequirementsChanged(_) => TransactionAuditFailureCode::X402RequirementChanged,
            Self::ConfirmedUnpaid(_) | Self::CancelledBeforeSubmission => {
                TransactionAuditFailureCode::X402RetryFailed
            }
            Self::Ambiguous(failure) if failure.is_settlement_failure() => {
                TransactionAuditFailureCode::X402SettlementFailed
            }
            Self::Ambiguous(_) => TransactionAuditFailureCode::X402OutcomeAmbiguous,
            Self::PaymentConfirmedContentUnavailable => {
                TransactionAuditFailureCode::X402ContentFailed
            }
            Self::ApprovalUnavailable => TransactionAuditFailureCode::ApprovalUnavailable,
            Self::CandidateChanged | Self::Expired | Self::RequestBindingMismatch => {
                TransactionAuditFailureCode::CandidateInvalidated
            }
            Self::SigningFailed => TransactionAuditFailureCode::SigningFailed,
            Self::PayloadRejected => TransactionAuditFailureCode::InternalInvariant,
        }
    }
}

#[cfg(target_os = "macos")]
impl X402AmbiguousFailure {
    const fn is_settlement_failure(self) -> bool {
        matches!(
            self,
            Self::UnexpectedStatus
                | Self::MissingPaymentResponse
                | Self::DuplicatePaymentResponse
                | Self::MalformedResponse
                | Self::SettlementRejected
                | Self::UnsupportedSettlementExtension
                | Self::SettlementMismatch
        )
    }
}

#[cfg(target_os = "macos")]
impl std::fmt::Debug for X402ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("X402ExecutionError")
            .field(&self.code())
            .finish()
    }
}

#[cfg(target_os = "macos")]
impl std::fmt::Display for X402ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ApprovalUnavailable => "approved x402 payment is unavailable",
            Self::CandidateChanged => "approved x402 candidate changed",
            Self::Expired => "approved x402 payment expired",
            Self::RequestBindingMismatch => "x402 request binding does not match",
            Self::SigningFailed => "x402 signing failed",
            Self::PayloadRejected => "x402 payment payload was rejected",
            Self::CancelledBeforeSubmission | Self::ConfirmedUnpaid(_) => {
                "x402 payment was not submitted"
            }
            Self::RequirementsChanged(_) => {
                "x402 requirements changed; payment may have occurred and fresh approval is required"
            }
            Self::Ambiguous(_) => "x402 payment may have occurred; it will not be retried",
            Self::PaymentConfirmedContentUnavailable => {
                "x402 payment succeeded but purchased content is unavailable"
            }
        })
    }
}

#[cfg(target_os = "macos")]
impl std::error::Error for X402ExecutionError {}

#[cfg(target_os = "macos")]
fn classify_retry_failure(error: WebBrokerError) -> X402ExecutionError {
    match error {
        WebBrokerError::SubmissionRejected => X402ExecutionError::CancelledBeforeSubmission,
        WebBrokerError::DnsDeadline => {
            X402ExecutionError::ConfirmedUnpaid(X402UnpaidFailure::DnsTimeout)
        }
        WebBrokerError::TargetRejected(_) => {
            X402ExecutionError::ConfirmedUnpaid(X402UnpaidFailure::TargetRejected)
        }
        WebBrokerError::ClientUnavailable => {
            X402ExecutionError::ConfirmedUnpaid(X402UnpaidFailure::ClientUnavailable)
        }
        WebBrokerError::Cancelled => X402ExecutionError::Ambiguous(X402AmbiguousFailure::Cancelled),
        WebBrokerError::ConnectionOrHeaderDeadline => {
            X402ExecutionError::Ambiguous(X402AmbiguousFailure::ConnectionOrHeaderTimeout)
        }
        WebBrokerError::BodyIdleDeadline => {
            X402ExecutionError::Ambiguous(X402AmbiguousFailure::BodyTimeout)
        }
        WebBrokerError::TotalDeadline => {
            X402ExecutionError::Ambiguous(X402AmbiguousFailure::TotalTimeout)
        }
        WebBrokerError::ResponseRejected(_) => {
            X402ExecutionError::Ambiguous(X402AmbiguousFailure::ResponseRejected)
        }
        WebBrokerError::TransportFailed => {
            X402ExecutionError::Ambiguous(X402AmbiguousFailure::TransportLoss)
        }
    }
}

#[cfg(target_os = "macos")]
fn classify_payment_response_failure(error: X402PaymentError) -> X402ExecutionError {
    let failure = match error {
        X402PaymentError::UnexpectedStatus => X402AmbiguousFailure::UnexpectedStatus,
        X402PaymentError::MissingResponse => X402AmbiguousFailure::MissingPaymentResponse,
        X402PaymentError::DuplicateResponse => X402AmbiguousFailure::DuplicatePaymentResponse,
        X402PaymentError::SettlementFailed => X402AmbiguousFailure::SettlementRejected,
        X402PaymentError::UnsupportedExtension => {
            X402AmbiguousFailure::UnsupportedSettlementExtension
        }
        X402PaymentError::CandidateMismatch => X402AmbiguousFailure::SettlementMismatch,
        _ => X402AmbiguousFailure::MalformedResponse,
    };
    X402ExecutionError::Ambiguous(failure)
}

#[cfg(target_os = "macos")]
fn record_x402_audit(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    stage: X402AuditStage,
    result: TransactionAuditResult,
    payment_may_have_occurred: bool,
) {
    record_x402_event(
        core,
        approval,
        review_id,
        TransactionAuditEventKind::X402Payment {
            stage,
            result,
            payment_may_have_occurred,
        },
    );
}

#[cfg(target_os = "macos")]
fn record_x402_approval_presented(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
) {
    let warning_codes = approval
        .presentation()
        .warnings()
        .iter()
        .map(|warning| match warning {
            X402ApprovalWarning::LocalDevelopmentEndpoint => {
                TransactionAuditWarningCode::LocalDevelopmentEndpoint
            }
            X402ApprovalWarning::Testnet => TransactionAuditWarningCode::Testnet,
            X402ApprovalWarning::UnrecognizedRecipient => {
                TransactionAuditWarningCode::UnrecognizedRecipient
            }
            X402ApprovalWarning::ExpiringAuthorization => {
                TransactionAuditWarningCode::ExpiringAuthorization
            }
            X402ApprovalWarning::UnusualPrice => TransactionAuditWarningCode::UnusualPrice,
        })
        .collect();
    record_x402_event(
        core,
        approval,
        review_id,
        TransactionAuditEventKind::ApprovalPresented { warning_codes },
    );
}

#[cfg(target_os = "macos")]
fn record_x402_approval_decision(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    decision: X402ApprovalDecision,
) {
    let audit_decision = match decision {
        X402ApprovalDecision::Approved => ApprovalAuditDecision::Approved,
        X402ApprovalDecision::Denied => ApprovalAuditDecision::Denied,
        X402ApprovalDecision::Dismissed => ApprovalAuditDecision::Dismissed,
        X402ApprovalDecision::Expired => ApprovalAuditDecision::Expired,
    };
    record_x402_event(
        core,
        approval,
        review_id,
        TransactionAuditEventKind::ApprovalDecision {
            decision: audit_decision,
        },
    );
    record_x402_audit(
        core,
        approval,
        review_id,
        X402AuditStage::Approval,
        if decision == X402ApprovalDecision::Approved {
            TransactionAuditResult::Succeeded
        } else {
            TransactionAuditResult::Failed(TransactionAuditFailureCode::ApprovalUnavailable)
        },
        false,
    );
}

#[cfg(target_os = "macos")]
fn record_x402_event(
    core: &AppCore,
    approval: &PreparedX402Approval,
    review_id: &TransactionProposalId,
    data: TransactionAuditEventKind,
) {
    let candidate = approval.candidate();
    let event = TransactionAuditEvent::new(
        approval.wallet_session_id(),
        &review_id.0,
        candidate.decoded().account(),
        candidate.decoded().chain_id(),
        TransactionAuditTransactionType::Eip712,
        Some(*candidate.signing_digest()),
        None,
        TransactionAuditOrigin::X402,
        data,
    );
    if let Ok(event) = event {
        let _ = core.record_transaction_audit(event);
    }
}

#[cfg(target_os = "macos")]
fn unix_now_for_execution() -> Result<u64, X402ExecutionError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| X402ExecutionError::CandidateChanged)
}

#[tauri::command]
pub(crate) async fn review_x402_payment(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, X402ApprovalControl>,
    review_id: TransactionProposalId,
) -> Result<X402ApprovalReviewOutcome, IpcError> {
    review_registered_x402_payment(&app, &core, &control, review_id).await
}

pub(crate) async fn review_registered_x402_payment(
    app: &tauri::AppHandle,
    core: &AppCore,
    control: &X402ApprovalControl,
    review_id: TransactionProposalId,
) -> Result<X402ApprovalReviewOutcome, IpcError> {
    let approval = control.begin(&review_id)?;
    let _active = ActiveReview(control);
    #[cfg(target_os = "macos")]
    record_x402_audit(
        core,
        &approval,
        &review_id,
        X402AuditStage::Preparation,
        TransactionAuditResult::Succeeded,
        false,
    );
    let now_unix = unix_now()?;
    if approval.is_expired_at(now_unix) {
        #[cfg(target_os = "macos")]
        record_x402_approval_decision(core, &approval, &review_id, X402ApprovalDecision::Expired);
        return Ok(outcome(review_id, X402ApprovalDecision::Expired));
    }
    let display = approval.presentation();
    #[cfg(target_os = "macos")]
    record_x402_approval_presented(core, &approval, &review_id);
    let decision =
        show_native_review(app, display.title().to_owned(), display.body().to_owned()).await?;
    let now_unix = unix_now()?;
    let decision = if approval.is_expired_at(now_unix) {
        X402ApprovalDecision::Expired
    } else {
        decision
    };
    #[cfg(target_os = "macos")]
    record_x402_approval_decision(core, &approval, &review_id, decision);
    if decision == X402ApprovalDecision::Approved {
        let commitment = ApprovalCommitment::issue(
            approval.binding_input().map_err(|_| IpcError::internal())?,
            now_unix,
        )
        .map_err(|_| IpcError::internal())?;
        control.retain_approved(&review_id, approval, commitment)?;
    }
    Ok(outcome(review_id, decision))
}

fn outcome(
    review_id: TransactionProposalId,
    decision: X402ApprovalDecision,
) -> X402ApprovalReviewOutcome {
    X402ApprovalReviewOutcome {
        review_id,
        decision,
    }
}

struct ActiveReview<'a>(&'a X402ApprovalControl);

impl Drop for ActiveReview<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

fn unix_now() -> Result<u64, IpcError> {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| IpcError::internal())
}

#[cfg(target_os = "macos")]
async fn show_native_review(
    app: &tauri::AppHandle,
    title: String,
    body: String,
) -> Result<X402ApprovalDecision, IpcError> {
    native::show(app, title, body).await
}

#[cfg(not(target_os = "macos"))]
async fn show_native_review(
    _app: &tauri::AppHandle,
    _title: String,
    _body: String,
) -> Result<X402ApprovalDecision, IpcError> {
    Err(IpcError {
        code: IpcErrorCode::Unsupported,
        message: "Trusted x402 payment review is currently available only on macOS".to_owned(),
        retryable: false,
    })
}

#[cfg(test)]
mod tests;
