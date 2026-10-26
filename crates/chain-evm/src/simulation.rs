//! Exact-block, read-only simulation for prepared Base transaction candidates.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};

use crate::{
    BaseNetwork, PublicEvmAddress, decode_token_transfer,
    rpc::{RpcCallOutcome, RpcClient, RpcError, Uint256},
};

const ERROR_STRING_SELECTOR: &str = "08c379a0";
const PANIC_SELECTOR: &str = "4e487b71";

/// Overall simulation result for an exact prepared candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SimulationState {
    /// Exact execution completed and observable effects match decoded intent.
    Succeeded,
    /// Exact execution reverted.
    Reverted,
    /// Observable output conflicts with decoded intent.
    Mismatch,
    /// Execution completed but the standard RPC exposed insufficient effects.
    Inconclusive,
    /// Exact-block simulation was not supported or could not be completed.
    Unavailable,
}

/// Comparison between an observed simulation effect and decoded intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SimulationComparison {
    /// Observed output agrees with decoded intent.
    Matched,
    /// Observed output disagrees with decoded intent.
    Mismatch,
    /// Standard RPC output cannot expose the effect conclusively.
    Inconclusive,
}

/// Supported effect category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SimulationEffectKind {
    /// Native ETH value movement.
    NativeTransfer,
    /// Trusted USDC token movement.
    TokenTransfer,
}

/// One trusted comparison of simulated output with decoded intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationEffect {
    kind: SimulationEffectKind,
    comparison: SimulationComparison,
    summary: String,
}

impl SimulationEffect {
    /// Effect category.
    #[must_use]
    pub const fn kind(&self) -> SimulationEffectKind {
        self.kind
    }
    /// Intent comparison.
    #[must_use]
    pub const fn comparison(&self) -> SimulationComparison {
        self.comparison
    }
    /// Trusted concise effect explanation.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// Simulation result bound to one candidate and observed canonical block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSimulation {
    state: SimulationState,
    summary: String,
    revert_reason: Option<String>,
    candidate_digest: String,
    chain_id: u64,
    block_number: String,
    block_hash: String,
    effects: Vec<SimulationEffect>,
}

impl TransactionSimulation {
    /// Overall simulation state.
    #[must_use]
    pub const fn state(&self) -> SimulationState {
        self.state
    }
    /// Trusted simulation summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
    /// Safely decoded revert reason when available.
    #[must_use]
    pub fn revert_reason(&self) -> Option<&str> {
        self.revert_reason.as_deref()
    }
    /// Keccak-256 digest of the exact unsigned candidate and block context.
    #[must_use]
    pub fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }
    /// Bound EIP-155 chain identifier.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    /// Bound canonical block number.
    #[must_use]
    pub fn block_number(&self) -> &str {
        &self.block_number
    }
    /// Bound canonical block hash.
    #[must_use]
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    /// Compared effects.
    #[must_use]
    pub fn effects(&self) -> &[SimulationEffect] {
        &self.effects
    }
}

pub(crate) struct ObservedBlock {
    pub(crate) number: Uint256,
    pub(crate) hash: String,
    pub(crate) base_fee: Uint256,
}

pub(crate) fn observe_latest_block(client: &RpcClient<'_>) -> Result<ObservedBlock, RpcError> {
    let block = client.value_result("eth_getBlockByNumber", &json!(["latest", false]))?;
    let object = block.as_object().ok_or(RpcError::MalformedResponse)?;
    let number = object
        .get("number")
        .and_then(Value::as_str)
        .and_then(|value| Uint256::from_quantity(value).ok())
        .ok_or(RpcError::MalformedResponse)?;
    let base_fee = object
        .get("baseFeePerGas")
        .and_then(Value::as_str)
        .and_then(|value| Uint256::from_quantity(value).ok())
        .ok_or(RpcError::MalformedResponse)?;
    let hash = object
        .get("hash")
        .and_then(Value::as_str)
        .filter(|value| is_hash(value))
        .ok_or(RpcError::MalformedResponse)?
        .to_ascii_lowercase();
    Ok(ObservedBlock {
        number,
        hash,
        base_fee,
    })
}

