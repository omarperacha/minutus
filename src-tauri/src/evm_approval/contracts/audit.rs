//! Public native review/signing/broadcast audit; never proof or approval authority.
use crate::evm_approval::{ApprovalOrigin, presentation::ApprovalEvidence};
use app_core::{
    AppCore, ApprovalAuditDecision, ApprovalFinalizationAuditOutcome,
    OwnerAuthorizationAuditOutcome, TransactionAuditEventKind, TransactionAuditFailureCode,
    TransactionAuditResult, TransactionAuditWarningCode,
};
use ipc_types::{EvmApprovalDecision, IpcError, IpcErrorCode};

pub(super) struct Audit<'a> {
    pub(super) core: &'a AppCore,
    pub(super) evidence: &'a ApprovalEvidence,
    pub(super) session: &'a str,
    pub(super) origin: ApprovalOrigin,
}
impl Audit<'_> {
    fn record(&self, event: TransactionAuditEventKind) {
        if let Ok(event) = self.evidence.audit_event(self.session, self.origin, event) {
            let _ = self.core.record_transaction_audit(event);
        }
    }
    pub(super) fn presented(&self, warning_codes: Vec<TransactionAuditWarningCode>) {
        self.record(TransactionAuditEventKind::ApprovalPresented { warning_codes });
    }
    pub(super) fn decision(&self, decision: EvmApprovalDecision) {
        self.record(TransactionAuditEventKind::ApprovalDecision {
            decision: match decision {
                EvmApprovalDecision::Approved => ApprovalAuditDecision::Approved,
                EvmApprovalDecision::Denied => ApprovalAuditDecision::Denied,
                EvmApprovalDecision::Dismissed => ApprovalAuditDecision::Dismissed,
            },
        });
    }
    pub(super) fn authorization<T>(&self, result: &Result<T, IpcError>) {
        self.record(TransactionAuditEventKind::OwnerAuthorization {
            outcome: match result {
                Ok(_) => OwnerAuthorizationAuditOutcome::Succeeded,
                Err(error) if error.code == IpcErrorCode::Cancelled => {
                    OwnerAuthorizationAuditOutcome::Cancelled
                }
                Err(error) if error.message.contains("denied") => {
                    OwnerAuthorizationAuditOutcome::Denied
                }
                Err(_) => OwnerAuthorizationAuditOutcome::Unavailable,
            },
        });
        self.record(TransactionAuditEventKind::ApprovalFinalization {
            outcome: if result.is_ok() {
                ApprovalFinalizationAuditOutcome::Consumed
            } else {
                ApprovalFinalizationAuditOutcome::Invalidated
            },
        });
        self.record(TransactionAuditEventKind::Signing {
            result: result.as_ref().map_or(
                TransactionAuditResult::Failed(TransactionAuditFailureCode::SigningFailed),
                |_| TransactionAuditResult::Succeeded,
            ),
        });
    }
    pub(super) fn broadcast<T>(&self, result: &Result<T, IpcError>) {
        self.record(TransactionAuditEventKind::Broadcast {
            result: result.as_ref().map_or(
                TransactionAuditResult::Failed(TransactionAuditFailureCode::BroadcastFailed),
                |_| TransactionAuditResult::Succeeded,
            ),
        });
    }
}
