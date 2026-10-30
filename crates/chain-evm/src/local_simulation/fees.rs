/// Host financial facts for an ordinary unsigned preview. Model tools cannot
/// provide these as signing authority or disable transaction validation.
#[derive(Clone, Copy, Debug)]
pub struct SimulationFeeFacts {
    /// Bounded gas limit independently admitted by the host.
    pub gas_limit: u64,
    /// Exact fee cap, wei.
    pub max_fee_per_gas: u128,
    /// Exact priority cap, wei.
    pub priority_fee_per_gas: u128,
}

/// Observes a host-selected execution envelope for the bounded direct profile.
/// The one-million gas cap is fixed policy; no model fee/state input is accepted.
/// The native outbound scope permit must precede this request.
///
/// # Errors
/// Rejects unavailable/cross-chain headers, malformed prices, overflow or expiry.
pub fn contract_fee_facts(
    network: crate::EvmNetwork,
    endpoint: &crate::EvmRpcUrl,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<SimulationFeeFacts, evm_simulation::SimulationError> {
    use evm_simulation::{SimulationControl, SimulationError};
    let control = SimulationControl::start(cancelled);
    let header = super::latest_header(endpoint, network, &control)?;
    let rpc =
        crate::rpc::RpcClient::new(endpoint).map_err(|_| SimulationError::StateUnavailable)?;
    let value = rpc
        .value_result_with_timeout(
            "eth_maxPriorityFeePerGas",
            &serde_json::json!([]),
            control.remaining()?.min(std::time::Duration::from_secs(1)),
        )
        .map_err(|_| SimulationError::StateUnavailable)?;
    control.remaining()?;
    let priority = priority_quantity(&value)?;
    let base = header
        .base_fee_per_gas
        .ok_or(SimulationError::UnsupportedRules)?;
    let max_fee = u128::from(base)
        .checked_mul(2)
        .and_then(|v| v.checked_add(priority))
        .ok_or(SimulationError::Limit)?;
    Ok(SimulationFeeFacts {
        gas_limit: 1_000_000,
        max_fee_per_gas: max_fee,
        priority_fee_per_gas: priority,
    })
}

fn priority_quantity(value: &serde_json::Value) -> Result<u128, evm_simulation::SimulationError> {
    value
        .as_str()
        .and_then(|value| value.strip_prefix("0x"))
        .filter(|digits| {
            !digits.is_empty()
                && digits.len() <= 32
                && !(digits.len() > 1 && digits.starts_with('0'))
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .and_then(|value| u128::from_str_radix(value, 16).ok())
        .ok_or(evm_simulation::SimulationError::InconsistentState)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_requires_canonical_bounded_rpc_quantity() {
        for invalid in [
            "0x",
            "0x00",
            "0x+1",
            "0x-1",
            "0xA",
            "0X1",
            "1",
            "0x100000000000000000000000000000000",
        ] {
            assert!(priority_quantity(&serde_json::json!(invalid)).is_err());
        }
        assert_eq!(priority_quantity(&serde_json::json!("0x0")), Ok(0));
        assert_eq!(
            priority_quantity(&serde_json::json!("0xffffffffffffffffffffffffffffffff")),
            Ok(u128::MAX)
        );
    }
}
