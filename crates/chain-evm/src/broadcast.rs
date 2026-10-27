//! Bounded signed-transaction submission and public receipt monitoring.

use std::{error::Error, fmt};

use serde_json::{Value, json};
use sha3::{Digest, Keccak256};
use zeroize::Zeroizing;

use crate::{
    BaseNetwork, EvmRpcUrl, PublicEvmAddress, RpcChainIdError,
    rpc::{RpcClient, RpcError, Uint256},
};

const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;

/// Validated lowercase EVM transaction hash.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransactionHash(String);

impl TransactionHash {
    /// Validates an exact 32-byte `0x`-prefixed transaction hash.
    ///
    /// # Errors
    ///
    /// Returns [`BroadcastError::InvalidTransactionHash`] for malformed input.
    pub fn new(value: &str) -> Result<Self, BroadcastError> {
        if !is_fixed_hex(value, 32) {
            return Err(BroadcastError::InvalidTransactionHash);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// Canonical lowercase hash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Successful node acceptance of one exact signed transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BroadcastReceipt {
    transaction_hash: TransactionHash,
    explorer_url: String,
}

impl BroadcastReceipt {
    /// Node-accepted transaction hash.
    #[must_use]
    pub const fn transaction_hash(&self) -> &TransactionHash {
        &self.transaction_hash
    }

    /// Transaction URL constructed from trusted network configuration.
    #[must_use]
    pub fn explorer_url(&self) -> &str {
        &self.explorer_url
    }
}

/// Public transaction lifecycle derived from chain state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionLifecycle {
    /// The node still knows the transaction but no receipt exists yet.
    Pending,
    /// A successful receipt was included in a block.
    Confirmed,
    /// A receipt was included with EVM status zero.
    Failed,
    /// No transaction or receipt exists and the sender nonce has advanced.
    DroppedOrReplaced,
}

/// One chain-derived transaction status observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionObservation {
    transaction_hash: TransactionHash,
    lifecycle: TransactionLifecycle,
    block_number: Option<String>,
    block_hash: Option<String>,
    explorer_url: String,
}

impl TransactionObservation {
    /// Observed transaction hash.
    #[must_use]
    pub const fn transaction_hash(&self) -> &TransactionHash {
        &self.transaction_hash
    }

    /// Chain-derived lifecycle.
    #[must_use]
    pub const fn lifecycle(&self) -> TransactionLifecycle {
        self.lifecycle
    }

    /// Inclusion block number for a terminal receipt.
    #[must_use]
    pub fn block_number(&self) -> Option<&str> {
        self.block_number.as_deref()
    }

    /// Inclusion block hash for a terminal receipt.
    #[must_use]
    pub fn block_hash(&self) -> Option<&str> {
        self.block_hash.as_deref()
    }

    /// Trusted block-explorer transaction URL.
    #[must_use]
    pub fn explorer_url(&self) -> &str {
        &self.explorer_url
    }
}

/// Computes the public hash of exact signed bytes without submitting them.
/// # Errors
/// Returns an error for malformed or oversized signed transaction bytes.
pub fn signed_transaction_hash(signed: &str) -> Result<TransactionHash, BroadcastError> {
    Ok(TransactionHash(format_hash(&decode_signed_transaction(
        signed,
    )?)))
}

/// Submits one already approved and wallet-signed transaction.
///
/// This chain boundary deliberately has no approval or signing API. Its caller
/// must keep signed bytes host-only and prove candidate binding before calling.
/// The RPC chain is revalidated and the node-returned hash must equal the local
/// Keccak-256 hash of the exact submitted bytes.
///
/// # Errors
///
/// Returns a bounded validation, chain, transport or RPC failure.
pub fn broadcast_signed_transaction(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    signed_transaction: Zeroizing<String>,
) -> Result<BroadcastReceipt, BroadcastError> {
    broadcast_signed_transaction_with_admission(network, endpoint, signed_transaction, || true)
}

