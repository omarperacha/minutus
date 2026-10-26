//! Native EVM transfer preparation without approval, signing or broadcast.

use chain_evm::{
    NativeTransferCandidate, NativeTransferError, NativeTransferRequest, TokenTransferCandidate,
    TokenTransferError, TokenTransferRequest, TransactionSimulation, prepare_native_transfer,
    prepare_token_transfer,
};
use ipc_types::{
    EvmDecodedTokenTransfer, EvmNativeTransferCandidate, EvmNativeTransferRequest,
    EvmSimulationComparison, EvmSimulationEffect, EvmSimulationEffectKind, EvmSimulationState,
    EvmTokenTransferCandidate, EvmTokenTransferRequest, EvmTransactionSimulation, IpcError,
    IpcErrorCode,
};
use tauri::State;

use crate::{
    evm_approval::{ApprovalCandidate, ApprovalOrigin, EvmApprovalControl},
    evm_rpc_settings::{chain_network, resolved},
    wallet_lifecycle::WalletControl,
};

#[tauri::command]
pub(crate) async fn prepare_evm_native_transfer(
    approval: State<'_, EvmApprovalControl>,
    wallet: State<'_, WalletControl>,
    request: EvmNativeTransferRequest,
) -> Result<EvmNativeTransferCandidate, IpcError> {
    let sender = wallet.selected_address()?;
    if !request.sender.eq_ignore_ascii_case(&sender) {
        return Err(selected_account_error());
    }
    let network = chain_network(request.network);
    let transfer = NativeTransferRequest::new(&sender, &request.recipient, &request.amount_eth)
        .map_err(|error| transfer_error(&error))?;
    let endpoint = resolved(network)?;
    let candidate =
        tokio::task::spawn_blocking(move || prepare_native_transfer(network, &endpoint, &transfer))
            .await
            .map_err(|_| IpcError::internal())?
            .map_err(|error| transfer_error(&error))?;
    let review_id = approval.register(
        ApprovalCandidate::Native(candidate.clone()),
        ApprovalOrigin::User,
    )?;
    Ok(project_candidate(request.network, review_id, &candidate))
}

#[tauri::command]
pub(crate) async fn prepare_evm_token_transfer(
    approval: State<'_, EvmApprovalControl>,
    wallet: State<'_, WalletControl>,
    request: EvmTokenTransferRequest,
) -> Result<EvmTokenTransferCandidate, IpcError> {
    let sender = wallet.selected_address()?;
    if !request.sender.eq_ignore_ascii_case(&sender) {
        return Err(selected_account_error());
    }
    let network = chain_network(request.network);
    let transfer = TokenTransferRequest::new(&sender, &request.recipient, &request.amount)
        .map_err(|error| token_error(&error))?;
    let endpoint = resolved(network)?;
    let candidate =
        tokio::task::spawn_blocking(move || prepare_token_transfer(network, &endpoint, &transfer))
            .await
            .map_err(|_| IpcError::internal())?
            .map_err(|error| token_error(&error))?;
    let review_id = approval.register(
        ApprovalCandidate::Token(candidate.clone()),
        ApprovalOrigin::User,
    )?;
    Ok(project_token_candidate(
        request.network,
        review_id,
        &candidate,
    ))
}

fn selected_account_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: "The transaction sender must be the exact activated wallet account".to_owned(),
        retryable: false,
    }
}

fn project_token_candidate(
    network: ipc_types::EvmNetwork,
    review_id: ipc_types::TransactionProposalId,
    candidate: &TokenTransferCandidate,
) -> EvmTokenTransferCandidate {
    let descriptor = chain_network(network).descriptor();
    let decoded = candidate.decoded();
    EvmTokenTransferCandidate {
        review_id,
        network,
        network_name: descriptor.display_name().to_owned(),
        caip2_id: candidate.caip2_id().to_owned(),
        chain_id: candidate.chain_id().to_string(),
        is_testnet: descriptor.is_testnet(),
        sender: candidate.sender().to_owned(),
        decoded: EvmDecodedTokenTransfer {
            token_contract: decoded.token_contract().to_owned(),
            symbol: decoded.symbol().to_owned(),
            decimals: decoded.decimals(),
            recipient: decoded.recipient().to_owned(),
            amount: decoded.amount().to_owned(),
            raw_amount: decoded.raw_amount().to_owned(),
        },
        token_balance_raw: candidate.token_balance_raw().to_owned(),
        token_balance: candidate.token_balance().to_owned(),
        native_balance_wei: candidate.native_balance_wei().to_owned(),
        nonce: candidate.nonce().to_owned(),
        gas_limit: candidate.gas_limit().to_owned(),
        base_fee_per_gas_wei: candidate.base_fee_per_gas_wei().to_owned(),
        max_priority_fee_per_gas_wei: candidate.max_priority_fee_per_gas_wei().to_owned(),
        max_fee_per_gas_wei: candidate.max_fee_per_gas_wei().to_owned(),
        maximum_execution_fee_wei: candidate.maximum_execution_fee_wei().to_owned(),
        maximum_l1_fee_wei: candidate.maximum_l1_fee_wei().to_owned(),
        operator_fee_wei: candidate.operator_fee_wei().to_owned(),
        maximum_network_fee_wei: candidate.maximum_network_fee_wei().to_owned(),
        maximum_network_fee_eth: candidate.maximum_network_fee_eth().to_owned(),
        transaction_type: candidate.transaction_type(),
        data: candidate.data().to_owned(),
        simulation: project_simulation(candidate.simulation()),
    }
}

