//! Base native-transfer preparation without signing or broadcast authority.

use std::{error::Error, fmt, fmt::Write as _};

use serde::Serialize;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};

use crate::{
    BaseNetwork, EvmRpcUrl, PublicEvmAddress, RpcChainIdError,
    rpc::{RpcClient, RpcError, Uint256, format_units},
    simulation::{
        SimulationIntent, SimulationTransaction, TransactionSimulation, simulate_transaction,
    },
};

const ETH_DECIMALS: u8 = 18;
const GAS_PRICE_ORACLE: &str = "0x420000000000000000000000000000000000000f";

/// Public inputs required to prepare one native ETH transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTransferRequest {
    sender: PublicEvmAddress,
    recipient: PublicEvmAddress,
    amount_eth: String,
}

impl NativeTransferRequest {
    /// Validates public sender, recipient and an exact positive ETH amount.
    ///
    /// # Errors
    ///
    /// Returns a closed validation error before any RPC operation.
    pub fn new(
        sender: &str,
        recipient: &str,
        amount_eth: &str,
    ) -> Result<Self, NativeTransferError> {
        let sender =
            PublicEvmAddress::new(sender).map_err(|_| NativeTransferError::InvalidSender)?;
        let recipient =
            PublicEvmAddress::new(recipient).map_err(|_| NativeTransferError::InvalidRecipient)?;
        let value = parse_eth_amount(amount_eth)?;
        if value.is_zero() {
            return Err(NativeTransferError::ZeroAmount);
        }
        Ok(Self {
            sender,
            recipient,
            amount_eth: format_units(&value.to_decimal(), ETH_DECIMALS),
        })
    }
}

/// Immutable, serializable EIP-1559 native-transfer candidate.
///
/// This type contains public transaction data only. It has no signature,
/// private key, vault session, approval receipt or broadcast capability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeTransferCandidate {
    chain_id: u64,
    caip2_id: String,
    sender: String,
    recipient: String,
    amount_eth: String,
    value_wei: String,
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
    account_balance_wei: String,
    transaction_type: u8,
    data: String,
    simulation: TransactionSimulation,
}

impl NativeTransferCandidate {
    /// EIP-155 chain identifier.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    /// CAIP-2 network identity.
    #[must_use]
    pub fn caip2_id(&self) -> &str {
        &self.caip2_id
    }
    /// Checksummed sending account.
    #[must_use]
    pub fn sender(&self) -> &str {
        &self.sender
    }
    /// Checksummed recipient.
    #[must_use]
    pub fn recipient(&self) -> &str {
        &self.recipient
    }
    /// Exact normalized ETH amount.
    #[must_use]
    pub fn amount_eth(&self) -> &str {
        &self.amount_eth
    }
    /// Exact transfer value in wei.
    #[must_use]
    pub fn value_wei(&self) -> &str {
        &self.value_wei
    }
    /// Pending account nonce.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    /// Estimated gas limit.
    #[must_use]
    pub fn gas_limit(&self) -> &str {
        &self.gas_limit
    }
    /// Latest observed base fee per gas in wei.
    #[must_use]
    pub fn base_fee_per_gas_wei(&self) -> &str {
        &self.base_fee_per_gas_wei
    }
    /// Suggested maximum priority fee per gas in wei.
    #[must_use]
    pub fn max_priority_fee_per_gas_wei(&self) -> &str {
        &self.max_priority_fee_per_gas_wei
    }
    /// EIP-1559 maximum total fee per gas in wei.
    #[must_use]
    pub fn max_fee_per_gas_wei(&self) -> &str {
        &self.max_fee_per_gas_wei
    }
    /// Maximum L2 execution fee in wei.
    #[must_use]
    pub fn maximum_execution_fee_wei(&self) -> &str {
        &self.maximum_execution_fee_wei
    }
    /// Base `GasPriceOracle` L1 data-fee upper bound in wei.
    #[must_use]
    pub fn maximum_l1_fee_wei(&self) -> &str {
        &self.maximum_l1_fee_wei
    }
    /// Current Base operator fee for the estimated gas limit in wei.
    #[must_use]
    pub fn operator_fee_wei(&self) -> &str {
        &self.operator_fee_wei
    }
    /// Combined maximum execution and L1 data fee in wei.
    #[must_use]
    pub fn maximum_network_fee_wei(&self) -> &str {
        &self.maximum_network_fee_wei
    }
    /// Combined maximum fee denominated exactly in ETH.
    #[must_use]
    pub fn maximum_network_fee_eth(&self) -> &str {
        &self.maximum_network_fee_eth
    }
    /// Pending sender balance used for validation.
    #[must_use]
    pub fn account_balance_wei(&self) -> &str {
        &self.account_balance_wei
    }
    /// EIP-2718 transaction type, fixed to `2`.
    #[must_use]
    pub const fn transaction_type(&self) -> u8 {
        self.transaction_type
    }
    /// Native-transfer calldata, fixed to `0x`.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }
    /// Exact-block simulation bound to this candidate.
    #[must_use]
    pub const fn simulation(&self) -> &TransactionSimulation {
        &self.simulation
    }
}