/// Submits only after a final host admission following read-only chain validation.
/// The callback cannot grant signing authority; signed bytes are already required.
/// # Errors
/// Returns cancellation before send, or the existing bounded broadcast failures.
pub fn broadcast_signed_transaction_with_admission(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    signed_transaction: Zeroizing<String>,
    admit: impl FnOnce() -> bool,
) -> Result<BroadcastReceipt, BroadcastError> {
    let bytes = decode_signed_transaction(&signed_transaction)?;
    let local_hash = TransactionHash(format_hash(&bytes));
    let client = RpcClient::new(endpoint)?;
    validate_chain(network, &client)?;
    if !admit() {
        return Err(BroadcastError::SubmissionCancelled);
    }
    let accepted = client.string_result(
        "eth_sendRawTransaction",
        &json!([signed_transaction.as_str()]),
    );
    drop(signed_transaction);
    let accepted = accepted?;
    let accepted = TransactionHash::new(&accepted)?;
    if accepted != local_hash {
        return Err(BroadcastError::TransactionHashMismatch);
    }
    Ok(BroadcastReceipt {
        explorer_url: explorer_url(network, &accepted),
        transaction_hash: accepted,
    })
}

/// Refreshes one submitted transaction using receipt, pool and sender nonce.
///
/// # Errors
///
/// Returns a bounded chain or RPC failure. An unavailable refresh is not
/// rewritten into an on-chain failure and callers should retain the prior
/// confirmed observation separately.
pub fn monitor_transaction(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    transaction_hash: &TransactionHash,
    sender: &str,
    submitted_nonce: &str,
) -> Result<TransactionObservation, BroadcastError> {
    let sender = PublicEvmAddress::new(sender).map_err(|_| BroadcastError::InvalidSender)?;
    let submitted_nonce = Uint256::from_decimal(submitted_nonce)
        .map_err(|_| BroadcastError::InvalidSubmittedNonce)?;
    let client = RpcClient::new(endpoint)?;
    validate_chain(network, &client)?;
    let receipt = client.value_result(
        "eth_getTransactionReceipt",
        &json!([transaction_hash.as_str()]),
    )?;
    if !receipt.is_null() {
        return receipt_observation(network, transaction_hash, &receipt);
    }
    let transaction = client.value_result(
        "eth_getTransactionByHash",
        &json!([transaction_hash.as_str()]),
    )?;
    let lifecycle = if transaction.is_null() {
        let current_nonce = client.string_result(
            "eth_getTransactionCount",
            &json!([sender.as_str(), "latest"]),
        )?;
        let current_nonce = Uint256::from_quantity(&current_nonce)
            .map_err(|_| BroadcastError::MalformedRpcResponse)?;
        if current_nonce > submitted_nonce {
            TransactionLifecycle::DroppedOrReplaced
        } else {
            TransactionLifecycle::Pending
        }
    } else {
        validate_pending_transaction(transaction_hash, &transaction)?;
        TransactionLifecycle::Pending
    };
    Ok(TransactionObservation {
        transaction_hash: transaction_hash.clone(),
        lifecycle,
        block_number: None,
        block_hash: None,
        explorer_url: explorer_url(network, transaction_hash),
    })
}

fn validate_chain(network: BaseNetwork, client: &RpcClient<'_>) -> Result<(), BroadcastError> {
    let chain_id = client.string_result("eth_chainId", &json!([]))?;
    network.validate_rpc_chain_id(&chain_id)?;
    Ok(())
}

