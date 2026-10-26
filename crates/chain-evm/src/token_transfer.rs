//! Trusted ERC-20 transfer construction, independent decoding and preparation.

use std::{error::Error, fmt, fmt::Write as _};

use serde::Serialize;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};

use crate::{
    BaseAssetDescriptor, BaseNetwork, EvmRpcUrl, PublicEvmAddress, RpcChainIdError,
    rpc::{RpcClient, RpcError, Uint256, format_units},
    simulation::{
        SimulationIntent, SimulationTransaction, TransactionSimulation, observe_latest_block,
        simulate_transaction,
    },
    trusted_assets,
};

const TRANSFER_SELECTOR: &str = "a9059cbb";
const BALANCE_OF_SELECTOR: &str = "70a08231";
const GAS_PRICE_ORACLE: &str = "0x420000000000000000000000000000000000000f";
const ETH_DECIMALS: u8 = 18;

/// Public inputs for preparing one transfer of the configured Base USDC token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenTransferRequest {
    sender: PublicEvmAddress,
    recipient: PublicEvmAddress,
    amount: String,
}

impl TokenTransferRequest {
    /// Validates public addresses and an exact positive USDC amount.
    ///
    /// # Errors
    ///
    /// Returns a closed address, syntax, precision, zero or overflow error.
    pub fn new(sender: &str, recipient: &str, amount: &str) -> Result<Self, TokenTransferError> {
        let sender =
            PublicEvmAddress::new(sender).map_err(|_| TokenTransferError::InvalidSender)?;
        let recipient =
            PublicEvmAddress::new(recipient).map_err(|_| TokenTransferError::InvalidRecipient)?;
        let raw = parse_units(amount, 6)?;
        if raw.is_zero() {
            return Err(TokenTransferError::ZeroAmount);
        }
        Ok(Self {
            sender,
            recipient,
            amount: format_units(&raw.to_decimal(), 6),
        })
    }
}

/// Independently decoded, trusted ERC-20 transfer intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedTokenTransfer {
    token_contract: String,
    symbol: String,
    decimals: u8,
    recipient: String,
    amount: String,
    raw_amount: String,
}

impl DecodedTokenTransfer {
    /// Checksummed configured token contract.
    #[must_use]
    pub fn token_contract(&self) -> &str {
        &self.token_contract
    }
    /// Trusted token symbol.
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    /// Trusted token decimals.
    #[must_use]
    pub const fn decimals(&self) -> u8 {
        self.decimals
    }
    /// Checksummed calldata recipient.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }
    /// Exact human-readable calldata amount.
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }
    /// Exact calldata integer amount.
    #[must_use]
    pub fn raw_amount(&self) -> &str {
        &self.raw_amount
    }
}

/// Fully populated unsigned EIP-1559 ERC-20 transfer candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTransferCandidate {
    chain_id: u64,
    caip2_id: String,
    sender: String,
    decoded: DecodedTokenTransfer,
    token_balance_raw: String,
    token_balance: String,
    native_balance_wei: String,
    nonce: String,
    gas_limit: String,
    base_fee_per_gas_wei: String,
    max_priority_fee_per_gas_wei: String,
    max_fee_per_gas_wei: String,
    maximum_execution_fee_wei: String,
    maximum_l1_fee_wei: String,
    operator_fee_wei: String,
    maximum_network_fee_wei: String,
    maximum_network_fee_eth: String,
    transaction_type: u8,
    data: String,
    simulation: TransactionSimulation,
}

macro_rules! string_getter {
    ($doc:literal, $name:ident) => {
        #[doc = $doc]
        #[must_use]
        pub fn $name(&self) -> &str {
            &self.$name
        }
    };
}