/// Prepares a fully populated unsigned EIP-1559 transfer candidate.
///
/// # Errors
///
/// Fails closed on chain mismatch, malformed RPC data, gas-estimation failure,
/// fee overflow or insufficient balance. It never signs or broadcasts.
pub fn prepare_native_transfer(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    request: &NativeTransferRequest,
) -> Result<NativeTransferCandidate, NativeTransferError> {
    prepare_with_caps(network, endpoint, request, None)
}

/// Refreshes observations using the original execution fee cap and tip.
/// # Errors
/// Rejects changed chain, malformed observations, simulation/preparation failures
/// or insufficient funds. Never signs, broadcasts or changes the original.
pub fn refresh_native_transfer(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    old: &NativeTransferCandidate,
) -> Result<NativeTransferCandidate, NativeTransferError> {
    network.validate_rpc_chain_id(&format!("0x{:x}", old.chain_id()))?;
    let request = NativeTransferRequest::new(old.sender(), old.recipient(), old.amount_eth())?;
    prepare_with_caps(
        network,
        endpoint,
        &request,
        Some((
            old.max_priority_fee_per_gas_wei(),
            old.max_fee_per_gas_wei(),
            old.gas_limit(),
        )),
    )
}

fn prepare_with_caps(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    request: &NativeTransferRequest,
    caps: Option<(&str, &str, &str)>,
) -> Result<NativeTransferCandidate, NativeTransferError> {
    let client = RpcClient::new(endpoint)?;
    let chain_id_response = client.string_result("eth_chainId", &json!([]))?;
    network.validate_rpc_chain_id(&chain_id_response)?;

    let value = parse_eth_amount(&request.amount_eth)?;
    let balance = quantity_result(
        &client,
        "eth_getBalance",
        &json!([request.sender.as_str(), "pending"]),
    )?;
    if value > balance {
        return Err(insufficient(balance, value));
    }
    let nonce = quantity_result(
        &client,
        "eth_getTransactionCount",
        &json!([request.sender.as_str(), "pending"]),
    )?;
    let fees =
        crate::execution_fees::observe(&client, caps.map(|(priority, cap, _)| (priority, cap)))?;
    let observed_block = fees.block;
    let base_fee = observed_block.base_fee;
    let priority_fee = fees.priority;
    let max_fee = fees.cap;
    let transaction = transaction_object(network, request, value, nonce, priority_fee, max_fee);
    let estimate = quantity_result(&client, "eth_estimateGas", &json!([transaction]))?;
    let gas_limit = crate::execution_fees::gas_limit(estimate, caps.map(|(_, _, gas)| gas))?;
    let maximum_execution_fee = gas_limit
        .checked_mul(max_fee)
        .ok_or(NativeTransferError::ArithmeticOverflow)?;
    let unsigned_size =
        unsigned_transaction_size(network, value, nonce, gas_limit, priority_fee, max_fee);
    let (maximum_l1_fee, operator_fee) = if network.is_base() {
        (
            l1_fee_upper_bound(&client, unsigned_size)?,
            oracle_uint_call(&client, "getOperatorFee(uint256)", gas_limit)?,
        )
    } else {
        (Uint256::ZERO, Uint256::ZERO)
    };
    let maximum_network_fee = maximum_execution_fee
        .checked_add(maximum_l1_fee)
        .and_then(|subtotal| subtotal.checked_add(operator_fee))
        .ok_or(NativeTransferError::ArithmeticOverflow)?;
    let required = value
        .checked_add(maximum_network_fee)
        .ok_or(NativeTransferError::ArithmeticOverflow)?;
    if required > balance {
        return Err(insufficient(balance, required));
    }

    let simulation_transaction = SimulationTransaction {
        network,
        sender: &request.sender,
        destination: &request.recipient,
        value,
        nonce,
        gas_limit,
        priority_fee,
        max_fee,
        data: "0x",
    };
    let simulation = simulate_transaction(
        &client,
        &observed_block,
        &simulation_transaction,
        SimulationIntent::Native {
            recipient: &request.recipient,
            value,
        },
    );

    Ok(NativeTransferCandidate {
        chain_id: network.descriptor().chain_id(),
        caip2_id: network.descriptor().caip2_id().to_owned(),
        sender: request.sender.checksum(),
        recipient: request.recipient.checksum(),
        amount_eth: request.amount_eth.clone(),
        value_wei: value.to_decimal(),
        nonce: nonce.to_decimal(),
        gas_limit: gas_limit.to_decimal(),
        base_fee_per_gas_wei: base_fee.to_decimal(),
        max_priority_fee_per_gas_wei: priority_fee.to_decimal(),
        max_fee_per_gas_wei: max_fee.to_decimal(),
        maximum_execution_fee_wei: maximum_execution_fee.to_decimal(),
        maximum_l1_fee_wei: maximum_l1_fee.to_decimal(),
        operator_fee_wei: operator_fee.to_decimal(),
        maximum_network_fee_wei: maximum_network_fee.to_decimal(),
        maximum_network_fee_eth: format_units(&maximum_network_fee.to_decimal(), ETH_DECIMALS),
        account_balance_wei: balance.to_decimal(),
        transaction_type: 2,
        data: "0x".to_owned(),
        simulation,
    })
}

