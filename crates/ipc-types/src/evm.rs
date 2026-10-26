//! Frontend-safe EVM network and RPC configuration status.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TransactionProposalId;

/// Closed set of EVM networks configurable from application settings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmNetwork {
    /// Base production network.
    BaseMainnet,
    /// Base public Sepolia test network.
    BaseSepolia,
    /// Ethereum production network.
    EthereumMainnet,
    /// Ethereum public Sepolia test network.
    EthereumSepolia,
}

/// Frontend-safe RPC configuration status for one supported network.
///
/// Custom endpoint text is intentionally absent because provider URLs can
/// contain credentials. The native settings flow reports only whether a custom
/// endpoint is stored and the credential-free official default.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmRpcNetworkSetting {
    /// Closed network identity accepted by settings commands.
    pub network: EvmNetwork,
    /// User-visible network name.
    pub display_name: String,
    /// CAIP-2 network identifier.
    pub caip2_id: String,
    /// EIP-155 chain identifier.
    pub chain_id: String,
    /// Native fee-currency symbol.
    pub native_currency_symbol: String,
    /// Credential-free official endpoint used without an override.
    pub default_rpc_url: String,
    /// Trusted block-explorer base URL.
    pub block_explorer_url: String,
    /// Whether this network uses test assets and requires testnet styling.
    pub is_testnet: bool,
    /// Whether a custom sensitive endpoint is stored in macOS Keychain.
    pub custom_rpc_configured: bool,
}

/// Complete EVM RPC settings snapshot in stable display order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmRpcSettings {
    /// Supported production and test networks in stable display order.
    pub networks: Vec<EvmRpcNetworkSetting>,
}

/// Public-address balance query for one supported Base network.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmBalanceRequest {
    /// Closed selected network identity.
    pub network: EvmNetwork,
    /// Public 20-byte EVM address. No vault capability is represented.
    pub address: String,
}

/// Whether a balance response is current, cached after failure, or unavailable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmBalanceFreshness {
    /// The selected RPC returned this snapshot successfully.
    Confirmed,
    /// A previous confirmed snapshot is shown because refresh failed.
    Stale,
    /// No confirmed snapshot is available.
    Unavailable,
}

/// Exact frontend-safe balance for one trusted asset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmAssetBalance {
    /// Explicit trusted symbol, initially ETH or USDC.
    pub symbol: String,
    /// Fixed asset decimals used for exact formatting.
    pub decimals: u8,
    /// Trusted chain-specific token contract, absent for native ETH.
    pub token_contract: Option<String>,
    /// Exact base-unit integer represented as decimal text.
    pub raw_amount: String,
    /// Exact human-readable value with no floating-point conversion.
    pub formatted_amount: String,
}

/// One successfully observed public Base balance snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmBalanceSnapshot {
    /// Selected network identity.
    pub network: EvmNetwork,
    /// Trusted user-visible network name.
    pub network_name: String,
    /// Normalized lowercase public address.
    pub address: String,
    /// RFC 3339 time at which the complete read succeeded.
    pub observed_at: String,
    /// Persistent testnet presentation signal.
    pub is_testnet: bool,
    /// ETH followed by USDC.
    pub assets: Vec<EvmAssetBalance>,
}

/// Public balance read result, including safe stale-cache fallback state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmBalanceResponse {
    /// Freshness of the optional snapshot.
    pub freshness: EvmBalanceFreshness,
    /// Current or stale snapshot; absent when no successful read exists.
    pub snapshot: Option<EvmBalanceSnapshot>,
    /// Credential-free failure explanation for stale or unavailable results.
    pub error_message: Option<String>,
}

/// Public inputs for preparing, but not signing, a native ETH transfer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmNativeTransferRequest {
    /// Closed selected Base network.
    pub network: EvmNetwork,
    /// Public sending account.
    pub sender: String,
    /// Public recipient account.
    pub recipient: String,
    /// Exact human-readable ETH amount.
    pub amount_eth: String,
}