impl TokenTransferCandidate {
    /// EIP-155 chain identifier.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    string_getter!("CAIP-2 network identity.", caip2_id);
    string_getter!("Checksummed sending account.", sender);
    /// Independently decoded intent.
    #[must_use]
    pub const fn decoded(&self) -> &DecodedTokenTransfer {
        &self.decoded
    }
    string_getter!(
        "Pending raw token balance used for validation.",
        token_balance_raw
    );
    string_getter!(
        "Pending formatted token balance used for validation.",
        token_balance
    );
    string_getter!(
        "Pending native ETH balance used for fee validation.",
        native_balance_wei
    );
    string_getter!("Pending account nonce.", nonce);
    string_getter!("RPC-estimated gas limit.", gas_limit);
    string_getter!("Latest observed base fee per gas.", base_fee_per_gas_wei);
    string_getter!(
        "Suggested priority fee per gas.",
        max_priority_fee_per_gas_wei
    );
    string_getter!("Maximum EIP-1559 fee per gas.", max_fee_per_gas_wei);
    string_getter!("Maximum L2 execution fee.", maximum_execution_fee_wei);
    string_getter!("Maximum Base L1 data fee.", maximum_l1_fee_wei);
    string_getter!("Current Base operator fee.", operator_fee_wei);
    string_getter!(
        "Combined maximum network fee in wei.",
        maximum_network_fee_wei
    );
    string_getter!(
        "Combined maximum network fee in ETH.",
        maximum_network_fee_eth
    );
    /// EIP-2718 transaction type.
    #[must_use]
    pub const fn transaction_type(&self) -> u8 {
        self.transaction_type
    }
    string_getter!("Exact decoded ERC-20 transfer calldata.", data);
    /// Exact-block simulation bound to this candidate.
    #[must_use]
    pub const fn simulation(&self) -> &TransactionSimulation {
        &self.simulation
    }
}

/// Decodes strict `transfer(address,uint256)` calldata using chain-bound metadata.
///
/// # Errors
///
/// Fails closed for an unknown token, non-canonical calldata or zero amount.
pub fn decode_token_transfer(
    network: BaseNetwork,
    token_contract: &str,
    data: &str,
) -> Result<DecodedTokenTransfer, TokenTransferError> {
    let asset = trusted_assets(network)[1];
    let configured = asset
        .token_contract()
        .ok_or(TokenTransferError::UnknownToken)?;
    let supplied =
        PublicEvmAddress::new(token_contract).map_err(|_| TokenTransferError::UnknownToken)?;
    let trusted =
        PublicEvmAddress::new(configured).map_err(|_| TokenTransferError::UnknownToken)?;
    if supplied != trusted {
        return Err(TokenTransferError::UnknownToken);
    }
    let hex = data
        .strip_prefix("0x")
        .ok_or(TokenTransferError::MalformedCalldata)?;
    if hex.len() != 136 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TokenTransferError::MalformedCalldata);
    }
    if !hex[..8].eq_ignore_ascii_case(TRANSFER_SELECTOR)
        || hex[8..32].bytes().any(|byte| byte != b'0')
    {
        return Err(TokenTransferError::MalformedCalldata);
    }
    let recipient = PublicEvmAddress::new(&format!("0x{}", &hex[32..72]))
        .map_err(|_| TokenTransferError::MalformedCalldata)?;
    let raw = Uint256::from_abi_word(&format!("0x{}", &hex[72..136]))
        .map_err(|_| TokenTransferError::MalformedCalldata)?;
    if raw.is_zero() {
        return Err(TokenTransferError::ZeroAmount);
    }
    let raw_amount = raw.to_decimal();
    Ok(DecodedTokenTransfer {
        token_contract: trusted.checksum(),
        symbol: asset.symbol().to_owned(),
        decimals: asset.decimals(),
        recipient: recipient.checksum(),
        amount: format_units(&raw_amount, asset.decimals()),
        raw_amount,
    })
}

/// Constructs, independently decodes and fully prepares one Base USDC transfer.
///
/// # Errors
///
/// Fails closed on construction/decoding mismatch, chain mismatch, malformed
/// RPC data, estimation failure, fee overflow or either insufficient balance.
pub fn prepare_token_transfer(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    request: &TokenTransferRequest,
) -> Result<TokenTransferCandidate, TokenTransferError> {
    let asset = trusted_assets(network)[1];
    let contract = asset
        .token_contract()
        .ok_or(TokenTransferError::UnknownToken)?;
    let contract_address =
        PublicEvmAddress::new(contract).map_err(|_| TokenTransferError::UnknownToken)?;
    let raw = parse_units(&request.amount, asset.decimals())?;
    let data = encode_transfer(&request.recipient, raw);
    let decoded = decode_token_transfer(network, contract, &data)?;
    if decoded.recipient != request.recipient.checksum()
        || decoded.raw_amount != raw.to_decimal()
        || decoded.amount != request.amount
    {
        return Err(TokenTransferError::IntentMismatch);
    }

    let client = RpcClient::new(endpoint)?;
    let chain_id = client.string_result("eth_chainId", &json!([]))?;
    network.validate_rpc_chain_id(&chain_id)?;
    prepare_token_rpc(
        network,
        &client,
        request,
        PreparedTokenIntent {
            asset,
            contract_address,
            raw,
            data,
            decoded,
        },
    )
}

