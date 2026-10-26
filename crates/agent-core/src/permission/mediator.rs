use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};

use super::binding::AuthorizationSource;
use crate::{
    ApprovalRequirement, DurableCapability, ExecutionAuthorization, ExecutionLedgerError,
    LedgerPermissionDecision, PermissionBinding, PermissionResourceKind, PermissionResourceScope,
    RiskLevel, ValidatedToolRequest, ledger_types::validate_id,
};

const SNAPSHOT_VERSION: u16 = 1;
const MAX_PERMISSION_RECORDS: usize = 128;

/// Safe trusted UI projection of one pending permission request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPrompt {
    /// Host-generated permission request ID.
    pub permission_id: String,
    /// Exact adapter call ID.
    pub call_id: String,
    /// Trusted registry tool name.
    pub tool_name: String,
    /// Trusted risk classification.
    pub risk: RiskLevel,
    /// Owning project ID.
    pub project_id: String,
    /// Resource category.
    pub resource_kind: PermissionResourceKind,
    /// Bounded non-authority resource labels.
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingPermission {
    permission_id: String,
    binding: PermissionBinding,
    risk: RiskLevel,
}

/// Persistable host-owned permission mediation state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionMediatorSnapshot {
    version: u16,
    pending: Vec<PendingPermission>,
    terminal: BTreeMap<String, LedgerPermissionDecision>,
}

/// Trusted terminal receipt for one exact interactive decision.
#[derive(Debug, Eq, PartialEq)]
pub struct PermissionDecisionReceipt {
    /// Host-generated permission request ID.
    pub permission_id: String,
    /// Exact request/resource binding that was decided.
    pub binding: PermissionBinding,
    /// Distinct trusted terminal decision.
    pub decision: LedgerPermissionDecision,
    /// One-shot authorization, present only for approval.
    pub authorization: Option<ExecutionAuthorization>,
}

/// Permission mediation failures that fail closed before execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionMediationError {
    /// A durable capability-only request is not currently covered.
    CapabilityNotGranted,
    /// The tool approval policy does not match the requested mediation path.
    WrongApprovalPolicy,
    /// The permission request ID is unknown, terminal, or was replayed.
    StaleOrReplayed,
    /// The supplied request, arguments, capability, or resource scope changed.
    BindingMismatch,
    /// Pending was supplied where a terminal trusted decision was required.
    NonTerminalDecision,
    /// A host-owned permission record is malformed or a bound was exceeded.
    InvalidRecord,
}

impl fmt::Display for PermissionMediationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityNotGranted => formatter.write_str("durable capability is not granted"),
            Self::WrongApprovalPolicy => formatter.write_str("tool uses another approval policy"),
            Self::StaleOrReplayed => formatter.write_str("permission request is stale or replayed"),
            Self::BindingMismatch => formatter.write_str("permission request binding changed"),
            Self::NonTerminalDecision => formatter.write_str("permission decision is not terminal"),
            Self::InvalidRecord => formatter.write_str("permission mediation state is invalid"),
        }
    }
}

impl Error for PermissionMediationError {}

/// Host-owned one-shot mediator for durable capabilities and interactive approval.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PermissionMediator {
    pending: BTreeMap<String, PendingPermission>,
    terminal: BTreeMap<String, LedgerPermissionDecision>,
}

impl PermissionMediator {
    /// Authorizes a capability-only request that explicitly needs no project authority.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry entry requires approval or any capability.
    pub fn authorize_without_capability(
        &self,
        request: &ValidatedToolRequest,
        scope: PermissionResourceScope,
    ) -> Result<ExecutionAuthorization, PermissionMediationError> {
        if request.definition().approval() != ApprovalRequirement::CapabilityOnly {
            return Err(PermissionMediationError::WrongApprovalPolicy);
        }
        if request.definition().capability() != crate::ProjectCapability::NoProjectAuthority {
            return Err(PermissionMediationError::CapabilityNotGranted);
        }
        let binding =
            PermissionBinding::from_request(request, scope).map_err(map_invalid_record)?;
        Ok(ExecutionAuthorization {
            binding,
            source: AuthorizationSource::HostPolicy,
        })
    }

    /// Authorizes a capability-only request when one exact durable grant covers it.
    ///
    /// # Errors
    ///
    /// Returns an error for interactive tools or stale/mismatched capability scope.
    pub fn authorize_capability(
        &self,
        request: &ValidatedToolRequest,
        scope: PermissionResourceScope,
        capability: &DurableCapability,
    ) -> Result<ExecutionAuthorization, PermissionMediationError> {
        if request.definition().approval() != ApprovalRequirement::CapabilityOnly {
            return Err(PermissionMediationError::WrongApprovalPolicy);
        }
        if request.definition().capability() == crate::ProjectCapability::NoProjectAuthority {
            return Err(PermissionMediationError::CapabilityNotGranted);
        }
        if !capability.covers(request.definition().capability(), &scope) {
            return Err(PermissionMediationError::CapabilityNotGranted);
        }
        let binding =
            PermissionBinding::from_request(request, scope).map_err(map_invalid_record)?;
        Ok(ExecutionAuthorization {
            binding,
            source: AuthorizationSource::DurableCapability,
        })
    }

