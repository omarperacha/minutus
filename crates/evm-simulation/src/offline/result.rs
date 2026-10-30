//! Signed worker output is tainted. Only fixed expected material effects and
//! independently reconstructed fee settlement may release a host-only receipt.
use super::{database::FrozenDb, envelope::Envelope, fees, snapshot::FrozenBaseRequest};
use crate::{
    AccountChange, CallObservation, ExecutionStatus, LogObservation, SimulationError,
    SimulationObservation, StorageChange, admission,
};
use alloy_primitives::{Address, U256, address};
use std::collections::{BTreeMap, BTreeSet};

/// Trusted host certificate for the direct, zero-native-value, fee-insensitive
/// runtime family. Construct from proof-checked source semantics, never child
/// output. This boundary does not itself establish that source qualification.
pub struct OfflineMaterialExpectation {
    storage: Vec<StorageChange>,
    logs: Vec<LogObservation>,
    output: String,
}
impl OfflineMaterialExpectation {
    /// Records independently derived ordered logs, changed words and return bytes.
    ///
    /// # Errors
    /// Rejects unbounded certificate material.
    pub fn new(
        storage: Vec<StorageChange>,
        logs: Vec<LogObservation>,
        output: String,
    ) -> Result<Self, SimulationError> {
        if storage.len() > 128
            || logs.len() > 64
            || output.len() > 16_386
            || serde_json::to_vec(&(&storage, &logs, &output))
                .map_err(|_| SimulationError::Protocol)?
                .len()
                > crate::MAX_FRAME
        {
            return Err(SimulationError::Limit);
        }
        Ok(Self {
            storage,
            logs,
            output,
        })
    }
}
// No Debug, Serialize, raw output getter, or conversion to online observations.
/// Opaque tainted signed-run output. It cannot qualify approval or be projected.
pub struct OfflineExecution {
    pub(super) observation: SimulationObservation,
    pub(super) envelope: Envelope,
    pub(super) snapshot: FrozenBaseRequest,
}
/// Host-only validated settlement receipt, rebuilt without raw child payloads.
/// This type carries no signed bytes and cannot approve or send a transaction.
pub struct ValidatedOfflineExecution {
    hash: String,
    snapshot: String,
}
impl ValidatedOfflineExecution {
    /// Hash of the independently decoded genuine envelope.
    #[must_use]
    pub fn transaction_hash(&self) -> &str {
        &self.hash
    }
    /// Frozen proof-state commitment used for the check.
    #[must_use]
    pub fn snapshot_commitment(&self) -> &str {
        &self.snapshot
    }
}
impl OfflineExecution {
    /// Checks every child-visible material field against the independent host
    /// certificate and native fee computation. Rejected payloads stay opaque.
    ///
    /// # Errors
    /// Rejects reverts, extra calls/effects, missing state, nonce/balance errors,
    /// mismatched commitments and any fee beyond the approved snapshot ceiling.
    pub fn validate(
        self,
        expected: &OfflineMaterialExpectation,
    ) -> Result<ValidatedOfflineExecution, SimulationError> {
        let request = &self.snapshot.request;
        let observation = &self.observation;
        let engine = match admission::validate(request)? {
            admission::Rules::BaseCobalt => "base1.5.1/Cobalt",
            admission::Rules::BaseDenim => "base1.5.1/Denim",
            _ => return Err(SimulationError::UnsupportedRules),
        };
        if observation.request_commitment != crate::protocol::commitment(request)?
            || observation.block_hash != request.block.hash
            || observation.engine != engine
            || observation.effects_qualified
            || observation.status != ExecutionStatus::Succeeded
            || request.transaction.value != "0x0"
        {
            return Err(SimulationError::InconsistentState);
        }
        let call = CallObservation {
            target: request.transaction.target.clone(),
            code_address: request.transaction.target.clone(),
            caller: request.transaction.caller.clone(),
            value: "0x0".into(),
            depth: 1,
        };
        if !same(&observation.calls, &vec![call])?
            || !same(&observation.logs, &expected.logs)?
            || observation.output != expected.output
            || !same(&observation.storage, &expected.storage)?
        {
            return Err(SimulationError::InconsistentState);
        }
        let db = check_writes(&self.snapshot, expected)?;
        let fees = fees::exact(&self.snapshot, &self.envelope, observation.gas_used)?;
        let mut balances = BTreeMap::<Address, (U256, u64)>::new();
        for (address, account) in &db.accounts {
            balances.insert(
                *address,
                account
                    .as_ref()
                    .map_or((U256::ZERO, 0), |value| (value.balance, value.nonce)),
            );
        }
        let caller = admission::address(&request.transaction.caller)?;
        let sender = balances
            .get_mut(&caller)
            .ok_or(SimulationError::StateUnavailable)?;
        if sender.1 != request.transaction.nonce {
            return Err(SimulationError::InconsistentState);
        }
        sender.0 = sender
            .0
            .checked_sub(fees.total)
            .ok_or(SimulationError::TransactionRejected)?;
        sender.1 = sender
            .1
            .checked_add(1)
            .ok_or(SimulationError::TransactionRejected)?;
        for (address, amount) in [
            (
                address!("420000000000000000000000000000000000001a"),
                fees.l1,
            ),
            (
                address!("4200000000000000000000000000000000000019"),
                fees.base,
            ),
            (
                address!("420000000000000000000000000000000000001b"),
                fees.operator,
            ),
            (admission::address(&request.block.beneficiary)?, fees.tip),
        ] {
            let account = balances
                .get_mut(&address)
                .ok_or(SimulationError::StateUnavailable)?;
            account.0 = account
                .0
                .checked_add(amount)
                .ok_or(SimulationError::Limit)?;
        }
        check_accounts(observation, &db, balances)?;
        Ok(ValidatedOfflineExecution {
            hash: self.envelope.hash,
            snapshot: self.snapshot.commitment,
        })
    }
}
fn check_account(
    change: &AccountChange,
    original: (U256, u64),
    final_state: (U256, u64),
) -> Result<(), SimulationError> {
    if admission::quantity(&change.balance_before)? != original.0
        || change.nonce_before != original.1
        || admission::quantity(&change.balance)? != final_state.0
        || change.nonce != final_state.1
    {
        return Err(SimulationError::InconsistentState);
    }
    Ok(())
}
fn same(a: &impl serde::Serialize, b: &impl serde::Serialize) -> Result<bool, SimulationError> {
    Ok(
        serde_json::to_vec(a).map_err(|_| SimulationError::Protocol)?
            == serde_json::to_vec(b).map_err(|_| SimulationError::Protocol)?,
    )
}

