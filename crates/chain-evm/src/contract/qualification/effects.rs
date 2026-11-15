//! Exact Ethereum execution and native fee settlement over reviewed material effects.
use super::{
    AllowanceEffect, ContractAction, QualificationError, QualifiedMaterialEffects,
    TokenBalanceEffect, action::parse_address,
};
use crate::local_simulation::ProvedSimulation;
use alloy_primitives::{Address, U256};
use evm_simulation::ExecutionStatus;
use std::{collections::BTreeMap, str::FromStr};

/// Complete profile-specific effects, privately minted from a sealed proved run.
/// There is no constructor from worker JSON, public flags, events or digests.
pub struct QualifiedEffects {
    material: QualifiedMaterialEffects,
    execution_fee_wei: String,
    maximum_execution_fee_wei: String,
}
impl QualifiedEffects {
    pub(super) fn from_run(
        run: &ProvedSimulation,
        action: ContractAction,
    ) -> Result<Self, QualificationError> {
        validate_frame(run)?;
        let target = parse_address(&run.request().transaction.target)?;
        let material = QualifiedMaterialEffects::from_state(run.state(), target, action)?;
        material.verify_observation(run.observation())?;
        let execution_fee_wei = verify_native(run)?.to_string();
        let maximum_execution_fee_wei = U256::from(run.request().transaction.gas_limit)
            .checked_mul(U256::from(run.request().transaction.max_fee_per_gas))
            .ok_or(QualificationError::Arithmetic)?
            .to_string();
        Ok(Self {
            material,
            execution_fee_wei,
            maximum_execution_fee_wei,
        })
    }
    /// Source-bound exact action, not arbitrary decoded ABI labels.
    #[must_use]
    pub const fn action(&self) -> &ContractAction {
        self.material.action()
    }
    /// Every affected holder, including a coalesced self-transfer/no-change case.
    #[must_use]
    pub fn balances(&self) -> &[TokenBalanceEffect] {
        self.material.balances()
    }
    /// Exact allowance state when applicable, including unchanged maximum allowance.
    #[must_use]
    pub const fn allowance(&self) -> Option<&AllowanceEffect> {
        self.material.allowance()
    }
    /// Exact local execution fee at the observed state; not an inclusion guarantee.
    #[must_use]
    pub fn execution_fee_wei(&self) -> &str {
        &self.execution_fee_wei
    }
    /// Signed execution gas/fee ceiling, separate from rollup additional fees.
    #[must_use]
    pub fn maximum_execution_fee_wei(&self) -> &str {
        &self.maximum_execution_fee_wei
    }
}

fn validate_frame(run: &ProvedSimulation) -> Result<(), QualificationError> {
    let obs = run.observation();
    let req = run.request();
    if obs.status != ExecutionStatus::Succeeded
        || obs.effects_qualified
        || obs.block_hash != req.block.hash
        || obs.calls.len() != 1
        || obs.output != format!("0x{:064x}", 1)
        || obs.gas_used == 0
        || obs.gas_used > req.transaction.gas_limit
    {
        return Err(QualificationError::IncompleteSimulation);
    }
    let frame = &obs.calls[0];
    if frame.target != req.transaction.target
        || frame.code_address != req.transaction.target
        || frame.caller != req.transaction.caller
        || frame.value != "0x0"
        || frame.depth != 1
    {
        return Err(QualificationError::UncoveredEffects);
    }
    Ok(())
}
fn verify_native(run: &ProvedSimulation) -> Result<U256, QualificationError> {
    let req = run.request();
    let tx = &req.transaction;
    let observed = run.observation();
    let caller = parse_address(&tx.caller)?;
    let target = parse_address(&tx.target)?;
    let beneficiary = Address::from_str(&req.block.beneficiary)
        .map_err(|_| QualificationError::SnapshotMismatch)?;
    let sender = run
        .state()
        .account(caller)
        .ok_or(QualificationError::MissingState)?;
    if !sender.exists()
        || !sender.code().is_empty()
        || sender.nonce() != tx.nonce
        || tx.value != "0x0"
    {
        return Err(QualificationError::SnapshotMismatch);
    }
    let price = tx.max_fee_per_gas.min(
        u128::from(req.block.base_fee)
            .checked_add(tx.priority_fee_per_gas)
            .ok_or(QualificationError::Arithmetic)?,
    );
    let fee = U256::from(observed.gas_used)
        .checked_mul(U256::from(price))
        .ok_or(QualificationError::Arithmetic)?;
    let tip = U256::from(observed.gas_used)
        .checked_mul(U256::from(
            price
                .checked_sub(u128::from(req.block.base_fee))
                .ok_or(QualificationError::Arithmetic)?,
        ))
        .ok_or(QualificationError::Arithmetic)?;
    let mut expected = BTreeMap::new();
    for address in [caller, target, beneficiary] {
        let original = run
            .state()
            .account(address)
            .ok_or(QualificationError::MissingState)?;
        expected.entry(address).or_insert((
            original.balance(),
            original.nonce(),
            original.balance(),
            original.nonce(),
        ));
    }
    let owner = expected
        .get_mut(&caller)
        .ok_or(QualificationError::MissingState)?;
    owner.2 = owner
        .2
        .checked_sub(fee)
        .ok_or(QualificationError::Arithmetic)?;
    owner.3 = owner
        .3
        .checked_add(1)
        .ok_or(QualificationError::Arithmetic)?;
    let reward = expected
        .get_mut(&beneficiary)
        .ok_or(QualificationError::MissingState)?;
    reward.2 = reward
        .2
        .checked_add(tip)
        .ok_or(QualificationError::Arithmetic)?;
    for account in &observed.accounts {
        let address = Address::from_str(&account.address)
            .map_err(|_| QualificationError::SnapshotMismatch)?;
        let expected = expected
            .remove(&address)
            .ok_or(QualificationError::UncoveredEffects)?;
        if (
            hex(&account.balance_before)?,
            account.nonce_before,
            hex(&account.balance)?,
            account.nonce,
        ) != expected
        {
            return Err(QualificationError::UncoveredEffects);
        }
    }
    if expected.values().any(|v| v.0 != v.2 || v.1 != v.3) {
        return Err(QualificationError::UncoveredEffects);
    }
    Ok(fee)
}
fn hex(value: &str) -> Result<U256, QualificationError> {
    U256::from_str(value).map_err(|_| QualificationError::SnapshotMismatch)
}
