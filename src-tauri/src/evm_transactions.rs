//! Restart-safe public monitoring for host-authorized EVM broadcasts.

use std::sync::Arc;

use app_core::{
    AppCore, StoredEvmTransaction, StoredEvmTransactionState, TransactionAuditEvent,
    TransactionAuditEventKind, TransactionAuditFailureCode, TransactionAuditOrigin,
    TransactionAuditResult, TransactionAuditTransactionType,
};
use chain_evm::{TransactionLifecycle, TransactionObservation, monitor_transaction};
use ipc_types::{
    EvmNetwork, EvmTrackedTransaction, EvmTransactionLifecycle, IpcError, IpcErrorCode,
};
use tauri::State;

use crate::{
    error_mapping::{app_core_error, current_timestamp},
    evm_rpc_settings::{chain_network, resolved},
};

/// Refreshes every restart-safe submitted transaction without rebroadcasting.
#[tauri::command]
pub(crate) async fn evm_transactions(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<EvmTrackedTransaction>, IpcError> {
    let records = core.evm_transactions().map_err(app_core_error)?;
    let mut projected = Vec::with_capacity(records.len());
    for record in records {
        projected.push(refresh_record(&core, record).await?);
    }
    Ok(projected)
}

async fn refresh_record(
    core: &AppCore,
    mut record: StoredEvmTransaction,
) -> Result<EvmTrackedTransaction, IpcError> {
    let network = ipc_network(&record.network)?;
    if matches!(
        record.state,
        StoredEvmTransactionState::Confirmed | StoredEvmTransactionState::Failed
    ) {
        return Ok(project_record(&record, network, true, None));
    }
    let transaction_hash = chain_evm::TransactionHash::new(&record.transaction_hash)
        .map_err(|_| IpcError::internal())?;
    let endpoint = resolved(chain_network(network))?;
    let sender = record.sender.clone();
    let nonce = record.nonce.clone();
    let observation = tokio::task::spawn_blocking(move || {
        monitor_transaction(
            chain_network(network),
            &endpoint,
            &transaction_hash,
            &sender,
            &nonce,
        )
    })
    .await
    .map_err(|_| IpcError::internal())?;
    match observation {
        Ok(observation) => {
            apply_observation(&mut record, &observation, current_timestamp()?);
            core.save_evm_transaction(&record).map_err(app_core_error)?;
            record_terminal_audit(core, &record, network);
            Ok(project_record(&record, network, true, None))
        }
        Err(error) => Ok(project_record(
            &record,
            network,
            false,
            Some(error.to_string()),
        )),
    }
}

fn record_terminal_audit(core: &AppCore, record: &StoredEvmTransaction, network: EvmNetwork) {
    let result = match record.state {
        StoredEvmTransactionState::Confirmed => TransactionAuditResult::Succeeded,
        StoredEvmTransactionState::Failed | StoredEvmTransactionState::DroppedOrReplaced => {
            TransactionAuditResult::Failed(TransactionAuditFailureCode::ConfirmationFailed)
        }
        StoredEvmTransactionState::Pending => return,
    };
    let Some(transaction_hash) = decode_hash(&record.transaction_hash) else {
        return;
    };
    let descriptor = chain_network(network).descriptor();
    let event = TransactionAuditEvent::new(
        &record.transaction_hash[2..],
        &record.transaction_hash[2..],
        &record.sender,
        descriptor.chain_id(),
        TransactionAuditTransactionType::Eip1559,
        None,
        Some(transaction_hash),
        TransactionAuditOrigin::LocalSystem,
        TransactionAuditEventKind::Confirmation { result },
    );
    if let Ok(event) = event {
        let _ = core.record_transaction_audit(event);
    }
}

fn decode_hash(value: &str) -> Option<[u8; 32]> {
    let hex = value.strip_prefix("0x")?;
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes[index] = (high << 4) | low;
    }
    Some(bytes)
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn apply_observation(
    record: &mut StoredEvmTransaction,
    observation: &TransactionObservation,
    observed_at: String,
) {
    record.state = match observation.lifecycle() {
        TransactionLifecycle::Pending => StoredEvmTransactionState::Pending,
        TransactionLifecycle::Confirmed => StoredEvmTransactionState::Confirmed,
        TransactionLifecycle::Failed => StoredEvmTransactionState::Failed,
        TransactionLifecycle::DroppedOrReplaced => StoredEvmTransactionState::DroppedOrReplaced,
    };
    record.block_number = observation.block_number().map(str::to_owned);
    record.block_hash = observation.block_hash().map(str::to_owned);
    record.updated_at = observed_at;
}

fn project_record(
    record: &StoredEvmTransaction,
    network: EvmNetwork,
    status_available: bool,
    status_error: Option<String>,
) -> EvmTrackedTransaction {
    let descriptor = chain_network(network).descriptor();
    EvmTrackedTransaction {
        network,
        network_name: descriptor.display_name().to_owned(),
        caip2_id: descriptor.caip2_id().to_owned(),
        chain_id: descriptor.chain_id().to_string(),
        is_testnet: descriptor.is_testnet(),
        transaction_hash: record.transaction_hash.clone(),
        lifecycle: match record.state {
            StoredEvmTransactionState::Pending => EvmTransactionLifecycle::Pending,
            StoredEvmTransactionState::Confirmed => EvmTransactionLifecycle::Confirmed,
            StoredEvmTransactionState::Failed => EvmTransactionLifecycle::Failed,
            StoredEvmTransactionState::DroppedOrReplaced => {
                EvmTransactionLifecycle::DroppedOrReplaced
            }
        },
        block_number: record.block_number.clone(),
        block_hash: record.block_hash.clone(),
        explorer_url: format!(
            "{}/tx/{}",
            descriptor.block_explorer_url(),
            record.transaction_hash
        ),
        sender: record.sender.clone(),
        nonce: record.nonce.clone(),
        candidate_digest: record.candidate_digest.clone(),
        submitted_at: record.submitted_at.clone(),
        updated_at: record.updated_at.clone(),
        status_available,
        status_error,
    }
}

fn ipc_network(caip2_id: &str) -> Result<EvmNetwork, IpcError> {
    match caip2_id {
        "eip155:8453" => Ok(EvmNetwork::BaseMainnet),
        "eip155:84532" => Ok(EvmNetwork::BaseSepolia),
        "eip155:1" => Ok(EvmNetwork::EthereumMainnet),
        "eip155:11155111" => Ok(EvmNetwork::EthereumSepolia),
        _ => Err(IpcError {
            code: IpcErrorCode::Internal,
            message: "Stored EVM transaction network is invalid".to_owned(),
            retryable: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_rpc_unavailability_distinct_from_chain_failure() {
        let record = StoredEvmTransaction {
            transaction_hash: format!("0x{}", "11".repeat(32)),
            network: "eip155:84532".to_owned(),
            sender: "0x9858effd232b4033e47d90003d41ec34ecaeda94".to_owned(),
            nonce: "1".to_owned(),
            candidate_digest: format!("0x{}", "22".repeat(32)),
            state: StoredEvmTransactionState::Pending,
            block_number: None,
            block_hash: None,
            submitted_at: "2026-09-01T00:00:00Z".to_owned(),
            updated_at: "2026-09-01T00:00:00Z".to_owned(),
        };
        let projected = project_record(
            &record,
            EvmNetwork::BaseSepolia,
            false,
            Some("Base RPC endpoint could not be reached".to_owned()),
        );
        assert_eq!(projected.lifecycle, EvmTransactionLifecycle::Pending);
        assert!(!projected.status_available);
        assert!(projected.status_error.is_some());
        assert!(
            projected
                .explorer_url
                .starts_with("https://sepolia-explorer.base.org/tx/")
        );
    }
}
