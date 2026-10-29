//! Observe execution pricing without changing a previously approved fee envelope.

use crate::{
    rpc::{RpcClient, RpcError, Uint256},
    simulation::{ObservedBlock, observe_latest_block},
};
use serde_json::json;

pub(crate) struct ExecutionFees {
    pub(crate) block: ObservedBlock,
    pub(crate) priority: Uint256,
    pub(crate) cap: Uint256,
}

pub(crate) enum ExecutionFeeError {
    Rpc(RpcError),
    Invalid,
    Overflow,
    ApprovedCapExceeded,
    ApprovedGasExceeded,
}
impl From<RpcError> for ExecutionFeeError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

pub(crate) fn observe(
    client: &RpcClient<'_>,
    caps: Option<(&str, &str)>,
) -> Result<ExecutionFees, ExecutionFeeError> {
    let priority = if let Some((priority, _)) = caps {
        Uint256::from_decimal(priority).map_err(|_| ExecutionFeeError::Invalid)?
    } else {
        Uint256::from_quantity(&client.string_result("eth_maxPriorityFeePerGas", &json!([]))?)
            .map_err(|_| ExecutionFeeError::Invalid)?
    };
    let block = observe_latest_block(client)?;
    let cap = if let Some((_, cap)) = caps {
        Uint256::from_decimal(cap).map_err(|_| ExecutionFeeError::Invalid)?
    } else {
        fee_cap(block.base_fee, priority)?
    };
    if priority > cap {
        return Err(ExecutionFeeError::Invalid);
    }
    if caps.is_some()
        && block
            .base_fee
            .checked_add(priority)
            .ok_or(ExecutionFeeError::Overflow)?
            > cap
    {
        return Err(ExecutionFeeError::ApprovedCapExceeded);
    }
    Ok(ExecutionFees {
        block,
        priority,
        cap,
    })
}

pub(crate) fn fee_cap(base: Uint256, priority: Uint256) -> Result<Uint256, ExecutionFeeError> {
    base.checked_add(base)
        .and_then(|v| v.checked_add(priority))
        .ok_or(ExecutionFeeError::Overflow)
}

/// Refresh estimates required gas but retains the exact reviewed gas limit.
pub(crate) fn gas_limit(
    estimated: Uint256,
    approved: Option<&str>,
) -> Result<Uint256, ExecutionFeeError> {
    if estimated.is_zero() {
        return Err(ExecutionFeeError::Invalid);
    }
    let Some(approved) = approved else {
        return Ok(estimated);
    };
    let approved = Uint256::from_decimal(approved).map_err(|_| ExecutionFeeError::Invalid)?;
    if estimated > approved {
        return Err(ExecutionFeeError::ApprovedGasExceeded);
    }
    Ok(approved)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_retains_original_gas_limit_and_rejects_requirements_above_it() {
        assert!(
            gas_limit(Uint256::from_u64(99), Some("100"))
                .is_ok_and(|gas| gas == Uint256::from_u64(100))
        );
        assert!(matches!(
            gas_limit(Uint256::from_u64(101), Some("100")),
            Err(ExecutionFeeError::ApprovedGasExceeded)
        ));
        assert!(matches!(
            gas_limit(Uint256::ZERO, Some("100")),
            Err(ExecutionFeeError::Invalid)
        ));
        assert!(matches!(
            gas_limit(Uint256::from_u64(99), Some("invalid")),
            Err(ExecutionFeeError::Invalid)
        ));
        assert!(
            gas_limit(Uint256::from_u64(99), None).is_ok_and(|gas| gas == Uint256::from_u64(99))
        );
    }
}
