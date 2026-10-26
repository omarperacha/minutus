//! Resolve RPC selection without silently changing providers on storage failure.

use chain_evm::{EvmNetwork, EvmRpcUrl};
use ipc_types::{IpcError, IpcErrorCode};

#[derive(Clone, Copy)]
enum StoredEndpointError {
    Unavailable,
    InvalidValue,
}

pub(crate) fn resolved(network: EvmNetwork) -> Result<EvmRpcUrl, IpcError> {
    resolve(network, read(network))
}

fn resolve(
    network: EvmNetwork,
    stored: Result<Option<EvmRpcUrl>, StoredEndpointError>,
) -> Result<EvmRpcUrl, IpcError> {
    match stored {
        Ok(Some(endpoint)) => Ok(endpoint),
        Ok(None) => EvmRpcUrl::new(network.descriptor().default_rpc_url().to_owned())
            .map_err(|_| IpcError::internal()),
        Err(error) => Err(IpcError {
            code: IpcErrorCode::Unsupported,
            message: format!(
                "The custom {} RPC endpoint {}. Restore Keychain access or remove the custom endpoint in settings; no public fallback was contacted.",
                network.descriptor().display_name(),
                match error {
                    StoredEndpointError::Unavailable => "could not be loaded from Keychain",
                    StoredEndpointError::InvalidValue => "has invalid stored configuration",
                }
            ),
            retryable: matches!(error, StoredEndpointError::Unavailable),
        }),
    }
}

#[cfg(target_os = "macos")]
fn read(network: EvmNetwork) -> Result<Option<EvmRpcUrl>, StoredEndpointError> {
    use security_framework::passwords::get_generic_password;
    use security_framework_sys::base::errSecItemNotFound;
    use zeroize::Zeroizing;

    let bytes =
        match get_generic_password(super::KEYCHAIN_SERVICE, super::keychain_account(network)) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(error) if error.code() == errSecItemNotFound => return Ok(None),
            Err(_) => return Err(StoredEndpointError::Unavailable),
        };
    let value = std::str::from_utf8(&bytes).map_err(|_| StoredEndpointError::InvalidValue)?;
    EvmRpcUrl::new(value.to_owned())
        .map(Some)
        .map_err(|_| StoredEndpointError::InvalidValue)
}

#[cfg(not(target_os = "macos"))]
fn read(_network: EvmNetwork) -> Result<Option<EvmRpcUrl>, StoredEndpointError> {
    Err(StoredEndpointError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absent_configuration_selects_the_public_default() -> Result<(), IpcError> {
        let network = EvmNetwork::BaseSepolia;
        assert_eq!(
            resolve(network, Ok(None))?.expose_sensitive(),
            network.descriptor().default_rpc_url()
        );
        let custom = EvmRpcUrl::new("https://custom.example/rpc/private-token".to_owned())
            .map_err(|_| IpcError::internal())?;
        assert_eq!(
            resolve(network, Ok(Some(custom)))?.expose_sensitive(),
            "https://custom.example/rpc/private-token"
        );
        for error in [
            StoredEndpointError::Unavailable,
            StoredEndpointError::InvalidValue,
        ] {
            let failure = resolve(network, Err(error))
                .err()
                .ok_or_else(IpcError::internal)?;
            assert!(failure.message.contains("no public fallback"));
            assert!(!failure.message.contains("private-token"));
        }
        Ok(())
    }
}