fn transaction_object(
    network: BaseNetwork,
    request: &NativeTransferRequest,
    value: Uint256,
    nonce: Uint256,
    priority_fee: Uint256,
    max_fee: Uint256,
) -> Value {
    json!({
        "type": "0x2",
        "chainId": Uint256::from_u64(network.descriptor().chain_id()).to_quantity(),
        "from": request.sender.as_str(),
        "to": request.recipient.as_str(),
        "value": value.to_quantity(),
        "nonce": nonce.to_quantity(),
        "maxPriorityFeePerGas": priority_fee.to_quantity(),
        "maxFeePerGas": max_fee.to_quantity(),
        "data": "0x",
        "accessList": [],
    })
}

fn quantity_result(
    client: &RpcClient<'_>,
    method: &str,
    params: &Value,
) -> Result<Uint256, NativeTransferError> {
    let result = client.string_result(method, params)?;
    Uint256::from_quantity(&result).map_err(|_| NativeTransferError::MalformedRpcResponse)
}

fn l1_fee_upper_bound(
    client: &RpcClient<'_>,
    unsigned_transaction_size: usize,
) -> Result<Uint256, NativeTransferError> {
    oracle_uint_call(
        client,
        "getL1FeeUpperBound(uint256)",
        u64::try_from(unsigned_transaction_size)
            .map(Uint256::from_u64)
            .map_err(|_| NativeTransferError::ArithmeticOverflow)?,
    )
}

fn oracle_uint_call(
    client: &RpcClient<'_>,
    signature: &str,
    argument: Uint256,
) -> Result<Uint256, NativeTransferError> {
    let selector_hash = Keccak256::digest(signature.as_bytes());
    let selector = selector_hash[..4]
        .iter()
        .fold(String::with_capacity(8), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        });
    let data = format!("0x{selector}{}", argument.to_abi_word());
    let result = client.string_result(
        "eth_call",
        &json!([{ "to": GAS_PRICE_ORACLE, "data": data }, "latest"]),
    )?;
    Uint256::from_abi_word(&result).map_err(|_| NativeTransferError::MalformedRpcResponse)
}

fn unsigned_transaction_size(
    network: BaseNetwork,
    value: Uint256,
    nonce: Uint256,
    gas_limit: Uint256,
    priority_fee: Uint256,
    max_fee: Uint256,
) -> usize {
    let payload = [
        Uint256::from_u64(network.descriptor().chain_id()),
        nonce,
        priority_fee,
        max_fee,
        gas_limit,
        value,
    ]
    .into_iter()
    .map(rlp_integer_size)
    .sum::<usize>()
        + rlp_byte_string_size(20)
        + rlp_byte_string_size(0)
        + 1;
    1 + rlp_list_size(payload)
}

fn rlp_integer_size(value: Uint256) -> usize {
    if value < Uint256::from_u64(128) {
        1
    } else {
        rlp_byte_string_size(value.byte_len())
    }
}

fn rlp_byte_string_size(length: usize) -> usize {
    if length <= 55 {
        1 + length
    } else {
        1 + length_of_length(length) + length
    }
}

