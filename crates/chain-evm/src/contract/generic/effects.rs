//! Complete effects from the trusted replay owner, never from child JSON flags.
use super::{
    error::GenericError,
    identity::GenericContractIdentity,
    layout::{self, TypedFieldChange},
    logs::{self, GenericLogEffect},
};
use crate::{contract::UnsignedContractIntent, local_simulation::ProvedState};
use alloy_primitives::{Address, U256};
use evm_simulation::{
    AccountChange, CallObservation, ExecutionStatus, ReplayCall, ReplayCallKind,
    SimulationObservation,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// One changed word, fully covered by source-declared primitive fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericStorageEffect {
    /// Actual persistent storage owner, retaining proxy context.
    pub storage_address: String,
    /// Exact 32-byte storage key.
    pub slot: String,
    /// Complete proved original word.
    pub before: String,
    /// Complete replayed final word.
    pub after: String,
    /// Every changed typed field within this word.
    pub fields: Vec<TypedFieldChange>,
}

/// One committed real native transfer. Delegate apparent value and reverted
/// frame transfers are excluded by the trusted executor's journal ownership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericNativeTransfer {
    /// Exact native debit account.
    pub from: String,
    /// Exact native credit account, including proved empty-code recipients.
    pub to: String,
    /// Exact decimal wei amount; protocol fees remain separately accounted.
    pub value_wei: String,
}

/// Exact executed frame, including failures and enclosing rollback state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenericCallEffect {
    /// Host-owned EVM call scheme; apparent value is not a real-transfer claim.
    pub kind: String,
    /// Persistent storage context.
    pub storage_address: String,
    /// Actual bytecode account.
    pub code_address: String,
    /// EVM caller retained separately from native debit account.
    pub caller: String,
    /// Exact apparent hexadecimal call value.
    pub apparent_value: String,
    /// Complete bounded calldata.
    pub input: String,
    /// Complete bounded return or revert bytes.
    pub output: String,
    /// Success of this frame alone.
    pub succeeded: bool,
    /// Whether all enclosing frames also succeeded.
    pub committed: bool,
    /// Actual frame depth, starting at one.
    pub depth: usize,
}

/// Meaning of the separate protocol-native account projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenericNativeAccountMode {
    /// Exact maintained transaction execution at the Ethereum snapshot.
    ExactTransaction,
    /// Base application phase with the all-signature fee ceiling prepaid.
    /// These are conservative remaining balances, never exact signed fees.
    ConservativeBasePrepayment,
}

pub(super) struct NativeEffectContext {
    pub(super) transfers: Vec<GenericNativeTransfer>,
    pub(super) mode: GenericNativeAccountMode,
}

