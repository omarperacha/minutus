//! Exact bounded delegation correspondence. A slot value alone proves no dispatch.
use super::error::GenericError;
use alloy_primitives::{Address, U256, address, hex};

pub(super) const IMPLEMENTATION: U256 = U256::from_be_bytes(hex!(
    "360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
));
pub(super) const ADMIN: U256 = U256::from_be_bytes(hex!(
    "b53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"
));
pub(super) const BEACON: U256 = U256::from_be_bytes(hex!(
    "a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"
));

/// Exact standard ERC-1167 runtime, including its fixed implementation address.
pub(super) fn minimal_target(code: &[u8]) -> Option<Address> {
    const PREFIX: [u8; 10] = hex!("363d3d373d3d3d363d73");
    const SUFFIX: [u8; 15] = hex!("5af43d82803e903d91602b57fd5bf3");
    if code.len() != 45 || code[..10] != PREFIX || code[30..] != SUFFIX {
        return None;
    }
    let target = Address::from_slice(&code[10..30]);
    (target != Address::ZERO).then_some(target)
}

pub(super) fn control(slot: U256) -> bool {
    [IMPLEMENTATION, ADMIN, BEACON].contains(&slot)
}

pub(super) fn slot_address(word: U256) -> Result<Address, GenericError> {
    let bytes = word.to_be_bytes::<32>();
    if bytes[..12].iter().any(|b| *b != 0) {
        return Err(GenericError::UnsupportedProxy);
    }
    let address = Address::from_slice(&bytes[12..]);
    if address == Address::ZERO {
        return Err(GenericError::UnsupportedProxy);
    }
    Ok(address)
}

pub(super) fn system_address(address: Address) -> bool {
    let bytes = address.as_slice();
    // Stateful rollup system contracts are not ordinary source-qualified app
    // dependencies. Precompiles require a separate fixed protocol interpretation.
    (bytes[..19].iter().all(|b| *b == 0) && bytes[19] <= 0x20)
        || (bytes[0] == 0x42 && bytes[1..19].iter().all(|b| *b == 0))
        || address == address!("0000000000000000000000000000000000000000")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_minimal_proxy_rejects_variants_trailing_bytes_and_zero_implementation()
    -> Result<(), Box<dyn std::error::Error>> {
        let code = alloy_primitives::hex::decode(
            "363d3d373d3d3d363d7311111111111111111111111111111111111111115af43d82803e903d91602b57fd5bf3",
        )?;
        assert_eq!(minimal_target(&code), Some(Address::repeat_byte(0x11)));
        let mut changed = code.clone();
        changed.push(0);
        assert_eq!(minimal_target(&changed), None);
        changed = code.clone();
        changed[0] = 0x60;
        assert_eq!(minimal_target(&changed), None);
        changed = code;
        changed[10..30].fill(0);
        assert_eq!(minimal_target(&changed), None);
        Ok(())
    }
    #[test]
    fn control_slots_and_dirty_or_zero_implementation_words_reject() {
        assert!(control(IMPLEMENTATION));
        assert!(control(ADMIN));
        assert!(control(BEACON));
        assert!(!control(U256::ZERO));
        assert_eq!(
            slot_address(U256::ZERO),
            Err(GenericError::UnsupportedProxy)
        );
        assert_eq!(
            slot_address(U256::ONE << 200),
            Err(GenericError::UnsupportedProxy)
        );
    }
}