fn token_error(error: &TokenTransferError) -> IpcError {
    let retryable = matches!(
        error,
        TokenTransferError::Transport
            | TokenTransferError::HttpStatus(_)
            | TokenTransferError::ResponseTooLarge
            | TokenTransferError::RpcRejected(_)
    );
    let code = if retryable || matches!(error, TokenTransferError::ChainId(_)) {
        IpcErrorCode::Unsupported
    } else {
        IpcErrorCode::InvalidRequest
    };
    IpcError {
        code,
        message: error.to_string(),
        retryable,
    }
}

fn project_candidate(
    network: ipc_types::EvmNetwork,
    review_id: ipc_types::TransactionProposalId,
    candidate: &NativeTransferCandidate,
) -> EvmNativeTransferCandidate {
    let descriptor = chain_network(network).descriptor();
    EvmNativeTransferCandidate {
        review_id,
        network,
        network_name: descriptor.display_name().to_owned(),
        caip2_id: candidate.caip2_id().to_owned(),
        chain_id: candidate.chain_id().to_string(),
        is_testnet: descriptor.is_testnet(),
        sender: candidate.sender().to_owned(),
        recipient: candidate.recipient().to_owned(),
        amount_eth: candidate.amount_eth().to_owned(),
        value_wei: candidate.value_wei().to_owned(),
        nonce: candidate.nonce().to_owned(),
        gas_limit: candidate.gas_limit().to_owned(),
        base_fee_per_gas_wei: candidate.base_fee_per_gas_wei().to_owned(),
        max_priority_fee_per_gas_wei: candidate.max_priority_fee_per_gas_wei().to_owned(),
        max_fee_per_gas_wei: candidate.max_fee_per_gas_wei().to_owned(),
        maximum_execution_fee_wei: candidate.maximum_execution_fee_wei().to_owned(),
        maximum_l1_fee_wei: candidate.maximum_l1_fee_wei().to_owned(),
        operator_fee_wei: candidate.operator_fee_wei().to_owned(),
        maximum_network_fee_wei: candidate.maximum_network_fee_wei().to_owned(),
        maximum_network_fee_eth: candidate.maximum_network_fee_eth().to_owned(),
        account_balance_wei: candidate.account_balance_wei().to_owned(),
        transaction_type: candidate.transaction_type(),
        data: candidate.data().to_owned(),
        simulation: project_simulation(candidate.simulation()),
    }
}

fn project_simulation(simulation: &TransactionSimulation) -> EvmTransactionSimulation {
    EvmTransactionSimulation {
        state: match simulation.state() {
            chain_evm::SimulationState::Succeeded => EvmSimulationState::Succeeded,
            chain_evm::SimulationState::Reverted => EvmSimulationState::Reverted,
            chain_evm::SimulationState::Mismatch => EvmSimulationState::Mismatch,
            chain_evm::SimulationState::Inconclusive => EvmSimulationState::Inconclusive,
            chain_evm::SimulationState::Unavailable => EvmSimulationState::Unavailable,
        },
        summary: simulation.summary().to_owned(),
        revert_reason: simulation.revert_reason().map(str::to_owned),
        candidate_digest: simulation.candidate_digest().to_owned(),
        chain_id: simulation.chain_id().to_string(),
        block_number: simulation.block_number().to_owned(),
        block_hash: simulation.block_hash().to_owned(),
        effects: simulation
            .effects()
            .iter()
            .map(|effect| EvmSimulationEffect {
                kind: match effect.kind() {
                    chain_evm::SimulationEffectKind::NativeTransfer => {
                        EvmSimulationEffectKind::NativeTransfer
                    }
                    chain_evm::SimulationEffectKind::TokenTransfer => {
                        EvmSimulationEffectKind::TokenTransfer
                    }
                },
                comparison: match effect.comparison() {
                    chain_evm::SimulationComparison::Matched => EvmSimulationComparison::Matched,
                    chain_evm::SimulationComparison::Mismatch => EvmSimulationComparison::Mismatch,
                    chain_evm::SimulationComparison::Inconclusive => {
                        EvmSimulationComparison::Inconclusive
                    }
                },
                summary: effect.summary().to_owned(),
            })
            .collect(),
    }
}

fn transfer_error(error: &NativeTransferError) -> IpcError {
    let (code, retryable) = match error {
        NativeTransferError::Transport
        | NativeTransferError::HttpStatus(_)
        | NativeTransferError::ResponseTooLarge
        | NativeTransferError::RpcRejected(_) => (IpcErrorCode::Unsupported, true),
        NativeTransferError::ChainId(_) => (IpcErrorCode::Unsupported, false),
        NativeTransferError::InvalidSender
        | NativeTransferError::InvalidRecipient
        | NativeTransferError::InvalidAmount
        | NativeTransferError::ZeroAmount
        | NativeTransferError::ExcessPrecision
        | NativeTransferError::AmountOverflow
        | NativeTransferError::MalformedRpcResponse
        | NativeTransferError::ArithmeticOverflow
        | NativeTransferError::InsufficientBalance { .. } => (IpcErrorCode::InvalidRequest, false),
    };
    IpcError {
        code,
        message: error.to_string(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_errors_are_not_retryable_rpc_failures() {
        let invalid = transfer_error(&NativeTransferError::InvalidRecipient);
        assert_eq!(invalid.code, IpcErrorCode::InvalidRequest);
        assert!(!invalid.retryable);

        let transport = transfer_error(&NativeTransferError::Transport);
        assert_eq!(transport.code, IpcErrorCode::Unsupported);
        assert!(transport.retryable);

        let token_validation = token_error(&TokenTransferError::MalformedCalldata);
        assert_eq!(token_validation.code, IpcErrorCode::InvalidRequest);
        assert!(!token_validation.retryable);

        let token_transport = token_error(&TokenTransferError::RpcRejected(-32_000));
        assert_eq!(token_transport.code, IpcErrorCode::Unsupported);
        assert!(token_transport.retryable);
    }
}