fn check_writes(
    snapshot: &FrozenBaseRequest,
    expected: &OfflineMaterialExpectation,
) -> Result<FrozenDb, SimulationError> {
    // Every approved write must bind to a genuine preloaded word. No writes
    // outside the direct target or duplicates can enter through a certificate.
    let mut db = FrozenDb::new(snapshot)?;
    let mut seen = BTreeSet::new();
    for change in &expected.storage {
        let address = admission::address(&change.address)?;
        let slot = U256::from_be_bytes(admission::hash(&change.slot)?.0);
        let before = U256::from_be_bytes(admission::hash(&change.before)?.0);
        let after = U256::from_be_bytes(admission::hash(&change.after)?.0);
        if address != admission::address(&snapshot.request.transaction.target)?
            || !seen.insert((address, slot))
            || before == after
            || revm_base::Database::storage(&mut db, address, slot)? != before
        {
            return Err(SimulationError::InconsistentState);
        }
    }
    Ok(db)
}

fn check_accounts(
    observation: &SimulationObservation,
    db: &FrozenDb,
    balances: BTreeMap<Address, (U256, u64)>,
) -> Result<(), SimulationError> {
    let mut actual = BTreeMap::new();
    for change in &observation.accounts {
        let address = admission::address(&change.address)?;
        if actual.insert(address, change).is_some() {
            return Err(SimulationError::InconsistentState);
        }
        let original = db
            .accounts
            .get(&address)
            .ok_or(SimulationError::StateUnavailable)?
            .as_ref()
            .map_or((U256::ZERO, 0), |value| (value.balance, value.nonce));
        let final_state = *balances
            .get(&address)
            .ok_or(SimulationError::StateUnavailable)?;
        check_account(change, original, final_state)?;
    }
    for (address, final_state) in balances {
        let original = db
            .accounts
            .get(&address)
            .ok_or(SimulationError::StateUnavailable)?
            .as_ref()
            .map_or((U256::ZERO, 0), |value| (value.balance, value.nonce));
        if final_state != original && !actual.contains_key(&address) {
            return Err(SimulationError::InconsistentState);
        }
    }
    Ok(())
}
