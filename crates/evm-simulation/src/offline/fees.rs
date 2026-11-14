//! Snapshot fee bounds for the pinned Fjord/Jovian algorithms. Checked arithmetic
//! deliberately fails rather than presenting an overflowed ceiling as a bound.
use super::{database::FrozenDb, envelope::Envelope, snapshot::FrozenBaseRequest};
use crate::{SimulationError, admission};
use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_primitives::{Bytes, TxKind, U256};
use base_evm::{BaseSpecId, BaseUpgrade, L1BlockInfo};

/// Maximum native fee for any canonical low-s signature on this exact body and
/// this frozen snapshot. Inclusion under different fee state requires reapproval.
pub struct BaseFeeCeiling {
    total: U256,
}
impl BaseFeeCeiling {
    /// Exact hexadecimal wei ceiling, including gas-cap, L1 and operator charges.
    #[must_use]
    pub fn total_wei(&self) -> String {
        format!("{:#x}", self.total)
    }
}
pub(super) fn spec(snapshot: &FrozenBaseRequest) -> Result<BaseSpecId, SimulationError> {
    Ok(BaseSpecId::new(
        match admission::validate(&snapshot.request)? {
            admission::Rules::BaseCobalt => BaseUpgrade::Cobalt,
            admission::Rules::BaseDenim => BaseUpgrade::Denim,
            _ => return Err(SimulationError::UnsupportedRules),
        },
    ))
}
pub(super) fn info(snapshot: &FrozenBaseRequest) -> Result<L1BlockInfo, SimulationError> {
    L1BlockInfo::try_fetch(
        &mut FrozenDb::new(snapshot)?,
        U256::from(snapshot.request.block.number),
        spec(snapshot)?,
    )
}
fn mul(a: U256, b: U256) -> Result<U256, SimulationError> {
    a.checked_mul(b).ok_or(SimulationError::Limit)
}
fn add(a: U256, b: U256) -> Result<U256, SimulationError> {
    a.checked_add(b).ok_or(SimulationError::Limit)
}
pub(super) fn operator(info: &L1BlockInfo, gas: u64) -> Result<U256, SimulationError> {
    add(
        mul(
            mul(
                info.operator_fee_scalar
                    .ok_or(SimulationError::StateUnavailable)?,
                U256::from(gas),
            )?,
            U256::from(L1BlockInfo::OPERATOR_FEE_JOVIAN_MULTIPLIER),
        )?,
        info.operator_fee_constant
            .ok_or(SimulationError::StateUnavailable)?,
    )
}
pub(super) fn ceiling(snapshot: &FrozenBaseRequest) -> Result<BaseFeeCeiling, SimulationError> {
    let tx = &snapshot.request.transaction;
    let unsigned = TxEip1559 {
        chain_id: snapshot.request.network.chain_id(),
        nonce: tx.nonce,
        gas_limit: tx.gas_limit,
        max_fee_per_gas: tx.max_fee_per_gas,
        max_priority_fee_per_gas: tx.priority_fee_per_gas,
        to: TxKind::Call(admission::address(&tx.target)?),
        value: admission::quantity(&tx.value)?,
        input: Bytes::from(admission::data(&tx.calldata, 8192)?),
        ..TxEip1559::default()
    };
    // <=8192 calldata and fixed-width numeric bounds keep payload below 65536.
    // Adding parity+r+s adds <=67 payload bytes and <=1 RLP length byte.
    let length = unsigned
        .encoded_for_signing()
        .len()
        .checked_add(68)
        .ok_or(SimulationError::Limit)?;
    if length > super::MAX_ENVELOPE {
        return Err(SimulationError::Limit);
    }
    // Pinned FastLZ literals add one control byte per <=32 bytes. Every match
    // saves >=1 byte, paying for an extra fragmented literal-run control byte.
    let compressed = length + length.div_ceil(32);
    let estimated = U256::from(compressed)
        .checked_mul(U256::from(836_500))
        .ok_or(SimulationError::Limit)?
        .saturating_sub(U256::from(42_585_600))
        .max(U256::from(100_000_000));
    let fees = info(snapshot)?;
    if fees.empty_ecotone_scalars {
        return Err(SimulationError::UnsupportedRules);
    }
    let scaled = add(
        mul(
            mul(fees.l1_base_fee, fees.l1_base_fee_scalar)?,
            U256::from(16),
        )?,
        mul(
            fees.l1_blob_base_fee
                .ok_or(SimulationError::StateUnavailable)?,
            fees.l1_blob_base_fee_scalar
                .ok_or(SimulationError::StateUnavailable)?,
        )?,
    )?;
    let l1 = mul(estimated, scaled)? / U256::from(1_000_000_000_000u64);
    let total = add(
        add(
            mul(U256::from(tx.gas_limit), U256::from(tx.max_fee_per_gas))?,
            l1,
        )?,
        operator(&fees, tx.gas_limit)?,
    )?;
    Ok(BaseFeeCeiling { total })
}
pub(super) struct ExactFees {
    pub(super) l1: U256,
    pub(super) operator: U256,
    pub(super) tip: U256,
    pub(super) base: U256,
    pub(super) total: U256,
}
pub(super) fn exact(
    snapshot: &FrozenBaseRequest,
    envelope: &Envelope,
    gas: u64,
) -> Result<ExactFees, SimulationError> {
    if gas > snapshot.request.transaction.gas_limit {
        return Err(SimulationError::InconsistentState);
    }
    let mut fees = info(snapshot)?;
    if fees.empty_ecotone_scalars {
        return Err(SimulationError::UnsupportedRules);
    }
    // First check the rigorous cap, which also guarantees native fee math fits.
    let cap = ceiling(snapshot)?;
    let l1 = fees.calculate_tx_l1_cost(&envelope.bytes, spec(snapshot)?);
    let operator = operator(&fees, gas)?;
    let tx = &snapshot.request.transaction;
    let base = mul(U256::from(gas), U256::from(snapshot.request.block.base_fee))?;
    // Cap the tip before adding the base fee, preserving valid u128::MAX
    // caps without an intermediate base+priority overflow.
    let base_price = u128::from(snapshot.request.block.base_fee);
    let tip_cap = tx
        .max_fee_per_gas
        .checked_sub(base_price)
        .ok_or(SimulationError::TransactionRejected)?;
    let price = base_price + tx.priority_fee_per_gas.min(tip_cap);
    let execution = mul(U256::from(gas), U256::from(price))?;
    let tip = execution
        .checked_sub(base)
        .ok_or(SimulationError::TransactionRejected)?;
    let total = add(add(l1, operator)?, execution)?;
    if total > cap.total {
        return Err(SimulationError::InconsistentState);
    }
    Ok(ExactFees {
        l1,
        operator,
        tip,
        base,
        total,
    })
}
