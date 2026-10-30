//! Native, explicit uncertainty reconciliation; never signing or automatic retry.
use super::EvmApprovalControl;
use crate::{
    error_mapping::app_core_error,
    wallet_lifecycle::{TransferCancellation, WalletControl},
};
use app_core::{AppCore, StoredEvmTransactionState};
use chain_evm::EvmNetwork;
use ipc_types::{IpcError, IpcErrorCode};
mod native;
const MAX_RECONCILIATIONS_PER_REQUEST: usize = 16;

pub(crate) async fn reconcile_prior_transfers(
    app: &tauri::AppHandle,
    core: &AppCore,
    approval: &EvmApprovalControl,
    wallet: &WalletControl,
    network: EvmNetwork,
    cancellation: &TransferCancellation,
) -> Result<(), IpcError> {
    let _lease = approval.begin_reconciliation()?;
    cancellation.check()?;
    let epoch = wallet.transfer_epoch()?;
    let sender = wallet.selected_address()?;
    let caip2 = network.descriptor().caip2_id();
    // Atomic retirement prevents any worker subsequently promoting/sending one
    // of these reservations, even if it is still alive in another process.
    core.retire_evm_reservations(caip2, &sender)
        .map_err(app_core_error)?;
    let records = core
        .unacknowledged_evm_transactions()
        .map_err(app_core_error)?;
    for record in records
        .into_iter()
        .filter(|r| r.network == caip2 && r.sender.eq_ignore_ascii_case(&sender))
        .take(MAX_RECONCILIATIONS_PER_REQUEST)
    {
        cancellation.check()?;
        wallet.check_transfer_epoch(epoch)?;
        let refreshed = crate::evm_transactions::refresh_record(core, record.clone()).await;
        if refreshed.is_ok()
            && core
                .evm_transactions()
                .map_err(app_core_error)?
                .iter()
                .any(|r| {
                    r.transaction_hash == record.transaction_hash
                        && r.state != StoredEvmTransactionState::Pending
                })
        {
            continue;
        }
        let review = ReconciliationDisplay::from_record(&record, network);
        let allow = native::show(app, review, cancellation.clone()).await?;
        cancellation.check()?;
        wallet.check_transfer_epoch(epoch)?;
        if !allow {
            return Err(IpcError {
                code: IpcErrorCode::Cancelled,
                message: "Unresolved transfer kept blocked; no new transfer was prepared".into(),
                retryable: false,
            });
        }
        // The exact host-retained hash is the sole acknowledgement target.
        core.acknowledge_evm_uncertainty(&record.transaction_hash)
            .map_err(app_core_error)?;
    }
    if core
        .unacknowledged_evm_transactions()
        .map_err(app_core_error)?
        .iter()
        .any(|r| r.network == caip2 && r.sender.eq_ignore_ascii_case(&sender))
    {
        return Err(IpcError::invalid_request(
            "More unresolved transfers remain. A new user request can reconcile the next records; no new transfer was prepared",
        ));
    }
    cancellation.check()
}

struct ReconciliationDisplay {
    body: String,
}
impl ReconciliationDisplay {
    fn from_record(record: &app_core::StoredEvmTransaction, network: EvmNetwork) -> Self {
        Self {
            body: format!(
                "Network: {} ({})\nSender: {}\nTransaction hash: {}\nNonce: {}\n\nThis earlier transfer has no resolved outcome. It may still complete, even if the node does not currently show it. A missing hash does not prove that nothing was paid.\n\nAllowing new transfer requests can cause additional payments. This acknowledgement stops this old hash from blocking future chat requests; its outcome remains unresolved until chain evidence establishes it. This does not retry, replace, cancel or approve the earlier transaction. Monitoring is retained. The new request still requires separate exact transaction review and a fresh password.\n\nOnly continue if you intend another independently reviewed transfer despite this uncertainty.",
                network.descriptor().display_name(),
                record.network,
                record.sender,
                record.transaction_hash,
                record.nonce
            ),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_never_claims_nonpayment_or_approval_authority() {
        let record = app_core::StoredEvmTransaction {
            transaction_hash: format!("0x{}", "11".repeat(32)),
            network: "eip155:8453".into(),
            sender: "0x1111111111111111111111111111111111111111".into(),
            nonce: "7".into(),
            candidate_digest: format!("0x{}", "22".repeat(32)),
            state: StoredEvmTransactionState::Pending,
            block_number: None,
            block_hash: None,
            submitted_at: "now".into(),
            updated_at: "now".into(),
        };
        let text = ReconciliationDisplay::from_record(&record, EvmNetwork::BaseMainnet).body;
        assert!(text.contains(&record.transaction_hash));
        assert!(text.contains("can cause additional payments"));
        assert!(text.contains("does not retry, replace, cancel or approve"));
        assert!(text.contains("separate exact transaction review and a fresh password"));
    }
}
