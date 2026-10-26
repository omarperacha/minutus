//! Locked-vault-safe public EVM balance coordination and stale caching.

use std::{collections::HashMap, sync::Mutex};

use chain_evm::{BaseAssetBalance, PublicEvmAddress, fetch_base_balances};
use ipc_types::{
    EvmAssetBalance, EvmBalanceFreshness, EvmBalanceRequest, EvmBalanceResponse,
    EvmBalanceSnapshot, EvmNetwork, IpcError, IpcErrorCode,
};
use tauri::State;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::evm_rpc_settings::{chain_network, resolved};

type BalanceCacheKey = (EvmNetwork, String);

/// Process-local cache of successful public reads only.
pub(crate) struct EvmBalanceCache(Mutex<HashMap<BalanceCacheKey, EvmBalanceSnapshot>>);

impl EvmBalanceCache {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

#[tauri::command]
pub(crate) async fn evm_balances(
    request: EvmBalanceRequest,
    cache: State<'_, EvmBalanceCache>,
) -> Result<EvmBalanceResponse, IpcError> {
    let address = PublicEvmAddress::new(&request.address).map_err(invalid_request)?;
    let normalized_address = address.as_str().to_owned();
    let network = chain_network(request.network);
    let endpoint = resolved(network)?;
    let query =
        tokio::task::spawn_blocking(move || fetch_base_balances(network, &endpoint, &address))
            .await
            .map_err(|_| IpcError::internal())?;

    match query {
        Ok(assets) => {
            let snapshot = snapshot(request.network, normalized_address.clone(), &assets)?;
            cache
                .0
                .lock()
                .map_err(|_| IpcError::internal())?
                .insert((request.network, normalized_address), snapshot.clone());
            Ok(EvmBalanceResponse {
                freshness: EvmBalanceFreshness::Confirmed,
                snapshot: Some(snapshot),
                error_message: None,
            })
        }
        Err(error) => {
            let prior = cache
                .0
                .lock()
                .map_err(|_| IpcError::internal())?
                .get(&(request.network, normalized_address))
                .cloned();
            Ok(fallback_response(prior, error.to_string()))
        }
    }
}

fn snapshot(
    network: EvmNetwork,
    address: String,
    assets: &[BaseAssetBalance],
) -> Result<EvmBalanceSnapshot, IpcError> {
    let descriptor = chain_network(network).descriptor();
    let observed_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| IpcError::internal())?;
    Ok(EvmBalanceSnapshot {
        network,
        network_name: descriptor.display_name().to_owned(),
        address,
        observed_at,
        is_testnet: descriptor.is_testnet(),
        assets: assets.iter().map(asset_balance).collect(),
    })
}

fn asset_balance(balance: &BaseAssetBalance) -> EvmAssetBalance {
    let descriptor = balance.descriptor();
    EvmAssetBalance {
        symbol: descriptor.symbol().to_owned(),
        decimals: descriptor.decimals(),
        token_contract: descriptor.token_contract().map(str::to_owned),
        raw_amount: balance.raw_amount().to_owned(),
        formatted_amount: balance.formatted_amount().to_owned(),
    }
}

fn fallback_response(
    snapshot: Option<EvmBalanceSnapshot>,
    error_message: String,
) -> EvmBalanceResponse {
    EvmBalanceResponse {
        freshness: if snapshot.is_some() {
            EvmBalanceFreshness::Stale
        } else {
            EvmBalanceFreshness::Unavailable
        },
        snapshot,
        error_message: Some(error_message),
    }
}

fn invalid_request(error: chain_evm::EvmBalanceError) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: error.to_string(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prior_snapshot() -> EvmBalanceSnapshot {
        EvmBalanceSnapshot {
            network: EvmNetwork::BaseMainnet,
            network_name: "Base mainnet".to_owned(),
            address: "0x0000000000000000000000000000000000000001".to_owned(),
            observed_at: "2026-09-01T00:00:00Z".to_owned(),
            is_testnet: false,
            assets: Vec::new(),
        }
    }

    #[test]
    fn rpc_failure_is_unavailable_without_a_confirmed_zero() {
        let response = fallback_response(None, "RPC unavailable".to_owned());
        assert_eq!(response.freshness, EvmBalanceFreshness::Unavailable);
        assert!(response.snapshot.is_none());
    }

    #[test]
    fn rpc_failure_preserves_the_last_confirmed_snapshot_as_stale() {
        let response = fallback_response(Some(prior_snapshot()), "RPC unavailable".to_owned());
        assert_eq!(response.freshness, EvmBalanceFreshness::Stale);
        assert_eq!(
            response
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.address.as_str()),
            Some("0x0000000000000000000000000000000000000001")
        );
    }

    #[test]
    fn balance_cache_has_no_vault_or_unlock_input() {
        let cache = EvmBalanceCache::new();
        assert!(cache.0.lock().is_ok_and(|entries| entries.is_empty()));
    }
}
