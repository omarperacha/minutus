//! Shared host preparation and post-authentication refresh of exact transfer facts.
use crate::evm_rpc_settings::resolved;
use chain_evm::{
    EvmNetwork, NativeTransferCandidate, NativeTransferRequest, TokenTransferCandidate,
    TokenTransferRequest, prepare_native_transfer, prepare_token_transfer,
};
use ipc_types::IpcError;

pub(crate) async fn native(
    network: EvmNetwork,
    sender: &str,
    recipient: &str,
    amount: &str,
) -> Result<NativeTransferCandidate, IpcError> {
    let request = NativeTransferRequest::new(sender, recipient, amount)
        .map_err(|e| super::transfer_error(&e))?;
    let endpoint = resolved(network)?;
    tokio::task::spawn_blocking(move || prepare_native_transfer(network, &endpoint, &request))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(|e| super::transfer_error(&e))
}
pub(crate) async fn token(
    network: EvmNetwork,
    sender: &str,
    recipient: &str,
    amount: &str,
) -> Result<TokenTransferCandidate, IpcError> {
    let request =
        TokenTransferRequest::new(sender, recipient, amount).map_err(|e| super::token_error(&e))?;
    let endpoint = resolved(network)?;
    tokio::task::spawn_blocking(move || prepare_token_transfer(network, &endpoint, &request))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(|e| super::token_error(&e))
}

mod freshness;
pub(crate) use freshness::{RefreshPolicy, revalidate};
