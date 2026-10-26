//! Native EVM RPC endpoint settings and Keychain-backed sensitive storage.

use chain_evm::{EvmNetwork as ChainEvmNetwork, EvmRpcUrl};
use ipc_types::{
    EvmNetwork as IpcEvmNetwork, EvmRpcNetworkSetting, EvmRpcSettings, IpcError, IpcErrorCode,
};

const KEYCHAIN_SERVICE: &str = "com.flowzed.zeta.evm-rpc";
const BASE_MAINNET_KEYCHAIN_ACCOUNT: &str = "base-mainnet";
const BASE_SEPOLIA_KEYCHAIN_ACCOUNT: &str = "base-sepolia";
const ETHEREUM_MAINNET_KEYCHAIN_ACCOUNT: &str = "ethereum-mainnet";
const ETHEREUM_SEPOLIA_KEYCHAIN_ACCOUNT: &str = "ethereum-sepolia";

mod endpoint;

pub(crate) use endpoint::resolved;

#[tauri::command]
pub(crate) fn evm_rpc_settings() -> EvmRpcSettings {
    EvmRpcSettings {
        networks: ChainEvmNetwork::ALL
            .into_iter()
            .map(|network| network_setting(network, custom_rpc_configured(network)))
            .collect(),
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn configure_evm_rpc(
    app: tauri::AppHandle,
    network: IpcEvmNetwork,
) -> Result<bool, IpcError> {
    use objc2::{MainThreadMarker, rc::autoreleasepool};
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSSecureTextField};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use tokio::sync::oneshot;
    use zeroize::Zeroizing;

    let network = chain_network(network);
    let descriptor = network.descriptor();
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(&format!(
                "Set {} RPC URL",
                descriptor.display_name()
            )));
            alert.setInformativeText(&NSString::from_str(
                "Paste a complete HTTPS JSON-RPC URL. Provider credentials inside the URL are stored in macOS Keychain and are never added to chats, model context, ordinary settings, or diagnostics. Minutus will verify the endpoint's chain ID before transaction-sensitive use.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Save securely"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSSecureTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 24.0)),
            );
            field.setPlaceholderString(Some(&NSString::from_str("https://provider.example/rpc")));
            alert.setAccessoryView(Some(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                return Ok(false);
            }
            let mut value = Zeroizing::new(field.stringValue().to_string());
            field.setStringValue(&NSString::new());
            let endpoint = EvmRpcUrl::new(std::mem::take(&mut *value)).map_err(|error| IpcError {
                code: IpcErrorCode::InvalidRequest,
                message: error.to_string(),
                retryable: false,
            })?;
            security_framework::passwords::set_generic_password(
                KEYCHAIN_SERVICE,
                keychain_account(network),
                endpoint.expose_sensitive().as_bytes(),
            )
            .map_err(|_| IpcError {
                code: IpcErrorCode::Internal,
                message: format!(
                    "Minutus could not save the {} RPC URL to macOS Keychain",
                    descriptor.display_name()
                ),
                retryable: true,
            })?;
            Ok(true)
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub(crate) async fn configure_evm_rpc(
    _app: tauri::AppHandle,
    network: IpcEvmNetwork,
) -> Result<bool, IpcError> {
    Err(missing_error(chain_network(network)))
}

#[tauri::command]
pub(crate) fn remove_evm_rpc(network: IpcEvmNetwork) -> Result<bool, IpcError> {
    let network = chain_network(network);
    #[cfg(target_os = "macos")]
    {
        if !custom_rpc_configured(network) {
            return Ok(false);
        }
        security_framework::passwords::delete_generic_password(
            KEYCHAIN_SERVICE,
            keychain_account(network),
        )
        .map_err(|_| IpcError {
            code: IpcErrorCode::Internal,
            message: format!(
                "Minutus could not remove the {} RPC URL from macOS Keychain",
                network.descriptor().display_name()
            ),
            retryable: true,
        })?;
        Ok(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(missing_error(network))
    }
}

pub(crate) const fn chain_network(network: IpcEvmNetwork) -> ChainEvmNetwork {
    match network {
        IpcEvmNetwork::BaseMainnet => ChainEvmNetwork::BaseMainnet,
        IpcEvmNetwork::BaseSepolia => ChainEvmNetwork::BaseSepolia,
        IpcEvmNetwork::EthereumMainnet => ChainEvmNetwork::EthereumMainnet,
        IpcEvmNetwork::EthereumSepolia => ChainEvmNetwork::EthereumSepolia,
    }
}

const fn ipc_network(network: ChainEvmNetwork) -> IpcEvmNetwork {
    match network {
        ChainEvmNetwork::BaseMainnet => IpcEvmNetwork::BaseMainnet,
        ChainEvmNetwork::BaseSepolia => IpcEvmNetwork::BaseSepolia,
        ChainEvmNetwork::EthereumMainnet => IpcEvmNetwork::EthereumMainnet,
        ChainEvmNetwork::EthereumSepolia => IpcEvmNetwork::EthereumSepolia,
    }
}

const fn keychain_account(network: ChainEvmNetwork) -> &'static str {
    match network {
        ChainEvmNetwork::BaseMainnet => BASE_MAINNET_KEYCHAIN_ACCOUNT,
        ChainEvmNetwork::BaseSepolia => BASE_SEPOLIA_KEYCHAIN_ACCOUNT,
        ChainEvmNetwork::EthereumMainnet => ETHEREUM_MAINNET_KEYCHAIN_ACCOUNT,
        ChainEvmNetwork::EthereumSepolia => ETHEREUM_SEPOLIA_KEYCHAIN_ACCOUNT,
    }
}

#[cfg(target_os = "macos")]
fn custom_rpc_configured(network: ChainEvmNetwork) -> bool {
    use security_framework::item::{ItemClass, ItemSearchOptions};

    ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(KEYCHAIN_SERVICE)
        .account(keychain_account(network))
        .load_attributes(true)
        .skip_authenticated_items(true)
        .search()
        .is_ok_and(|items| !items.is_empty())
}

#[cfg(not(target_os = "macos"))]
const fn custom_rpc_configured(_network: ChainEvmNetwork) -> bool {
    false
}

fn network_setting(network: ChainEvmNetwork, custom_rpc_configured: bool) -> EvmRpcNetworkSetting {
    let descriptor = network.descriptor();
    EvmRpcNetworkSetting {
        network: ipc_network(network),
        display_name: descriptor.display_name().to_owned(),
        caip2_id: descriptor.caip2_id().to_owned(),
        chain_id: descriptor.chain_id().to_string(),
        native_currency_symbol: descriptor.native_currency_symbol().to_owned(),
        default_rpc_url: descriptor.default_rpc_url().to_owned(),
        block_explorer_url: descriptor.block_explorer_url().to_owned(),
        is_testnet: descriptor.is_testnet(),
        custom_rpc_configured,
    }
}

#[cfg(not(target_os = "macos"))]
fn missing_error(network: ChainEvmNetwork) -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: format!(
            "No valid custom {} RPC URL is configured in this device's Keychain",
            network.descriptor().display_name()
        ),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_projection_contains_no_custom_endpoint_text() {
        let setting = network_setting(ChainEvmNetwork::BaseSepolia, true);
        assert_eq!(setting.network, IpcEvmNetwork::BaseSepolia);
        assert_eq!(setting.chain_id, "84532");
        assert!(setting.is_testnet);
        assert!(setting.custom_rpc_configured);
        assert_eq!(setting.default_rpc_url, "https://sepolia.base.org");
    }

    #[test]
    fn keychain_accounts_are_network_specific() {
        assert_ne!(
            keychain_account(ChainEvmNetwork::BaseMainnet),
            keychain_account(ChainEvmNetwork::BaseSepolia)
        );
    }
}