struct PreparedTokenIntent {
    asset: BaseAssetDescriptor,
    contract_address: PublicEvmAddress,
    raw: Uint256,
    data: String,
    decoded: DecodedTokenTransfer,
}

fn prepare_token_rpc(
    network: BaseNetwork,
    client: &RpcClient<'_>,
    request: &TokenTransferRequest,
    intent: PreparedTokenIntent,
) -> Result<TokenTransferCandidate, TokenTransferError> {
    let contract = intent
        .asset
        .token_contract()
        .ok_or(TokenTransferError::UnknownToken)?;
    let native_balance = quantity(
        client,
        "eth_getBalance",
        &json!([request.sender.as_str(), "pending"]),
    )?;
    let token_balance = token_balance(client, contract, &request.sender)?;
    if intent.raw > token_balance {
        return Err(TokenTransferError::InsufficientTokenBalance {
            available: token_balance.to_decimal(),
            required: intent.raw.to_decimal(),
        });
    }
    let nonce = quantity(
        client,
        "eth_getTransactionCount",
        &json!([request.sender.as_str(), "pending"]),
    )?;
    let priority = quantity(client, "eth_maxPriorityFeePerGas", &json!([]))?;
    let observed_block = observe_latest_block(client)?;
    let base = observed_block.base_fee;
    let max_fee = base
        .checked_add(base)
        .and_then(|value| value.checked_add(priority))
        .ok_or(TokenTransferError::ArithmeticOverflow)?;
    if priority > max_fee {
        return Err(TokenTransferError::MalformedRpcResponse);
    }
    let transaction = token_transaction_object(
        network,
        request,
        contract,
        &intent.data,
        nonce,
        priority,
        max_fee,
    );
    let gas = quantity(client, "eth_estimateGas", &json!([transaction]))?;
    if gas.is_zero() {
        return Err(TokenTransferError::MalformedRpcResponse);
    }
    let unsigned_size = unsigned_transaction_size(network, nonce, priority, max_fee, gas, 68);
    let (l1, operator) = network_fees(network, client, unsigned_size, gas)?;
    let (execution, total) = token_fee_total(gas, max_fee, l1, operator)?;
    if total > native_balance {
        return Err(TokenTransferError::InsufficientNativeFeeBalance {
            available_wei: native_balance.to_decimal(),
            required_wei: total.to_decimal(),
        });
    }
    let simulation_transaction = SimulationTransaction {
        network,
        sender: &request.sender,
        destination: &intent.contract_address,
        value: Uint256::ZERO,
        nonce,
        gas_limit: gas,
        priority_fee: priority,
        max_fee,
        data: &intent.data,
    };
    let simulation = simulate_prepared_token(
        client,
        &observed_block,
        &simulation_transaction,
        contract,
        request,
        intent.raw,
    );
    let token_balance_raw = token_balance.to_decimal();
    Ok(TokenTransferCandidate {
        chain_id: network.descriptor().chain_id(),
        caip2_id: network.descriptor().caip2_id().to_owned(),
        sender: request.sender.checksum(),
        decoded: intent.decoded,
        token_balance: format_units(&token_balance_raw, intent.asset.decimals()),
        token_balance_raw,
        native_balance_wei: native_balance.to_decimal(),
        nonce: nonce.to_decimal(),
        gas_limit: gas.to_decimal(),
        base_fee_per_gas_wei: base.to_decimal(),
        max_priority_fee_per_gas_wei: priority.to_decimal(),
        max_fee_per_gas_wei: max_fee.to_decimal(),
        maximum_execution_fee_wei: execution.to_decimal(),
        maximum_l1_fee_wei: l1.to_decimal(),
        operator_fee_wei: operator.to_decimal(),
        maximum_network_fee_wei: total.to_decimal(),
        maximum_network_fee_eth: format_units(&total.to_decimal(), ETH_DECIMALS),
        transaction_type: 2,
        data: intent.data,
        simulation,
    })
}