/// Frontend-safe immutable native-transfer preparation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmNativeTransferCandidate {
    /// Opaque host reference for opening the native trusted review surface.
    ///
    /// This reference carries no signing or approval authority.
    pub review_id: TransactionProposalId,
    /// Closed selected network.
    pub network: EvmNetwork,
    /// Trusted network display name.
    pub network_name: String,
    /// CAIP-2 network identity.
    pub caip2_id: String,
    /// Numeric EIP-155 chain ID.
    pub chain_id: String,
    /// Persistent testnet presentation signal.
    pub is_testnet: bool,
    /// Checksummed sender address.
    pub sender: String,
    /// Checksummed recipient address.
    pub recipient: String,
    /// Exact normalized ETH amount.
    pub amount_eth: String,
    /// Exact raw transfer value in wei.
    pub value_wei: String,
    /// Pending account nonce.
    pub nonce: String,
    /// RPC-estimated gas limit.
    pub gas_limit: String,
    /// Latest observed base fee per gas in wei.
    pub base_fee_per_gas_wei: String,
    /// Suggested EIP-1559 priority fee per gas in wei.
    pub max_priority_fee_per_gas_wei: String,
    /// EIP-1559 maximum fee per gas in wei.
    pub max_fee_per_gas_wei: String,
    /// Maximum L2 execution fee in wei.
    pub maximum_execution_fee_wei: String,
    /// Base L1 data-fee upper bound in wei.
    pub maximum_l1_fee_wei: String,
    /// Current Base operator fee for the estimated gas limit in wei.
    pub operator_fee_wei: String,
    /// Combined maximum network fee in wei.
    pub maximum_network_fee_wei: String,
    /// Combined maximum network fee in ETH.
    pub maximum_network_fee_eth: String,
    /// Pending balance used for validation in wei.
    pub account_balance_wei: String,
    /// EIP-2718 transaction type, initially `2`.
    pub transaction_type: u8,
    /// Native transfer calldata, initially `0x`.
    pub data: String,
    /// Exact-block read-only simulation attempt.
    pub simulation: EvmTransactionSimulation,
}

/// Public inputs for preparing, but not signing, a trusted USDC transfer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmTokenTransferRequest {
    /// Closed selected Base network.
    pub network: EvmNetwork,
    /// Public sending account.
    pub sender: String,
    /// Public token recipient.
    pub recipient: String,
    /// Exact human-readable USDC amount.
    pub amount: String,
}

/// Frontend-safe independently decoded ERC-20 transfer intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmDecodedTokenTransfer {
    /// Trusted chain-specific token contract.
    pub token_contract: String,
    /// Trusted token symbol.
    pub symbol: String,
    /// Trusted token decimals.
    pub decimals: u8,
    /// Checksummed recipient decoded from calldata.
    pub recipient: String,
    /// Exact formatted amount decoded from calldata.
    pub amount: String,
    /// Exact integer amount decoded from calldata.
    pub raw_amount: String,
}

/// Frontend-safe immutable ERC-20 transfer preparation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmTokenTransferCandidate {
    /// Opaque host reference for opening the native trusted review surface.
    ///
    /// This reference carries no signing or approval authority.
    pub review_id: TransactionProposalId,
    /// Closed selected network.
    pub network: EvmNetwork,
    /// Trusted network display name.
    pub network_name: String,
    /// CAIP-2 network identity.
    pub caip2_id: String,
    /// Numeric EIP-155 chain ID.
    pub chain_id: String,
    /// Persistent testnet presentation signal.
    pub is_testnet: bool,
    /// Checksummed sender address.
    pub sender: String,
    /// Independently decoded intent.
    pub decoded: EvmDecodedTokenTransfer,
    /// Pending raw token balance.
    pub token_balance_raw: String,
    /// Pending formatted token balance.
    pub token_balance: String,
    /// Pending ETH balance used for fee validation in wei.
    pub native_balance_wei: String,
    /// Pending account nonce.
    pub nonce: String,
    /// RPC-estimated gas limit.
    pub gas_limit: String,
    /// Latest observed base fee per gas in wei.
    pub base_fee_per_gas_wei: String,
    /// Suggested priority fee per gas in wei.
    pub max_priority_fee_per_gas_wei: String,
    /// Maximum fee per gas in wei.
    pub max_fee_per_gas_wei: String,
    /// Maximum L2 execution fee in wei.
    pub maximum_execution_fee_wei: String,
    /// Base L1 data-fee upper bound in wei.
    pub maximum_l1_fee_wei: String,
    /// Current Base operator fee in wei.
    pub operator_fee_wei: String,
    /// Combined maximum network fee in wei.
    pub maximum_network_fee_wei: String,
    /// Combined maximum network fee in ETH.
    pub maximum_network_fee_eth: String,
    /// EIP-2718 transaction type.
    pub transaction_type: u8,
    /// Exact ERC-20 transfer calldata.
    pub data: String,
    /// Exact-block read-only simulation attempt.
    pub simulation: EvmTransactionSimulation,
}