fn receipt_observation(
    network: BaseNetwork,
    expected_hash: &TransactionHash,
    receipt: &Value,
) -> Result<TransactionObservation, BroadcastError> {
    let object = receipt
        .as_object()
        .ok_or(BroadcastError::MalformedRpcResponse)?;
    let actual_hash = object
        .get("transactionHash")
        .and_then(Value::as_str)
        .ok_or(BroadcastError::MalformedRpcResponse)
        .and_then(TransactionHash::new)?;
    if &actual_hash != expected_hash {
        return Err(BroadcastError::TransactionHashMismatch);
    }
    let block_number = object
        .get("blockNumber")
        .and_then(Value::as_str)
        .and_then(|value| Uint256::from_quantity(value).ok())
        .ok_or(BroadcastError::MalformedRpcResponse)?
        .to_decimal();
    let block_hash = object
        .get("blockHash")
        .and_then(Value::as_str)
        .filter(|value| is_fixed_hex(value, 32))
        .ok_or(BroadcastError::MalformedRpcResponse)?
        .to_ascii_lowercase();
    let lifecycle = match object.get("status").and_then(Value::as_str) {
        Some("0x1") => TransactionLifecycle::Confirmed,
        Some("0x0") => TransactionLifecycle::Failed,
        _ => return Err(BroadcastError::MalformedRpcResponse),
    };
    Ok(TransactionObservation {
        transaction_hash: expected_hash.clone(),
        lifecycle,
        block_number: Some(block_number),
        block_hash: Some(block_hash),
        explorer_url: explorer_url(network, expected_hash),
    })
}

fn validate_pending_transaction(
    expected_hash: &TransactionHash,
    transaction: &Value,
) -> Result<(), BroadcastError> {
    let actual = transaction
        .as_object()
        .and_then(|object| object.get("hash"))
        .and_then(Value::as_str)
        .ok_or(BroadcastError::MalformedRpcResponse)
        .and_then(TransactionHash::new)?;
    if &actual != expected_hash {
        return Err(BroadcastError::TransactionHashMismatch);
    }
    Ok(())
}