fn network_fees(
    network: BaseNetwork,
    client: &RpcClient<'_>,
    unsigned_size: usize,
    gas: Uint256,
) -> Result<(Uint256, Uint256), TokenTransferError> {
    if !network.is_base() {
        return Ok((Uint256::ZERO, Uint256::ZERO));
    }
    Ok((
        oracle_call(
            client,
            "getL1FeeUpperBound(uint256)",
            Uint256::from_u64(unsigned_size as u64),
        )?,
        oracle_call(client, "getOperatorFee(uint256)", gas)?,
    ))
}

fn token_fee_total(
    gas: Uint256,
    max_fee: Uint256,
    l1: Uint256,
    operator: Uint256,
) -> Result<(Uint256, Uint256), TokenTransferError> {
    let execution = gas
        .checked_mul(max_fee)
        .ok_or(TokenTransferError::ArithmeticOverflow)?;
    let total = execution
        .checked_add(l1)
        .and_then(|value| value.checked_add(operator))
        .ok_or(TokenTransferError::ArithmeticOverflow)?;
    Ok((execution, total))
}

fn simulate_prepared_token(
    client: &RpcClient<'_>,
    block: &crate::simulation::ObservedBlock,
    transaction: &SimulationTransaction<'_>,
    contract: &str,
    request: &TokenTransferRequest,
    raw_amount: Uint256,
) -> TransactionSimulation {
    simulate_transaction(
        client,
        block,
        transaction,
        SimulationIntent::TokenTransfer {
            token_contract: contract,
            recipient: &request.recipient,
            raw_amount,
        },
    )
}

fn token_transaction_object(
    network: BaseNetwork,
    request: &TokenTransferRequest,
    contract: &str,
    data: &str,
    nonce: Uint256,
    priority: Uint256,
    max_fee: Uint256,
) -> Value {
    json!({
        "type": "0x2", "chainId": Uint256::from_u64(network.descriptor().chain_id()).to_quantity(),
        "from": request.sender.as_str(), "to": contract, "value": "0x0", "nonce": nonce.to_quantity(),
        "maxPriorityFeePerGas": priority.to_quantity(), "maxFeePerGas": max_fee.to_quantity(),
        "data": data, "accessList": [],
    })
}

fn parse_units(amount: &str, decimals: u8) -> Result<Uint256, TokenTransferError> {
    if amount.is_empty() || amount.trim() != amount || amount.starts_with(['+', '-']) {
        return Err(TokenTransferError::InvalidAmount);
    }
    let mut parts = amount.split('.');
    let whole = parts.next().ok_or(TokenTransferError::InvalidAmount)?;
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(TokenTransferError::InvalidAmount);
    }
    let fraction = fraction.unwrap_or("");
    if fraction.len() > usize::from(decimals) {
        return Err(TokenTransferError::ExcessPrecision);
    }
    let raw = format!("{whole}{fraction:0<width$}", width = usize::from(decimals));
    let significant = raw.trim_start_matches('0');
    if significant.is_empty() {
        Ok(Uint256::ZERO)
    } else {
        Uint256::from_decimal(significant).map_err(|_| TokenTransferError::AmountOverflow)
    }
}

fn encode_transfer(recipient: &PublicEvmAddress, amount: Uint256) -> String {
    format!(
        "0x{TRANSFER_SELECTOR}{:0>64}{}",
        recipient.as_str().trim_start_matches("0x"),
        amount.to_abi_word()
    )
}

fn token_balance(
    client: &RpcClient<'_>,
    contract: &str,
    owner: &PublicEvmAddress,
) -> Result<Uint256, TokenTransferError> {
    let data = format!(
        "0x{BALANCE_OF_SELECTOR}{:0>64}",
        owner.as_str().trim_start_matches("0x")
    );
    let result = client.string_result(
        "eth_call",
        &json!([{ "to": contract, "data": data }, "pending"]),
    )?;
    Uint256::from_abi_word(&result).map_err(|_| TokenTransferError::MalformedRpcResponse)
}

fn quantity(
    client: &RpcClient<'_>,
    method: &str,
    params: &Value,
) -> Result<Uint256, TokenTransferError> {
    Uint256::from_quantity(&client.string_result(method, params)?)
        .map_err(|_| TokenTransferError::MalformedRpcResponse)
}