#[derive(Clone, Copy)]
pub(crate) struct SimulationTransaction<'a> {
    pub(crate) network: BaseNetwork,
    pub(crate) sender: &'a PublicEvmAddress,
    pub(crate) destination: &'a PublicEvmAddress,
    pub(crate) value: Uint256,
    pub(crate) nonce: Uint256,
    pub(crate) gas_limit: Uint256,
    pub(crate) priority_fee: Uint256,
    pub(crate) max_fee: Uint256,
    pub(crate) data: &'a str,
}

#[derive(Clone, Copy)]
pub(crate) enum SimulationIntent<'a> {
    Native {
        recipient: &'a PublicEvmAddress,
        value: Uint256,
    },
    TokenTransfer {
        token_contract: &'a str,
        recipient: &'a PublicEvmAddress,
        raw_amount: Uint256,
    },
}

pub(crate) fn simulate_transaction(
    client: &RpcClient<'_>,
    block: &ObservedBlock,
    transaction: &SimulationTransaction<'_>,
    intent: SimulationIntent<'_>,
) -> TransactionSimulation {
    let digest = candidate_digest(transaction, block);
    if !intent_matches(transaction, &intent) {
        return outcome(
            SimulationState::Mismatch,
            "Prepared transaction fields do not match independently decoded intent",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Mismatch)],
        );
    }
    let call = transaction_object(transaction);
    let block_selector = json!({ "blockHash": block.hash, "requireCanonical": true });
    match client.call_outcome(&json!([call, block_selector])) {
        Ok(RpcCallOutcome::Reverted { code, data }) => outcome(
            SimulationState::Reverted,
            "Exact candidate execution reverted",
            Some(decode_revert(code, data.as_deref())),
            digest,
            transaction.network,
            block,
            Vec::new(),
        ),
        Ok(RpcCallOutcome::Succeeded(output)) => {
            successful_outcome(client, transaction, intent, block, digest, &output)
        }
        Err(error) => outcome(
            SimulationState::Unavailable,
            unavailable_summary(error),
            None,
            digest,
            transaction.network,
            block,
            Vec::new(),
        ),
    }
}

fn successful_outcome(
    client: &RpcClient<'_>,
    transaction: &SimulationTransaction<'_>,
    intent: SimulationIntent<'_>,
    block: &ObservedBlock,
    digest: String,
    output: &str,
) -> TransactionSimulation {
    match intent {
        SimulationIntent::TokenTransfer { .. } if output == format!("0x{:0>64}", "1") => outcome(
            SimulationState::Succeeded,
            "Trusted token call succeeded and returned canonical true",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Matched)],
        ),
        SimulationIntent::TokenTransfer { .. } => outcome(
            SimulationState::Mismatch,
            "Token simulation did not return canonical true",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Mismatch)],
        ),
        SimulationIntent::Native { .. } => {
            native_success(client, transaction, intent, block, digest)
        }
    }
}

fn native_success(
    client: &RpcClient<'_>,
    transaction: &SimulationTransaction<'_>,
    intent: SimulationIntent<'_>,
    block: &ObservedBlock,
    digest: String,
) -> TransactionSimulation {
    let selector = json!({ "blockHash": block.hash, "requireCanonical": true });
    match client.string_result(
        "eth_getCode",
        &json!([transaction.destination.as_str(), selector]),
    ) {
        Ok(code) if code == "0x" => outcome(
            SimulationState::Succeeded,
            "Exact native transfer to a code-free account executed successfully",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Matched)],
        ),
        Ok(code) if valid_code(&code) => outcome(
            SimulationState::Inconclusive,
            "Contract call executed, but standard RPC exposed no native balance state diff",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Inconclusive)],
        ),
        Ok(_) | Err(_) => outcome(
            SimulationState::Inconclusive,
            "Execution succeeded, but recipient code and balance effects could not be confirmed",
            None,
            digest,
            transaction.network,
            block,
            vec![effect(&intent, SimulationComparison::Inconclusive)],
        ),
    }
}