fn decode_signed_transaction(value: &str) -> Result<Vec<u8>, BroadcastError> {
    let hex = value
        .strip_prefix("0x")
        .ok_or(BroadcastError::InvalidSignedTransaction)?;
    if hex.is_empty()
        || !hex.len().is_multiple_of(2)
        || hex.len() / 2 > MAX_SIGNED_TRANSACTION_BYTES
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(BroadcastError::InvalidSignedTransaction);
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or(BroadcastError::InvalidSignedTransaction)?;
        let low = hex_digit(pair[1]).ok_or(BroadcastError::InvalidSignedTransaction)?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn format_hash(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let digest = Keccak256::digest(bytes);
    let mut output = String::with_capacity(66);
    output.push_str("0x");
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn explorer_url(network: BaseNetwork, hash: &TransactionHash) -> String {
    format!(
        "{}/tx/{}",
        network.descriptor().block_explorer_url(),
        hash.as_str()
    )
}

fn is_fixed_hex(value: &str, bytes: usize) -> bool {
    value.strip_prefix("0x").is_some_and(|hex| {
        hex.len() == bytes * 2 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Closed transaction submission and monitoring failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BroadcastError {
    /// Host revoked or expired the attempt before the network send.
    SubmissionCancelled,
    /// Signed transaction encoding is malformed or exceeds its bound.
    InvalidSignedTransaction,
    /// Transaction hash is not exactly 32 bytes.
    InvalidTransactionHash,
    /// Persisted public sender metadata is malformed.
    InvalidSender,
    /// Persisted submitted nonce is not a canonical EVM quantity.
    InvalidSubmittedNonce,
    /// Node response failed strict validation.
    MalformedRpcResponse,
    /// The node returned a different hash than the submitted bytes or request.
    TransactionHashMismatch,
    /// RPC endpoint could not be reached.
    Transport,
    /// RPC endpoint returned a non-success HTTP status.
    HttpStatus(u16),
    /// RPC response exceeded its safety limit.
    ResponseTooLarge,
    /// RPC rejected the request.
    RpcRejected(i64),
    /// RPC endpoint reported the wrong chain.
    ChainId(RpcChainIdError),
}

impl fmt::Display for BroadcastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmissionCancelled => {
                formatter.write_str("transaction cancelled before submission")
            }
            Self::InvalidSignedTransaction => {
                formatter.write_str("signed transaction is malformed")
            }
            Self::InvalidTransactionHash => formatter.write_str("transaction hash is malformed"),
            Self::InvalidSender => formatter.write_str("transaction sender is malformed"),
            Self::InvalidSubmittedNonce => formatter.write_str("submitted nonce is malformed"),
            Self::MalformedRpcResponse => {
                formatter.write_str("EVM RPC returned a malformed transaction status")
            }
            Self::TransactionHashMismatch => {
                formatter.write_str("EVM RPC returned a mismatched transaction hash")
            }
            Self::Transport => formatter.write_str("EVM RPC endpoint could not be reached"),
            Self::HttpStatus(status) => write!(formatter, "EVM RPC returned HTTP {status}"),
            Self::ResponseTooLarge => {
                formatter.write_str("EVM RPC response exceeded its safety limit")
            }
            Self::RpcRejected(code) => {
                write!(formatter, "EVM RPC rejected the request (code {code})")
            }
            Self::ChainId(error) => error.fmt(formatter),
        }
    }
}

impl Error for BroadcastError {}

impl From<RpcError> for BroadcastError {
    fn from(error: RpcError) -> Self {
        match error {
            RpcError::Transport => Self::Transport,
            RpcError::HttpStatus(status) => Self::HttpStatus(status),
            RpcError::ResponseTooLarge => Self::ResponseTooLarge,
            RpcError::MalformedResponse => Self::MalformedRpcResponse,
            RpcError::Rejected(code) => Self::RpcRejected(code),
        }
    }
}

impl From<RpcChainIdError> for BroadcastError {
    fn from(error: RpcChainIdError) -> Self {
        Self::ChainId(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: &str) -> Result<TransactionHash, BroadcastError> {
        TransactionHash::new(&format!("0x{}", byte.repeat(32)))
    }

    #[test]
    fn receipt_status_is_the_only_confirmation_source() -> Result<(), BroadcastError> {
        let transaction_hash = hash("11")?;
        let confirmed = receipt_observation(
            BaseNetwork::Mainnet,
            &transaction_hash,
            &json!({
                "transactionHash": transaction_hash.as_str(),
                "blockNumber": "0x2a",
                "blockHash": format!("0x{}", "22".repeat(32)),
                "status": "0x1"
            }),
        )?;
        assert_eq!(confirmed.lifecycle(), TransactionLifecycle::Confirmed);
        assert_eq!(confirmed.block_number(), Some("42"));

        let failed = receipt_observation(
            BaseNetwork::Mainnet,
            &transaction_hash,
            &json!({
                "transactionHash": transaction_hash.as_str(),
                "blockNumber": "0x2b",
                "blockHash": format!("0x{}", "33".repeat(32)),
                "status": "0x0"
            }),
        )?;
        assert_eq!(failed.lifecycle(), TransactionLifecycle::Failed);
        Ok(())
    }

    #[test]
    fn explorer_links_use_only_trusted_network_and_validated_hashes() -> Result<(), BroadcastError>
    {
        let transaction_hash = hash("ab")?;
        assert_eq!(
            explorer_url(BaseNetwork::Sepolia, &transaction_hash),
            format!(
                "https://sepolia-explorer.base.org/tx/{}",
                transaction_hash.as_str()
            )
        );
        assert_eq!(
            TransactionHash::new("https://attacker.example"),
            Err(BroadcastError::InvalidTransactionHash)
        );
        Ok(())
    }

    #[test]
    fn signed_transaction_hash_is_exact_and_input_is_bounded() -> Result<(), BroadcastError> {
        assert_eq!(
            format_hash(&decode_signed_transaction("0x02c0")?),
            "0xcace66b6a82a5dc496a5b21a4032eb58baf223ea6f142a01593a5d105d5ee150"
        );
        assert_eq!(
            decode_signed_transaction("0x0").map(|_| ()),
            Err(BroadcastError::InvalidSignedTransaction)
        );
        Ok(())
    }
}