fn oracle_call(
    client: &RpcClient<'_>,
    signature: &str,
    argument: Uint256,
) -> Result<Uint256, TokenTransferError> {
    let hash = Keccak256::digest(signature.as_bytes());
    let selector = hash[..4].iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    });
    let result = client.string_result("eth_call", &json!([{ "to": GAS_PRICE_ORACLE, "data": format!("0x{selector}{}", argument.to_abi_word()) }, "latest"]))?;
    Uint256::from_abi_word(&result).map_err(|_| TokenTransferError::MalformedRpcResponse)
}

fn unsigned_transaction_size(
    network: BaseNetwork,
    nonce: Uint256,
    priority: Uint256,
    max_fee: Uint256,
    gas: Uint256,
    data_len: usize,
) -> usize {
    let payload = [
        Uint256::from_u64(network.descriptor().chain_id()),
        nonce,
        priority,
        max_fee,
        gas,
        Uint256::ZERO,
    ]
    .into_iter()
    .map(rlp_integer_size)
    .sum::<usize>()
        + rlp_bytes(20)
        + rlp_bytes(data_len)
        + 1;
    1 + rlp_list(payload)
}
fn rlp_integer_size(value: Uint256) -> usize {
    if value < Uint256::from_u64(128) {
        1
    } else {
        rlp_bytes(value.byte_len())
    }
}
fn rlp_bytes(length: usize) -> usize {
    if length <= 55 {
        1 + length
    } else {
        1 + length_len(length) + length
    }
}
fn rlp_list(length: usize) -> usize {
    if length <= 55 {
        1 + length
    } else {
        1 + length_len(length) + length
    }
}
fn length_len(mut length: usize) -> usize {
    let mut bytes = 0;
    while length != 0 {
        bytes += 1;
        length >>= 8;
    }
    bytes
}

/// Closed token-transfer preparation failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenTransferError {
    /// Sender address is malformed.
    InvalidSender,
    /// Recipient address is malformed.
    InvalidRecipient,
    /// Human-readable amount syntax is malformed.
    InvalidAmount,
    /// Amount resolves to zero base units.
    ZeroAmount,
    /// Amount exceeds the configured token precision.
    ExcessPrecision,
    /// Amount cannot fit in uint256.
    AmountOverflow,
    /// Token contract is not the configured token for this chain.
    UnknownToken,
    /// Calldata is not one canonical transfer call.
    MalformedCalldata,
    /// Decoded intent differs from the construction inputs.
    IntentMismatch,
    /// HTTPS transport could not complete.
    Transport,
    /// Endpoint returned a non-success HTTP status.
    HttpStatus(u16),
    /// Bounded response exceeded its limit.
    ResponseTooLarge,
    /// Endpoint returned a JSON-RPC rejection code.
    RpcRejected(i64),
    /// RPC chain does not match the selected EVM network.
    ChainId(RpcChainIdError),
    /// RPC returned malformed preparation data.
    MalformedRpcResponse,
    /// Fee arithmetic overflowed uint256.
    ArithmeticOverflow,
    /// Pending token balance is below the decoded amount.
    InsufficientTokenBalance {
        /// Available token base units.
        available: String,
        /// Required token base units.
        required: String,
    },
    /// Pending native balance cannot cover maximum network fees.
    InsufficientNativeFeeBalance {
        /// Available ETH balance in wei.
        available_wei: String,
        /// Maximum required network fee in wei.
        required_wei: String,
    },
}