    /// Opens trusted UI mediation for one exact interactive request.
    ///
    /// # Errors
    ///
    /// Returns an error for capability-only tools, malformed IDs, reuse, or limits.
    pub fn request_interactive(
        &mut self,
        permission_id: impl Into<String>,
        request: &ValidatedToolRequest,
        scope: PermissionResourceScope,
    ) -> Result<PermissionPrompt, PermissionMediationError> {
        if request.definition().approval() != ApprovalRequirement::Interactive {
            return Err(PermissionMediationError::WrongApprovalPolicy);
        }
        let permission_id = permission_id.into();
        validate_id("permission request ID", &permission_id).map_err(map_invalid_record)?;
        if self.pending.contains_key(&permission_id) || self.terminal.contains_key(&permission_id) {
            return Err(PermissionMediationError::StaleOrReplayed);
        }
        if self.pending.len() + self.terminal.len() >= MAX_PERMISSION_RECORDS {
            return Err(PermissionMediationError::InvalidRecord);
        }
        let binding =
            PermissionBinding::from_request(request, scope).map_err(map_invalid_record)?;
        let pending = PendingPermission {
            permission_id: permission_id.clone(),
            binding,
            risk: request.definition().risk(),
        };
        let prompt = prompt_from_pending(&pending);
        self.pending.insert(permission_id, pending);
        Ok(prompt)
    }

    /// Applies one terminal trusted UI decision to the unchanged pending request.
    ///
    /// # Errors
    ///
    /// Returns an error for model-like pending decisions, modification, or replay.
    pub fn decide(
        &mut self,
        permission_id: &str,
        request: &ValidatedToolRequest,
        scope: &PermissionResourceScope,
        decision: LedgerPermissionDecision,
    ) -> Result<PermissionDecisionReceipt, PermissionMediationError> {
        if decision == LedgerPermissionDecision::Pending {
            return Err(PermissionMediationError::NonTerminalDecision);
        }
        let pending = self
            .pending
            .get(permission_id)
            .ok_or(PermissionMediationError::StaleOrReplayed)?;
        if !pending.binding.matches_request(request, scope) {
            return Err(PermissionMediationError::BindingMismatch);
        }
        let pending = self
            .pending
            .remove(permission_id)
            .ok_or(PermissionMediationError::StaleOrReplayed)?;
        self.terminal.insert(permission_id.to_owned(), decision);
        let authorization =
            (decision == LedgerPermissionDecision::Approved).then(|| ExecutionAuthorization {
                binding: pending.binding.clone(),
                source: AuthorizationSource::InteractiveApproval,
            });
        Ok(PermissionDecisionReceipt {
            permission_id: permission_id.to_owned(),
            binding: pending.binding,
            decision,
            authorization,
        })
    }

    /// Returns a safe UI prompt for a pending host-owned request.
    #[must_use]
    pub fn prompt(&self, permission_id: &str) -> Option<PermissionPrompt> {
        self.pending.get(permission_id).map(prompt_from_pending)
    }

    /// Exports bounded restart state without model text or raw arguments.
    #[must_use]
    pub fn snapshot(&self) -> PermissionMediatorSnapshot {
        PermissionMediatorSnapshot {
            version: SNAPSHOT_VERSION,
            pending: self.pending.values().cloned().collect(),
            terminal: self.terminal.clone(),
        }
    }

    /// Restores trusted mediation state after validating every binding and ID.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported, duplicate, malformed, or oversized state.
    pub fn restore(snapshot: PermissionMediatorSnapshot) -> Result<Self, PermissionMediationError> {
        if snapshot.version != SNAPSHOT_VERSION
            || snapshot.pending.len() + snapshot.terminal.len() > MAX_PERMISSION_RECORDS
        {
            return Err(PermissionMediationError::InvalidRecord);
        }
        let mut pending = BTreeMap::new();
        for record in snapshot.pending {
            validate_id("permission request ID", &record.permission_id)
                .map_err(map_invalid_record)?;
            record.binding.validate().map_err(map_invalid_record)?;
            if snapshot.terminal.contains_key(&record.permission_id)
                || pending
                    .insert(record.permission_id.clone(), record)
                    .is_some()
            {
                return Err(PermissionMediationError::InvalidRecord);
            }
        }
        for (permission_id, decision) in &snapshot.terminal {
            validate_id("permission request ID", permission_id).map_err(map_invalid_record)?;
            if *decision == LedgerPermissionDecision::Pending {
                return Err(PermissionMediationError::InvalidRecord);
            }
        }
        Ok(Self {
            pending,
            terminal: snapshot.terminal,
        })
    }
}

fn prompt_from_pending(pending: &PendingPermission) -> PermissionPrompt {
    let scope = pending.binding.resource_scope();
    PermissionPrompt {
        permission_id: pending.permission_id.clone(),
        call_id: pending.binding.call_id().to_owned(),
        tool_name: pending.binding.tool_name().to_owned(),
        risk: pending.risk,
        project_id: scope.project_id().to_owned(),
        resource_kind: scope.kind(),
        resources: scope.resources().to_vec(),
    }
}

fn map_invalid_record(_: ExecutionLedgerError) -> PermissionMediationError {
    PermissionMediationError::InvalidRecord
}