fn rlp_list_size(payload_length: usize) -> usize {
    if payload_length <= 55 {
        1 + payload_length
    } else {
        1 + length_of_length(payload_length) + payload_length
    }
}

fn length_of_length(mut length: usize) -> usize {
    let mut bytes = 0;
    while length != 0 {
        bytes += 1;
        length >>= 8;
    }
    bytes
}

fn parse_eth_amount(amount: &str) -> Result<Uint256, NativeTransferError> {
    if amount.is_empty()
        || amount.trim() != amount
        || amount.starts_with('+')
        || amount.starts_with('-')
    {
        return Err(NativeTransferError::InvalidAmount);
    }
    let mut parts = amount.split('.');
    let whole = parts.next().ok_or(NativeTransferError::InvalidAmount)?;
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|value| {
            value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(NativeTransferError::InvalidAmount);
    }
    let fraction = fraction.unwrap_or("");
    if fraction.len() > usize::from(ETH_DECIMALS) {
        return Err(NativeTransferError::ExcessPrecision);
    }
    let raw = format!("{whole}{fraction:0<18}");
    let significant = raw.trim_start_matches('0');
    if significant.is_empty() {
        Ok(Uint256::ZERO)
    } else {
        Uint256::from_decimal(significant).map_err(|_| NativeTransferError::AmountOverflow)
    }
}

fn insufficient(balance: Uint256, required: Uint256) -> NativeTransferError {
    NativeTransferError::InsufficientBalance {
        balance_wei: balance.to_decimal(),
        required_wei: required.to_decimal(),
    }
}

/// Closed native-transfer preparation failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeTransferError {
    /// Sender address is malformed.
    InvalidSender,
    /// Recipient address is malformed.
    InvalidRecipient,
    /// Human-readable ETH amount syntax is malformed.
    InvalidAmount,
    /// Amount resolves to zero wei.
    ZeroAmount,
    /// Amount uses more than 18 fractional digits.
    ExcessPrecision,
    /// Amount cannot fit in uint256 wei.
    AmountOverflow,
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
    /// RPC returned a malformed field.
    MalformedRpcResponse,
    /// Fee or required-balance arithmetic overflowed uint256.
    ArithmeticOverflow,
    /// Current base fee no longer fits the immutable reviewed execution cap.
    ApprovedFeeCapExceeded,
    /// Current gas requirement exceeds the immutable reviewed gas limit.
    ApprovedGasLimitExceeded,
    /// Pending balance does not cover the value plus maximum fees.
    InsufficientBalance {
        /// Pending sender balance in wei.
        balance_wei: String,
        /// Transfer value plus maximum network fees in wei.
        required_wei: String,
    },
}

impl fmt::Display for NativeTransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSender => formatter.write_str("Enter a valid 0x-prefixed sender address"),
            Self::InvalidRecipient => {
                formatter.write_str("Enter a valid 0x-prefixed recipient address")
            }
            Self::InvalidAmount => formatter.write_str("Enter an ETH amount using decimal digits"),
            Self::ZeroAmount => formatter.write_str("ETH amount must be greater than zero"),
            Self::ExcessPrecision => {
                formatter.write_str("ETH amount cannot use more than 18 decimal places")
            }
            Self::AmountOverflow => formatter.write_str("ETH amount exceeds the EVM uint256 limit"),
            Self::Transport => formatter.write_str("The EVM RPC endpoint could not be reached"),
            Self::HttpStatus(status) => {
                write!(formatter, "The EVM RPC endpoint returned HTTP {status}")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("The EVM RPC response exceeded its safety limit")
            }
            Self::RpcRejected(code) => write!(
                formatter,
                "The EVM RPC endpoint rejected transfer preparation (code {code})"
            ),
            Self::ChainId(error) => error.fmt(formatter),
            Self::MalformedRpcResponse => formatter
                .write_str("The EVM RPC endpoint returned invalid transfer preparation data"),
            Self::ApprovedGasLimitExceeded => formatter.write_str("gas_limit_exceeded: current gas requirement needs a new transaction and fresh native approval"),
            Self::ApprovedFeeCapExceeded => formatter.write_str("base_fee_exceeds_signed_cap: current base fee requires a new transaction and fresh native approval"),
            Self::ArithmeticOverflow => {
                formatter.write_str("The prepared transfer fee exceeds the EVM uint256 limit")
            }
            Self::InsufficientBalance {
                balance_wei,
                required_wei,
            } => write!(
                formatter,
                "Insufficient ETH balance: {balance_wei} wei available, {required_wei} wei required"
            ),
        }
    }
}

impl Error for NativeTransferError {}