/// Decision made in the native application-owned transaction review surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmApprovalDecision {
    /// The user explicitly approved the exact displayed candidate.
    Approved,
    /// The user explicitly denied the exact displayed candidate.
    Denied,
    /// The native surface closed without an approval decision.
    Dismissed,
}

/// Frontend-safe record of one native review decision.
///
/// This receipt is display-only. It is not an approval commitment and cannot
/// be presented to the wallet boundary for signing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmApprovalReviewOutcome {
    /// Exact opaque review reference consumed by the native surface.
    pub review_id: TransactionProposalId,
    /// Distinct native decision.
    pub decision: EvmApprovalDecision,
    /// Node-accepted hash when approval proceeded through signing and broadcast.
    pub transaction_hash: Option<String>,
    /// Trusted explorer URL for the accepted transaction.
    pub explorer_url: Option<String>,
}

/// Overall exact-candidate simulation state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmSimulationState {
    /// Execution and observable effects matched intent.
    Succeeded,
    /// Execution reverted.
    Reverted,
    /// Observable output disagreed with intent.
    Mismatch,
    /// Execution completed without enough effect evidence.
    Inconclusive,
    /// Exact-block simulation could not be completed.
    Unavailable,
}

/// Effect-to-intent comparison state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmSimulationComparison {
    /// Effect agrees with intent.
    Matched,
    /// Effect disagrees with intent.
    Mismatch,
    /// RPC output cannot establish the effect.
    Inconclusive,
}

/// Supported simulated effect category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmSimulationEffectKind {
    /// Native ETH transfer.
    NativeTransfer,
    /// Trusted USDC transfer.
    TokenTransfer,
}

/// One simulation effect comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmSimulationEffect {
    /// Effect category.
    pub kind: EvmSimulationEffectKind,
    /// Comparison with independently decoded intent.
    pub comparison: EvmSimulationComparison,
    /// Trusted effect summary.
    pub summary: String,
}

/// Read-only simulation bound to one candidate and canonical block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmTransactionSimulation {
    /// Overall simulation state.
    pub state: EvmSimulationState,
    /// Trusted execution summary.
    pub summary: String,
    /// Safely decoded revert reason when available.
    pub revert_reason: Option<String>,
    /// Keccak-256 digest of exact candidate fields and block context.
    pub candidate_digest: String,
    /// Bound EIP-155 chain ID.
    pub chain_id: String,
    /// Bound canonical block number.
    pub block_number: String,
    /// Bound canonical block hash.
    pub block_hash: String,
    /// Observed or inferable effect comparisons.
    pub effects: Vec<EvmSimulationEffect>,
}

/// Chain-derived lifecycle for a submitted EVM transaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmTransactionLifecycle {
    /// Submitted without a receipt.
    Pending,
    /// Included in a block with successful receipt status.
    Confirmed,
    /// Included in a block with failed receipt status.
    Failed,
    /// Absent after the sender nonce advanced.
    DroppedOrReplaced,
}

/// Restart-safe public status for one submitted EVM transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmTrackedTransaction {
    /// Closed Base network identity.
    pub network: EvmNetwork,
    /// Trusted network display name.
    pub network_name: String,
    /// CAIP-2 chain identity.
    pub caip2_id: String,
    /// EIP-155 chain identifier.
    pub chain_id: String,
    /// Persistent testnet presentation signal.
    pub is_testnet: bool,
    /// Node-accepted transaction hash.
    pub transaction_hash: String,
    /// Latest successfully observed lifecycle.
    pub lifecycle: EvmTransactionLifecycle,
    /// Inclusion block number for confirmed or failed receipts.
    pub block_number: Option<String>,
    /// Inclusion block hash for confirmed or failed receipts.
    pub block_hash: Option<String>,
    /// Explorer URL built from trusted network configuration.
    pub explorer_url: String,
    /// Public sending account used for replacement detection.
    pub sender: String,
    /// Submitted nonce as an exact decimal integer.
    pub nonce: String,
    /// Digest of the exact approved candidate.
    pub candidate_digest: String,
    /// RFC 3339 node-acceptance time.
    pub submitted_at: String,
    /// RFC 3339 time of the latest successful chain observation.
    pub updated_at: String,
    /// Whether the latest requested refresh reached and validated the RPC.
    pub status_available: bool,
    /// Bounded transient refresh failure, distinct from on-chain failure.
    pub status_error: Option<String>,
}