fn transaction_object(transaction: &SimulationTransaction<'_>) -> Value {
    json!({
        "type": "0x2",
        "chainId": Uint256::from_u64(transaction.network.descriptor().chain_id()).to_quantity(),
        "from": transaction.sender.as_str(),
        "to": transaction.destination.as_str(),
        "value": transaction.value.to_quantity(),
        "nonce": transaction.nonce.to_quantity(),
        "gas": transaction.gas_limit.to_quantity(),
        "maxPriorityFeePerGas": transaction.priority_fee.to_quantity(),
        "maxFeePerGas": transaction.max_fee.to_quantity(),
        "data": transaction.data,
        "accessList": [],
    })
}

fn intent_matches(transaction: &SimulationTransaction<'_>, intent: &SimulationIntent<'_>) -> bool {
    match intent {
        SimulationIntent::Native { recipient, value } => {
            transaction.destination == *recipient
                && transaction.value == *value
                && transaction.data == "0x"
        }
        SimulationIntent::TokenTransfer {
            token_contract,
            recipient,
            raw_amount,
        } => {
            transaction.value.is_zero()
                && transaction.destination.as_str() == *token_contract
                && decode_token_transfer(transaction.network, token_contract, transaction.data)
                    .is_ok_and(|decoded| {
                        decoded.recipient() == recipient.checksum()
                            && decoded.raw_amount() == raw_amount.to_decimal()
                    })
        }
    }
}

fn effect(intent: &SimulationIntent<'_>, comparison: SimulationComparison) -> SimulationEffect {
    let (kind, summary) = match intent {
        SimulationIntent::Native { recipient, value } => (
            SimulationEffectKind::NativeTransfer,
            format!("{} wei to {}", value.to_decimal(), recipient.checksum()),
        ),
        SimulationIntent::TokenTransfer {
            recipient,
            raw_amount,
            ..
        } => (
            SimulationEffectKind::TokenTransfer,
            format!(
                "{} raw USDC units to {}",
                raw_amount.to_decimal(),
                recipient.checksum()
            ),
        ),
    };
    SimulationEffect {
        kind,
        comparison,
        summary,
    }
}

