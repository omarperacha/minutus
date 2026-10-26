//! Content-free classification of permission, plan relationship, and progress.
//!
//! These coordinates describe what the host observed. They never authorize an
//! operation, reinterpret model arguments, or establish a successful outcome.

use ipc_types::{IpcError, IpcErrorCode, TaskActivityStatus, ToolOutcome};

use super::ChatToolExchange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationPermissionOutcome {
    Admitted,
    Denied,
    Cancelled,
    NotEvaluated,
}

impl OperationPermissionOutcome {
    #[cfg(debug_assertions)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::NotEvaluated => "not_evaluated",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationStepRelationship {
    PlanControl,
    PreferredCurrent,
    AdaptiveCurrent,
    ReadyIndependent,
    FutureStep,
    OutcomeRecovery,
    Unrelated,
    NoRemainingStep,
}

impl OperationStepRelationship {
    #[cfg(debug_assertions)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PlanControl => "plan_control",
            Self::PreferredCurrent => "preferred_current",
            Self::AdaptiveCurrent => "adaptive_current",
            Self::ReadyIndependent => "ready_independent",
            Self::FutureStep => "future_step",
            Self::OutcomeRecovery => "outcome_recovery",
            Self::Unrelated => "unrelated",
            Self::NoRemainingStep => "no_remaining_step",
        }
    }

    pub(crate) const fn can_advance_current(self) -> bool {
        matches!(
            self,
            Self::PreferredCurrent
                | Self::AdaptiveCurrent
                | Self::ReadyIndependent
                | Self::OutcomeRecovery
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationProgressContribution {
    Advanced,
    Refinement,
    Blocked,
    None,
}

impl OperationProgressContribution {
    #[cfg(debug_assertions)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::Refinement => "refinement",
            Self::Blocked => "blocked",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationAdmissionRecord {
    pub(crate) call_id: String,
    pub(crate) permission: OperationPermissionOutcome,
    pub(crate) relationship: OperationStepRelationship,
    pub(crate) progress: OperationProgressContribution,
}

pub(crate) fn permission_outcome(
    exchange: &Result<ChatToolExchange, IpcError>,
) -> OperationPermissionOutcome {
    let (error, reached_execution) = match exchange {
        Err(error) => (error, false),
        Ok(exchange) => match &exchange.result.outcome {
            ToolOutcome::Completed { .. } => return OperationPermissionOutcome::Admitted,
            ToolOutcome::Failed { error } => (
                error,
                exchange.activity.status != TaskActivityStatus::Partial,
            ),
        },
    };
    match &error.code {
        IpcErrorCode::PermissionDenied => OperationPermissionOutcome::Denied,
        IpcErrorCode::Cancelled => OperationPermissionOutcome::Cancelled,
        IpcErrorCode::ScopeCorrection | IpcErrorCode::PlanMismatch => {
            OperationPermissionOutcome::NotEvaluated
        }
        _ if reached_execution => OperationPermissionOutcome::Admitted,
        _ => OperationPermissionOutcome::NotEvaluated,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ipc_types::{
        TaskActivity, TaskActivityKind, ToolCallId, ToolName, ToolRequest, ToolResult,
    };

    use super::*;

    #[test]
    fn closed_permission_errors_are_not_misreported_as_execution_admission() {
        assert_eq!(
            permission_outcome(&Err(IpcError::permission_denied("denied"))),
            OperationPermissionOutcome::Denied
        );
        assert_eq!(
            permission_outcome(&Err(IpcError::plan_mismatch("mismatch"))),
            OperationPermissionOutcome::NotEvaluated
        );
    }

    #[test]
    fn pre_execution_argument_corrections_are_not_permission_admissions() {
        let exchange = ChatToolExchange {
            request: ToolRequest {
                call_id: ToolCallId("invalid".to_owned()),
                name: ToolName::FilesystemReadText,
                arguments: BTreeMap::new(),
            },
            result: ToolResult {
                call_id: ToolCallId("invalid".to_owned()),
                outcome: ToolOutcome::Failed {
                    error: IpcError::invalid_request("path is required"),
                },
            },
            activity: TaskActivity {
                id: "invalid".to_owned(),
                kind: TaskActivityKind::FileRead,
                title: "Correct workspace operation".to_owned(),
                detail: None,
                status: TaskActivityStatus::Partial,
            },
            workspace_grant_id: None,
            undo_changes: Vec::new(),
            inspected_asset_id: None,
            computation_reuse_digest: None,
        };
        assert_eq!(
            permission_outcome(&Ok(exchange)),
            OperationPermissionOutcome::NotEvaluated
        );
    }
}
