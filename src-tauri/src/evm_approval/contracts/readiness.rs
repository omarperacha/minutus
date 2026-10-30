//! Initial public readiness projection. It never unlocks, reads a hidden locked
//! address, performs RPC, or creates signing authority.
use chain_evm::PublicEvmAddress;
use ipc_types::{IpcError, PublicWalletState, WalletAvailability};

pub(super) fn initial_account(state: &PublicWalletState) -> Result<String, IpcError> {
    match &state.availability {
        WalletAvailability::Locked => Err(IpcError::permission_denied(
            "Unlock the existing account in Wallet, then make a fresh contract workflow request",
        )),
        WalletAvailability::Unavailable => Err(crate::wallet_lifecycle::wallet_error(
            "Create, restore or import an account first",
            false,
        )),
        WalletAvailability::Ready => {
            let address = state.address.as_deref().ok_or_else(|| {
                IpcError::invalid_request(
                    "Select an account in Wallet before requesting a contract workflow",
                )
            })?;
            PublicEvmAddress::new(address)
                .map(|address| address.as_str().to_owned())
                .map_err(|_| {
                    IpcError::invalid_request("The selected public wallet address is invalid")
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(availability: WalletAvailability, address: Option<&str>) -> PublicWalletState {
        PublicWalletState {
            wallet_id: None,
            address: address.map(str::to_owned),
            network: None,
            availability,
        }
    }
    #[test]
    fn locked_wallet_is_not_reported_as_absent_or_allowed_to_supply_an_address() {
        for address in [None, Some("untrusted locked metadata")] {
            let result = initial_account(&state(WalletAvailability::Locked, address));
            assert!(result.as_ref().is_err_and(|error| {
                error
                    .message
                    .contains("Unlock the existing account in Wallet")
            }));
            assert!(
                result
                    .as_ref()
                    .is_err_and(|error| error.message.contains("fresh contract workflow request"))
            );
            assert!(result.is_err_and(|error| !error.message.contains("Create, restore")));
        }
    }
    #[test]
    fn absent_wallet_keeps_the_existing_creation_guidance() {
        let result = initial_account(&state(WalletAvailability::Unavailable, None));
        assert!(result.as_ref().is_err_and(|error| error.code
            == ipc_types::IpcErrorCode::Unsupported
            && !error.retryable));
        assert!(
            result
                .is_err_and(|error| error.message == "Create, restore or import an account first")
        );
    }
    #[test]
    fn ready_wallet_address_is_validated_and_canonicalised() -> Result<(), IpcError> {
        assert_eq!(
            initial_account(&state(
                WalletAvailability::Ready,
                Some("0x52908400098527886E0F7030069857D2E4169EE7")
            ))?,
            "0x52908400098527886e0f7030069857d2e4169ee7"
        );
        assert!(initial_account(&state(WalletAvailability::Ready, None)).is_err());
        assert!(
            initial_account(&state(WalletAvailability::Ready, Some("not-an-address"))).is_err()
        );
        Ok(())
    }
}
