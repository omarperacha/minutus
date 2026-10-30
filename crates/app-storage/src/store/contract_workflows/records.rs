use crate::StorageError;

/// Opaque host-created database identity; cannot be deserialised from model input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractWorkflowId(pub(super) i64);
/// One consumed host step identity. It contains no approval or signing handle.
#[derive(Debug)]
pub struct ContractStepClaim {
    pub(super) workflow: ContractWorkflowId,
    pub(super) index: u8,
    pub(super) commitment: String,
}
impl ContractStepClaim {
    /// Host workflow identity; an identifier alone grants no execution authority.
    #[must_use]
    pub const fn workflow(&self) -> &ContractWorkflowId {
        &self.workflow
    }
    /// Immutable ordered step position.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.index
    }
    /// Digest of semantic intent, excluding fresh transaction financial facts.
    #[must_use]
    pub fn intent_commitment(&self) -> &str {
        &self.commitment
    }
}
/// Safe journal facts; execution and task success remain separate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractWorkflowStep {
    /// Host-owned ordered index.
    pub index: u8,
    /// Digest of exact intended step facts, never a signing digest.
    pub commitment: String,
    /// Last durable execution/postcondition status.
    pub state: ContractStepState,
    /// Transaction identity when known; uncertainty may still lack a hash.
    pub transaction_hash: Option<String>,
    /// Host-verified postcondition evidence commitment, only for Verified.
    pub evidence_commitment: Option<String>,
}
/// Durable execution states. Stored facts never replace fresh native authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractStepState {
    /// Never attempted.
    Planned,
    /// Attempt consumed; native review/signing may be in progress.
    Reviewing,
    /// Transport admission may have occurred; reconcile before any new action.
    Dispatching,
    /// Hash acknowledged; receipt/outcome not established.
    Submitted,
    /// Non-reverting execution observed; intended task outcome still unknown.
    ConfirmedUnverified,
    /// Host separately verified intended effects at a canonical block.
    Verified,
    /// Human denied the attempt; terminal.
    Denied,
    /// Cancellation; terminal.
    Cancelled,
    /// Failed; prerequisite effects may remain.
    Failed,
    /// Possible submission/reorg/replacement; no automatic retry.
    Uncertain,
    /// Restart retired pending native authority; no automatic resume.
    Interrupted,
}
impl ContractStepState {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Reviewing => "reviewing",
            Self::Dispatching => "dispatching",
            Self::Submitted => "submitted",
            Self::ConfirmedUnverified => "confirmed_unverified",
            Self::Verified => "verified",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Interrupted => "interrupted",
        }
    }
    pub(super) fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "planned" => Ok(Self::Planned),
            "reviewing" => Ok(Self::Reviewing),
            "dispatching" => Ok(Self::Dispatching),
            "submitted" => Ok(Self::Submitted),
            "confirmed_unverified" => Ok(Self::ConfirmedUnverified),
            "verified" => Ok(Self::Verified),
            "denied" => Ok(Self::Denied),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "uncertain" => Ok(Self::Uncertain),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(StorageError::CorruptDatabase),
        }
    }
    pub(super) const fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Reviewing,
                Self::Dispatching | Self::Denied | Self::Cancelled | Self::Failed
            ) | (Self::Dispatching, Self::Submitted | Self::Uncertain)
                | (
                    Self::Submitted,
                    Self::ConfirmedUnverified | Self::Failed | Self::Uncertain
                )
                | (Self::ConfirmedUnverified | Self::Verified, Self::Uncertain)
                | (Self::Uncertain, Self::ConfirmedUnverified | Self::Failed)
        )
    }
}
pub(super) fn validate_digest(value: &str) -> Result<(), StorageError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StorageError::InvalidRecord("invalid workflow commitment"));
    }
    Ok(())
}
pub(super) fn validate_hash(value: &str) -> Result<(), StorageError> {
    value
        .strip_prefix("0x")
        .ok_or(StorageError::InvalidRecord(
            "invalid workflow transaction hash",
        ))
        .and_then(validate_digest)
}

pub(super) fn load_workflow(
    connection: &rusqlite::Connection,
    id: &ContractWorkflowId,
) -> Result<Vec<ContractWorkflowStep>, StorageError> {
    use rusqlite::{OptionalExtension, params};
    let manifest:Option<(String,String)>=connection.query_row("SELECT scope_commitment,step_commitments_json FROM contract_workflows WHERE id=?1 AND length(step_commitments_json)<=2048",params![id.0],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let (scope, manifest) = manifest.ok_or(StorageError::CorruptDatabase)?;
    validate_digest(&scope)?;
    let manifest: Vec<String> =
        serde_json::from_str(&manifest).map_err(|_| StorageError::CorruptDatabase)?;
    if manifest.is_empty() || manifest.len() > 16 {
        return Err(StorageError::CorruptDatabase);
    }
    for commitment in &manifest {
        validate_digest(commitment)?;
    }
    let mut statement=connection.prepare("SELECT step_index,commitment,state,transaction_hash,evidence_commitment FROM contract_workflow_steps WHERE workflow_id=?1 ORDER BY step_index LIMIT 17")?;
    let mut rows = statement.query(params![id.0])?;
    let mut steps = Vec::new();
    while let Some(row) = rows.next()? {
        let index: u8 = row.get(0)?;
        let commitment: String = row.get(1)?;
        if usize::from(index) != steps.len() || manifest.get(steps.len()) != Some(&commitment) {
            return Err(StorageError::CorruptDatabase);
        }
        let state = ContractStepState::parse(&row.get::<_, String>(2)?)?;
        let hash: Option<String> = row.get(3)?;
        let evidence: Option<String> = row.get(4)?;
        if let Some(hash) = &hash {
            validate_hash(hash)?;
        }
        if let Some(evidence) = &evidence {
            validate_digest(evidence)?;
        }
        let requires_hash = matches!(
            state,
            ContractStepState::Submitted
                | ContractStepState::ConfirmedUnverified
                | ContractStepState::Verified
        );
        let forbids_hash = matches!(
            state,
            ContractStepState::Planned
                | ContractStepState::Reviewing
                | ContractStepState::Denied
                | ContractStepState::Cancelled
                | ContractStepState::Interrupted
        );
        if (requires_hash && hash.is_none())
            || (forbids_hash && hash.is_some())
            || (state == ContractStepState::Verified) != evidence.is_some()
        {
            return Err(StorageError::CorruptDatabase);
        }
        steps.push(ContractWorkflowStep {
            index,
            commitment,
            state,
            transaction_hash: hash,
            evidence_commitment: evidence,
        });
    }
    if steps.len() != manifest.len() {
        return Err(StorageError::CorruptDatabase);
    }
    super::scoped::validate_context(connection, id, &steps)?;
    Ok(steps)
}