fn candidate_digest(transaction: &SimulationTransaction<'_>, block: &ObservedBlock) -> String {
    let mut hasher = Keccak256::new();
    for field in [
        transaction.network.descriptor().chain_id().to_string(),
        transaction.sender.as_str().to_owned(),
        transaction.destination.as_str().to_owned(),
        transaction.value.to_decimal(),
        transaction.nonce.to_decimal(),
        transaction.gas_limit.to_decimal(),
        transaction.priority_fee.to_decimal(),
        transaction.max_fee.to_decimal(),
        transaction.data.to_ascii_lowercase(),
        block.number.to_decimal(),
        block.hash.clone(),
    ] {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(66);
    output.push_str("0x");
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn outcome(
    state: SimulationState,
    summary: impl Into<String>,
    revert_reason: Option<String>,
    candidate_digest: String,
    network: BaseNetwork,
    block: &ObservedBlock,
    effects: Vec<SimulationEffect>,
) -> TransactionSimulation {
    TransactionSimulation {
        state,
        summary: summary.into(),
        revert_reason,
        candidate_digest,
        chain_id: network.descriptor().chain_id(),
        block_number: block.number.to_decimal(),
        block_hash: block.hash.clone(),
        effects,
    }
}

fn decode_revert(code: i64, data: Option<&str>) -> String {
    let Some(hex) = data.and_then(|value| value.strip_prefix("0x")) else {
        return format!("Execution reverted (RPC code {code})");
    };
    if let Some(reason) = decode_error_string(hex) {
        return reason;
    }
    if hex.starts_with(PANIC_SELECTOR) && hex.len() == 72 {
        return Uint256::from_abi_word(&format!("0x{}", &hex[8..])).map_or_else(
            |_| "EVM panic".to_owned(),
            |value| {
                format!(
                    "EVM panic code 0x{:x}",
                    value.try_to_u64().unwrap_or_default()
                )
            },
        );
    }
    if hex.len() >= 8 {
        format!("Custom error selector 0x{}", &hex[..8])
    } else {
        format!("Execution reverted (RPC code {code})")
    }
}

fn decode_error_string(hex: &str) -> Option<String> {
    if !hex.starts_with(ERROR_STRING_SELECTOR) || hex.len() < 8 + 64 * 3 {
        return None;
    }
    let offset = Uint256::from_abi_word(&format!("0x{}", &hex[8..72]))
        .ok()?
        .try_to_u64()?;
    if offset != 32 {
        return None;
    }
    let length = Uint256::from_abi_word(&format!("0x{}", &hex[72..136]))
        .ok()?
        .try_to_u64()?;
    let length = usize::try_from(length).ok()?;
    if length > 512 || hex.len() < 136 + length * 2 {
        return None;
    }
    let bytes = decode_hex(&hex[136..136 + length * 2])?;
    let reason = String::from_utf8(bytes).ok()?;
    reason
        .chars()
        .all(|character| !character.is_control())
        .then(|| format!("Reverted: {reason}"))
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn unavailable_summary(error: RpcError) -> &'static str {
    match error {
        RpcError::Transport | RpcError::HttpStatus(_) => {
            "Simulation unavailable because the RPC endpoint could not complete the exact call"
        }
        RpcError::ResponseTooLarge => {
            "Simulation unavailable because the RPC response exceeded its safety limit"
        }
        RpcError::MalformedResponse => {
            "Simulation inconclusive because the RPC returned malformed call data"
        }
        RpcError::Rejected(_) => {
            "Exact-block simulation is unsupported or unavailable at this RPC endpoint"
        }
    }
}

fn is_hash(value: &str) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn valid_code(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|hex| {
        !hex.is_empty()
            && hex.len().is_multiple_of(2)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revert_reason_decoding_is_bounded_and_sanitized() {
        let reason = "Insufficient funds";
        let data = format!(
            "0x{ERROR_STRING_SELECTOR}{:0>64}{:0>64}{}{}",
            "20",
            format!("{:x}", reason.len()),
            reason
                .as_bytes()
                .iter()
                .fold(String::new(), |mut text, byte| {
                    let _ = write!(text, "{byte:02x}");
                    text
                }),
            "0".repeat(28)
        );
        assert_eq!(
            decode_revert(3, Some(&data)),
            "Reverted: Insufficient funds"
        );
        assert_eq!(
            decode_revert(-32_000, Some("0xdeadbeef")),
            "Custom error selector 0xdeadbeef"
        );
    }

    #[test]
    fn candidate_digest_changes_with_block_and_call_fields() -> Result<(), crate::EvmBalanceError> {
        let sender = PublicEvmAddress::new("0x9858effd232b4033e47d90003d41ec34ecaeda94")?;
        let recipient = PublicEvmAddress::new("0xde709f2102306220921060314715629080e2fb77")?;
        let transaction = SimulationTransaction {
            network: BaseNetwork::Mainnet,
            sender: &sender,
            destination: &recipient,
            value: Uint256::from_u64(1),
            nonce: Uint256::ZERO,
            gas_limit: Uint256::from_u64(21_000),
            priority_fee: Uint256::from_u64(1),
            max_fee: Uint256::from_u64(2),
            data: "0x",
        };
        let first = ObservedBlock {
            number: Uint256::from_u64(1),
            hash: format!("0x{}", "11".repeat(32)),
            base_fee: Uint256::from_u64(1),
        };
        let second = ObservedBlock {
            number: Uint256::from_u64(2),
            hash: format!("0x{}", "22".repeat(32)),
            base_fee: Uint256::from_u64(1),
        };
        assert_ne!(
            candidate_digest(&transaction, &first),
            candidate_digest(&transaction, &second)
        );
        let changed_transaction = SimulationTransaction {
            value: Uint256::from_u64(2),
            ..transaction
        };
        assert_ne!(
            candidate_digest(&transaction, &first),
            candidate_digest(&changed_transaction, &first)
        );
        Ok(())
    }
}
