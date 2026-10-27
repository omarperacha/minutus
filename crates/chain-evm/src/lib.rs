//! EVM payload construction, trusted decoding and simulation interfaces.

mod authorization;
mod balance;
mod broadcast;
mod network;
mod rpc;
mod signing_payload;
mod simulation;
mod token_transfer;
mod transfer;

pub use authorization::{
    AuthorizationDecodeError, AuthorizationField, AuthorizationKind, AuthorizationStatus,
    AuthorizationWarning, AuthorizationWarningCode, DecodedAuthorization, WarningSeverity,
    decode_approval_calldata, decode_typed_data,
};
pub use balance::{
    BaseAssetBalance, BaseAssetDescriptor, EvmBalanceError, PublicEvmAddress, fetch_base_balances,
    trusted_assets,
};
pub use broadcast::{
    BroadcastError, BroadcastReceipt, TransactionHash, TransactionLifecycle,
    TransactionObservation, broadcast_signed_transaction,
    broadcast_signed_transaction_with_admission, monitor_transaction, signed_transaction_hash,
};
pub use network::{
    BASE_MAINNET_BLOCK_EXPLORER_URL, BASE_MAINNET_CHAIN_ID, BASE_MAINNET_DEFAULT_RPC_URL,
    BASE_SEPOLIA_BLOCK_EXPLORER_URL, BASE_SEPOLIA_CHAIN_ID, BASE_SEPOLIA_DEFAULT_RPC_URL,
    BaseNetwork, BaseNetworkDescriptor, ETHEREUM_MAINNET_BLOCK_EXPLORER_URL,
    ETHEREUM_MAINNET_CHAIN_ID, ETHEREUM_MAINNET_DEFAULT_RPC_URL,
    ETHEREUM_SEPOLIA_BLOCK_EXPLORER_URL, ETHEREUM_SEPOLIA_CHAIN_ID,
    ETHEREUM_SEPOLIA_DEFAULT_RPC_URL, EvmNetwork, EvmNetworkDescriptor, EvmRpcUrl, EvmRpcUrlError,
    RpcChainIdError,
};
pub use signing_payload::{
    DecodedEip1559Transaction, DecodedSigningPayload, DecodedTransactionAction,
    DecodedTransactionStatus, DecodedTransactionWarning, DecodedTransactionWarningCode,
    PreparedTransactionCandidate, SigningPayload, SigningPayloadDecodeError,
    assemble_signed_eip1559_transaction, decode_and_verify_prepared_transaction,
    decode_signing_payload, eip1559_signing_digest, encode_prepared_transaction,
};
pub use simulation::{
    SimulationComparison, SimulationEffect, SimulationEffectKind, SimulationState,
    TransactionSimulation,
};
pub use token_transfer::{
    DecodedTokenTransfer, TokenTransferCandidate, TokenTransferError, TokenTransferRequest,
    decode_token_transfer, prepare_token_transfer,
};
pub use transfer::{
    NativeTransferCandidate, NativeTransferError, NativeTransferRequest, prepare_native_transfer,
};