/// Complete structurally reviewable transition at one proved snapshot.
/// Names and logs are source declarations, not independently proved token or
/// business semantics. Construction stays inside the trusted candidate owner.
pub struct GenericEffects {
    storage: Vec<GenericStorageEffect>,
    accounts: Vec<AccountChange>,
    calls: Vec<CallObservation>,
    logs: Vec<GenericLogEffect>,
    output: Vec<Value>,
    output_bytes: String,
    native_transfers: Vec<GenericNativeTransfer>,
    frames: Vec<GenericCallEffect>,
    native_account_mode: GenericNativeAccountMode,
    material_commitment: String,
}
impl GenericEffects {
    pub(super) fn from_observation(
        intent: &UnsignedContractIntent,
        identity: &GenericContractIdentity,
        state: &ProvedState,
        observation: &SimulationObservation,
        preimages: &[Vec<u8>],
        native: NativeEffectContext,
        frames: &[ReplayCall],
    ) -> Result<Self, GenericError> {
        if observation.status != ExecutionStatus::Succeeded
            || observation.effects_qualified
            || observation.storage.len() > 128
            || observation.accounts.len() > 128
            || observation.calls.len() > 128
        {
            return Err(GenericError::ExecutionRejected);
        }
        let mut writes = BTreeMap::<Address, BTreeMap<U256, U256>>::new();
        for change in &observation.storage {
            let address = change
                .address
                .parse()
                .map_err(|_| GenericError::SnapshotMismatch)?;
            let slot = word(&change.slot)?;
            let before = word(&change.before)?;
            let after = word(&change.after)?;
            if before == after
                || state.slot(address, slot) != Some(before)
                || writes
                    .entry(address)
                    .or_default()
                    .insert(slot, after)
                    .is_some()
            {
                return Err(GenericError::SnapshotMismatch);
            }
        }
        let mut storage = Vec::new();
        for (address, after) in writes {
            let artifact = identity
                .storage_artifact(address)
                .ok_or(GenericError::SourceMismatch)?;
            let typed =
                layout::changes(artifact.storage_layout(), state, address, &after, preimages)?;
            if typed.len() != after.len() {
                return Err(GenericError::OpaqueEffects);
            }
            for (slot, fields) in typed {
                storage.push(GenericStorageEffect {
                    storage_address: format!("{address:#x}"),
                    slot: format!("{slot:#066x}"),
                    before: format!(
                        "{:#066x}",
                        state
                            .slot(address, slot)
                            .ok_or(GenericError::MissingState)?
                    ),
                    after: format!("{:#066x}", after[&slot]),
                    fields,
                });
            }
        }
        let accounts = verify_accounts(state, &observation.accounts)?;
        let output = intent
            .call()
            .decode_result(&observation.output)
            .map_err(|_| GenericError::InvalidAbi)?;
        let logs = logs::decode(identity, &observation.logs)?;
        let native_transfers = native.transfers;
        if native_transfers.len() > 128 {
            return Err(GenericError::Limit);
        }
        let frames = project_frames(frames);
        let material = serde_json::json!({"domain":"minutus-generic-material-v1","storage":storage,"calls":frames,"logs":logs,"output":output,"outputBytes":observation.output,"nativeTransfers":native_transfers});
        let material_commitment = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&material).map_err(|_| GenericError::OpaqueEffects)?)
        );
        Ok(Self {
            storage,
            accounts,
            calls: observation.calls.clone(),
            logs,
            output,
            output_bytes: observation.output.clone(),
            native_transfers,
            frames,
            native_account_mode: native.mode,
            material_commitment,
        })
    }
    /// Complete changed persistent words with exact typed coverage.
    #[must_use]
    pub fn storage_changes(&self) -> &[GenericStorageEffect] {
        &self.storage
    }
    /// Complete native account projection. Consult `native_account_mode` before
    /// interpreting these balances as exact settlement or conservative bounds.
    #[must_use]
    pub fn accounts(&self) -> &[AccountChange] {
        &self.accounts
    }
    /// Whether native balances are exact Ethereum settlement or conservative
    /// Base prepayment bounds. Neither is an inclusion-time guarantee.
    #[must_use]
    pub const fn native_account_mode(&self) -> GenericNativeAccountMode {
        self.native_account_mode
    }
    /// Every committed application native transfer, excluding protocol fees.
    #[must_use]
    pub fn native_transfers(&self) -> &[GenericNativeTransfer] {
        &self.native_transfers
    }
    /// Every executed frame, including revert data and enclosing rollback status.
    #[must_use]
    pub fn frames(&self) -> &[GenericCallEffect] {
        &self.frames
    }
    /// Complete executed call graph, retaining code and storage addresses.
    #[must_use]
    pub fn calls(&self) -> &[CallObservation] {
        &self.calls
    }
    /// Complete raw logs with source-declared ABI interpretation.
    #[must_use]
    pub fn logs(&self) -> &[GenericLogEffect] {
        &self.logs
    }
    /// Canonical compiler-ABI output; return values do not prove a business goal.
    #[must_use]
    pub fn output(&self) -> &[Value] {
        &self.output
    }
    /// Complete canonical raw return bytes.
    #[must_use]
    pub fn output_bytes(&self) -> &str {
        &self.output_bytes
    }
    /// All application material, excluding protocol-only native fees.
    #[must_use]
    pub fn material_commitment(&self) -> &str {
        &self.material_commitment
    }
}
fn project_frames(frames: &[ReplayCall]) -> Vec<GenericCallEffect> {
    frames
        .iter()
        .map(|frame| GenericCallEffect {
            kind: match frame.kind() {
                ReplayCallKind::Call => "call",
                ReplayCallKind::CallCode => "callCode",
                ReplayCallKind::DelegateCall => "delegateCall",
                ReplayCallKind::StaticCall => "staticCall",
            }
            .into(),
            storage_address: frame.target().into(),
            code_address: frame.code_address().into(),
            caller: frame.caller().into(),
            apparent_value: frame.value().into(),
            input: frame.input().into(),
            output: frame.output().into(),
            succeeded: frame.succeeded(),
            committed: frame.committed(),
            depth: frame.depth(),
        })
        .collect()
}
fn verify_accounts(
    state: &ProvedState,
    changes: &[AccountChange],
) -> Result<Vec<AccountChange>, GenericError> {
    let mut accounts = BTreeMap::new();
    for change in changes {
        let address: Address = change
            .address
            .parse()
            .map_err(|_| GenericError::SnapshotMismatch)?;
        let account = state.account(address).ok_or(GenericError::MissingState)?;
        if quantity(&change.balance_before)? != account.balance()
            || change.nonce_before != account.nonce()
            || accounts.insert(address, change.clone()).is_some()
        {
            return Err(GenericError::SnapshotMismatch);
        }
        quantity(&change.balance)?;
    }
    Ok(accounts.into_values().collect())
}
fn word(value: &str) -> Result<U256, GenericError> {
    if value.len() != 66 || !value.starts_with("0x") {
        return Err(GenericError::SnapshotMismatch);
    }
    U256::from_str_radix(&value[2..], 16).map_err(|_| GenericError::SnapshotMismatch)
}
fn quantity(value: &str) -> Result<U256, GenericError> {
    let text = value
        .strip_prefix("0x")
        .ok_or(GenericError::SnapshotMismatch)?;
    if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
        return Err(GenericError::SnapshotMismatch);
    }
    U256::from_str_radix(text, 16).map_err(|_| GenericError::SnapshotMismatch)
}