impl From<RpcError> for NativeTransferError {
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

impl From<RpcChainIdError> for NativeTransferError {
    fn from(error: RpcChainIdError) -> Self {
        Self::ChainId(error)
    }
}

impl From<crate::execution_fees::ExecutionFeeError> for NativeTransferError {
    fn from(error: crate::execution_fees::ExecutionFeeError) -> Self {
        use crate::execution_fees::ExecutionFeeError;
        match error {
            ExecutionFeeError::Rpc(error) => Self::from(error),
            ExecutionFeeError::Invalid => Self::MalformedRpcResponse,
            ExecutionFeeError::Overflow => Self::ArithmeticOverflow,
            ExecutionFeeError::ApprovedCapExceeded => Self::ApprovedFeeCapExceeded,
            ExecutionFeeError::ApprovedGasExceeded => Self::ApprovedGasLimitExceeded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_eth_amounts_accept_full_precision_and_reject_invalid_values()
    -> Result<(), NativeTransferError> {
        assert_eq!(parse_eth_amount("1")?.to_decimal(), "1000000000000000000");
        assert_eq!(parse_eth_amount("0.000000000000000001")?.to_decimal(), "1");
        assert_eq!(
            parse_eth_amount("0001.2300")?.to_decimal(),
            "1230000000000000000"
        );
        assert_eq!(
            parse_eth_amount("1.0000000000000000001"),
            Err(NativeTransferError::ExcessPrecision)
        );
        for invalid in ["", "1.", ".1", "-1", "+1", " 1", "1e2", "1.2.3"] {
            assert_eq!(
                parse_eth_amount(invalid),
                Err(NativeTransferError::InvalidAmount)
            );
        }
        Ok(())
    }

    #[test]
    fn request_rejects_zero_and_displays_checksum_recipient() -> Result<(), NativeTransferError> {
        assert_eq!(
            NativeTransferRequest::new(
                "0x9858effd232b4033e47d90003d41ec34ecaeda94",
                "0x0000000000000000000000000000000000000001",
                "0",
            ),
            Err(NativeTransferError::ZeroAmount)
        );
        let request = NativeTransferRequest::new(
            "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            "0xde709f2102306220921060314715629080e2fb77",
            "1",
        )?;
        assert_eq!(
            request.recipient.checksum(),
            "0xde709f2102306220921060314715629080e2fb77"
        );
        Ok(())
    }

    #[test]
    fn fee_arithmetic_includes_execution_and_l1_components() -> Result<(), NativeTransferError> {
        let gas = Uint256::from_u64(21_000);
        let max_fee = Uint256::from_u64(7_000_000);
        let execution = gas
            .checked_mul(max_fee)
            .ok_or(NativeTransferError::ArithmeticOverflow)?;
        let total = execution
            .checked_add(Uint256::from_u64(10_000))
            .ok_or(NativeTransferError::ArithmeticOverflow)?;
        assert_eq!(execution.to_decimal(), "147000000000");
        assert_eq!(total.to_decimal(), "147000010000");
        Ok(())
    }

    #[test]
    fn gas_estimation_object_contains_every_unsigned_type_two_field()
    -> Result<(), NativeTransferError> {
        let request = NativeTransferRequest::new(
            "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            "0xde709f2102306220921060314715629080e2fb77",
            "1",
        )?;
        let transaction = transaction_object(
            BaseNetwork::Mainnet,
            &request,
            parse_eth_amount("1")?,
            Uint256::from_u64(7),
            Uint256::from_u64(2_000_000),
            Uint256::from_u64(12_000_000),
        );
        assert_eq!(transaction["type"], "0x2");
        assert_eq!(transaction["chainId"], "0x2105");
        assert_eq!(transaction["nonce"], "0x7");
        assert_eq!(transaction["value"], "0xde0b6b3a7640000");
        assert_eq!(transaction["maxPriorityFeePerGas"], "0x1e8480");
        assert_eq!(transaction["maxFeePerGas"], "0xb71b00");
        assert_eq!(transaction["data"], "0x");
        assert_eq!(transaction["accessList"], json!([]));
        Ok(())
    }

    #[test]
    fn eip1559_fee_uses_double_base_fee_headroom() -> Result<(), NativeTransferError> {
        assert_eq!(
            crate::execution_fees::fee_cap(
                Uint256::from_u64(5_000_000),
                Uint256::from_u64(1_000_000)
            )
            .map_err(NativeTransferError::from)?
            .to_decimal(),
            "11000000"
        );
        Ok(())
    }
}
