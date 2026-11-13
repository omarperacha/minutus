//! Public transaction status only; no signed payload or submission API is used.
use super::super::{
    BroadcastError, TransactionHash, TransactionLifecycle, TransactionObservation, explorer_url,
    receipt_observation, validate_pending_transaction,
};
use super::{MonitorCause, MonitorFailure, MonitorStage as Stage, budget::Budget};
use crate::{
    BaseNetwork, EvmRpcUrl, PublicEvmAddress,
    rpc::{RpcClient, Uint256},
};
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Duration};

/// Refreshes public status with the existing per-RPC limits and error surface.
/// This observation does not establish canonical inclusion or finality.
/// # Errors
/// Returns the existing bounded chain, parsing or RPC failures.
pub fn monitor_transaction(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    hash: &TransactionHash,
    sender: &str,
    nonce: &str,
) -> Result<TransactionObservation, BroadcastError> {
    let cancelled = AtomicBool::new(false);
    observe(
        network,
        endpoint,
        hash,
        sender,
        nonce,
        &Budget::new(None, &cancelled),
    )
    .map_err(MonitorFailure::legacy_error)
}

/// Performs one read-only status attempt within the caller's remaining total
/// receipt observation allowance. Each RPC is capped by both that shrinking
/// allowance and the unchanged 12-second transport limit. Null receipts are
/// normal pending state. This function never retries, signs or submits.
/// # Errors
/// Returns closed causal diagnostics, including strict validation failures,
/// actual request cancellation and exhausted observation allowance.
pub fn monitor_transaction_bounded(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    hash: &TransactionHash,
    sender: &str,
    nonce: &str,
    remaining: Duration,
    cancelled: &AtomicBool,
) -> Result<TransactionObservation, MonitorFailure> {
    observe(
        network,
        endpoint,
        hash,
        sender,
        nonce,
        &Budget::new(Some(remaining), cancelled),
    )
}

fn observe(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    hash: &TransactionHash,
    sender: &str,
    nonce: &str,
    budget: &Budget<'_>,
) -> Result<TransactionObservation, MonitorFailure> {
    budget.check(Stage::InputValidation)?;
    let sender = PublicEvmAddress::new(sender)
        .map_err(|_| budget.failure(Stage::InputValidation, MonitorCause::InvalidSender))?;
    let nonce = Uint256::from_decimal(nonce)
        .map_err(|_| budget.failure(Stage::InputValidation, MonitorCause::InvalidSubmittedNonce))?;
    budget.check(Stage::ClientSetup)?;
    let client =
        RpcClient::new(endpoint).map_err(|error| budget.failure(Stage::ClientSetup, error))?;
    let chain = query(
        &client,
        "eth_chainId",
        &json!([]),
        budget,
        Stage::ChainIdRpc,
    )?;
    let chain = chain
        .as_str()
        .ok_or_else(|| budget.failure(Stage::ChainIdValidation, MonitorCause::MalformedResponse))?;
    network
        .validate_rpc_chain_id(chain)
        .map_err(|error| budget.failure(Stage::ChainIdValidation, error))?;
    let receipt = query(
        &client,
        "eth_getTransactionReceipt",
        &json!([hash.as_str()]),
        budget,
        Stage::ReceiptRpc,
    )?;
    if !receipt.is_null() {
        let observed = receipt_observation(network, hash, &receipt)
            .map_err(|error| budget.failure(Stage::ReceiptValidation, error))?;
        budget.check(Stage::ReceiptValidation)?;
        return Ok(observed);
    }
    let transaction = query(
        &client,
        "eth_getTransactionByHash",
        &json!([hash.as_str()]),
        budget,
        Stage::TransactionRpc,
    )?;
    let (lifecycle, last_stage) = if transaction.is_null() {
        let current = query(
            &client,
            "eth_getTransactionCount",
            &json!([sender.as_str(), "latest"]),
            budget,
            Stage::NonceRpc,
        )?;
        let current = current
            .as_str()
            .and_then(|value| Uint256::from_quantity(value).ok())
            .ok_or_else(|| {
                budget.failure(Stage::NonceValidation, MonitorCause::MalformedResponse)
            })?;
        (
            if current > nonce {
                TransactionLifecycle::DroppedOrReplaced
            } else {
                TransactionLifecycle::Pending
            },
            Stage::NonceValidation,
        )
    } else {
        validate_pending_transaction(hash, &transaction)
            .map_err(|error| budget.failure(Stage::TransactionValidation, error))?;
        (TransactionLifecycle::Pending, Stage::TransactionValidation)
    };
    budget.check(last_stage)?;
    Ok(TransactionObservation {
        transaction_hash: hash.clone(),
        lifecycle,
        block_number: None,
        block_hash: None,
        explorer_url: explorer_url(network, hash),
    })
}
fn query(
    client: &RpcClient<'_>,
    method: &str,
    params: &Value,
    budget: &Budget<'_>,
    stage: Stage,
) -> Result<Value, MonitorFailure> {
    let timeout = budget.timeout(stage)?;
    let result = client.value_result_with_timeout(method, params, timeout);
    budget.check(stage)?;
    result.map_err(|error| budget.failure(stage, error))
}