impl fmt::Display for TokenTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSender => f.write_str("Enter a valid 0x-prefixed sender address"),
            Self::InvalidRecipient => f.write_str("Enter a valid 0x-prefixed recipient address"),
            Self::InvalidAmount => f.write_str("Enter a USDC amount using decimal digits"),
            Self::ZeroAmount => f.write_str("USDC amount must be greater than zero"),
            Self::ExcessPrecision => {
                f.write_str("USDC amount cannot use more than 6 decimal places")
            }
            Self::AmountOverflow => f.write_str("USDC amount exceeds the EVM uint256 limit"),
            Self::UnknownToken => {
                f.write_str("The token is not trusted for the selected EVM network")
            }
            Self::MalformedCalldata => {
                f.write_str("The ERC-20 transfer calldata could not be decoded exactly")
            }
            Self::IntentMismatch => {
                f.write_str("Decoded ERC-20 calldata does not match the requested transfer")
            }
            Self::Transport => f.write_str("The EVM RPC endpoint could not be reached"),
            Self::HttpStatus(status) => write!(f, "The EVM RPC endpoint returned HTTP {status}"),
            Self::ResponseTooLarge => f.write_str("The EVM RPC response exceeded its safety limit"),
            Self::RpcRejected(code) => write!(
                f,
                "The EVM RPC endpoint rejected token transfer preparation (code {code})"
            ),
            Self::ChainId(error) => error.fmt(f),
            Self::MalformedRpcResponse => {
                f.write_str("The EVM RPC endpoint returned invalid token transfer data")
            }
            Self::ArithmeticOverflow => {
                f.write_str("The prepared transfer fee exceeds the EVM uint256 limit")
            }
            Self::InsufficientTokenBalance {
                available,
                required,
            } => write!(
                f,
                "Insufficient USDC balance: {available} units available, {required} units required"
            ),
            Self::InsufficientNativeFeeBalance {
                available_wei,
                required_wei,
            } => write!(
                f,
                "Insufficient ETH for fees: {available_wei} wei available, {required_wei} wei required"
            ),
        }
    }
}
impl Error for TokenTransferError {}
impl From<RpcError> for TokenTransferError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Transport => Self::Transport,
            RpcError::HttpStatus(s) => Self::HttpStatus(s),
            RpcError::ResponseTooLarge => Self::ResponseTooLarge,
            RpcError::MalformedResponse => Self::MalformedRpcResponse,
            RpcError::Rejected(c) => Self::RpcRejected(c),
        }
    }
}
impl From<RpcChainIdError> for TokenTransferError {
    fn from(e: RpcChainIdError) -> Self {
        Self::ChainId(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_conversion_is_exact_and_closed() -> Result<(), TokenTransferError> {
        assert_eq!(parse_units("1.230000", 6)?.to_decimal(), "1230000");
        assert_eq!(parse_units("0.000001", 6)?.to_decimal(), "1");
        assert_eq!(
            parse_units("1.0000001", 6),
            Err(TokenTransferError::ExcessPrecision)
        );
        for invalid in ["", "1.", ".1", "-1", "+1", " 1", "1e2"] {
            assert_eq!(
                parse_units(invalid, 6),
                Err(TokenTransferError::InvalidAmount)
            );
        }
        Ok(())
    }

    #[test]
    fn decoder_reports_trusted_metadata_and_exact_intent() -> Result<(), TokenTransferError> {
        let recipient = PublicEvmAddress::new("0xde709f2102306220921060314715629080e2fb77")
            .map_err(|_| TokenTransferError::InvalidRecipient)?;
        let data = encode_transfer(&recipient, Uint256::from_u64(1_250_000));
        let decoded = decode_token_transfer(
            BaseNetwork::Mainnet,
            "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            &data,
        )?;
        assert_eq!(decoded.symbol(), "USDC");
        assert_eq!(decoded.decimals(), 6);
        assert_eq!(decoded.amount(), "1.25");
        assert_eq!(decoded.raw_amount(), "1250000");
        assert_eq!(
            decoded.recipient(),
            "0xde709f2102306220921060314715629080e2fb77"
        );
        Ok(())
    }

    #[test]
    fn decoder_rejects_unknown_token_bad_padding_and_trailing_data() {
        let valid = format!(
            "0x{TRANSFER_SELECTOR}{}{}",
            "0".repeat(24) + &"11".repeat(20),
            Uint256::from_u64(1).to_abi_word()
        );
        assert_eq!(
            decode_token_transfer(
                BaseNetwork::Mainnet,
                "0x0000000000000000000000000000000000000001",
                &valid
            ),
            Err(TokenTransferError::UnknownToken)
        );
        let bad_padding = format!(
            "0x{TRANSFER_SELECTOR}{}{}",
            "1".repeat(64),
            Uint256::from_u64(1).to_abi_word()
        );
        assert_eq!(
            decode_token_transfer(
                BaseNetwork::Mainnet,
                "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
                &bad_padding
            ),
            Err(TokenTransferError::MalformedCalldata)
        );
        assert_eq!(
            decode_token_transfer(
                BaseNetwork::Mainnet,
                "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
                &(valid + "00")
            ),
            Err(TokenTransferError::MalformedCalldata)
        );
    }
}