/// Closed authorization payload source selected by the review UI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmAuthorizationPayloadKind {
    /// ERC-20 contract calldata plus an explicit token contract.
    ApprovalCalldata,
    /// Raw EIP-712 typed-data JSON.
    TypedData,
}

/// Read-only authorization decoding request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmAuthorizationDecodeRequest {
    /// Closed selected Base network.
    pub network: EvmNetwork,
    /// Payload interpretation requested by the user.
    pub payload_kind: EvmAuthorizationPayloadKind,
    /// Optional expected public owner account.
    pub owner: Option<String>,
    /// Token contract required for calldata and ignored for typed data.
    pub token_contract: Option<String>,
    /// Exact raw calldata or typed-data JSON retained in the result.
    pub payload: String,
}

/// Closed decoded authorization kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmAuthorizationKind {
    /// ERC-20 approval calldata.
    Approval,
    /// Exact supported EIP-2612 permit.
    Permit,
    /// Unknown or malformed calldata.
    UnknownCalldata,
    /// Unknown or unsupported typed data.
    UnknownTypedData,
}

/// Overall decoder confidence and risk state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmAuthorizationStatus {
    /// Fully recognized with no warnings.
    Verified,
    /// Fully recognized with policy warnings.
    Warning,
    /// Cannot be represented as verified benign intent.
    Unknown,
}

/// Warning prominence for authorization review.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmAuthorizationWarningSeverity {
    /// Lower-severity caution.
    Caution,
    /// Prominent high-risk warning.
    Critical,
    /// Untrusted or undecodable payload condition.
    Unknown,
}

/// Stable authorization warning category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum EvmAuthorizationWarningCode {
    /// Maximum uint256 approval.
    UnlimitedApproval,
    /// Exceptionally large near-unbounded approval.
    EffectivelyUnlimitedApproval,
    /// Unknown token or verifying contract.
    UnknownContract,
    /// Structurally suspicious spender.
    SuspiciousSpender,
    /// Spender absent from the reviewed allowlist.
    UnrecognizedSpender,
    /// Domain chain mismatch.
    ChainMismatch,
    /// Permit owner mismatch.
    OwnerMismatch,
    /// Unrecognized domain name or version.
    UnrecognizedDomain,
    /// Authorization is expired.
    ExpiredAuthorization,
    /// Authorization expires soon.
    ExpiringAuthorization,
    /// Unsupported calldata selector.
    UnknownSelector,
    /// Unsupported typed-data structure.
    UnsupportedTypedData,
    /// Security field cannot be decoded.
    UndecodableField,
}

/// One decoded security-relevant field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmAuthorizationField {
    /// Trusted field label.
    pub label: String,
    /// Decoded public value.
    pub value: String,
}

/// One trusted authorization warning.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmAuthorizationWarning {
    /// Stable warning category.
    pub code: EvmAuthorizationWarningCode,
    /// Warning prominence.
    pub severity: EvmAuthorizationWarningSeverity,
    /// Trusted user-facing text.
    pub message: String,
}

/// Frontend-safe read-only decoded authorization result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvmDecodedAuthorization {
    /// Selected network.
    pub network: EvmNetwork,
    /// Trusted network display name.
    pub network_name: String,
    /// Persistent testnet presentation signal.
    pub is_testnet: bool,
    /// Decoded payload kind.
    pub kind: EvmAuthorizationKind,
    /// Overall decoder confidence and risk state.
    pub status: EvmAuthorizationStatus,
    /// Ordered decoded security fields.
    pub fields: Vec<EvmAuthorizationField>,
    /// Trusted warnings.
    pub warnings: Vec<EvmAuthorizationWarning>,
    /// Exact untrusted calldata or typed data.
    pub raw_payload: String,
}
