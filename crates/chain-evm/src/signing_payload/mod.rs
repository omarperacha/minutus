//! Independent decoding of exact EVM signing payloads.

mod rlp;

use std::{error::Error, fmt, fmt::Write as _};

use serde::Serialize;
use sha3::{Digest as _, Keccak256};
use zeroize::Zeroizing;

use crate::{
    AuthorizationDecodeError, AuthorizationKind, AuthorizationStatus, DecodedAuthorization,
    DecodedTokenTransfer, EvmNetwork, NativeTransferCandidate, PublicEvmAddress,
    SimulationComparison, SimulationState, TokenTransferCandidate, WarningSeverity,
    decode_approval_calldata, decode_token_transfer, decode_typed_data,
    rpc::{Uint256, format_units},
    trusted_assets,
};

use self::rlp::{Item, ItemKind, decode_item, encode_bytes, encode_list, list_items};

const TRANSACTION_TYPE_EIP1559: u8 = 2;
const EIP1559_FIELD_COUNT: usize = 9;
const MAX_SIGNING_PAYLOAD_BYTES: usize = 64 * 1024;
const ETH_DECIMALS: u8 = 18;
const TRANSFER_SELECTOR: &[u8; 4] = &[0xa9, 0x05, 0x9c, 0xbb];
const APPROVE_SELECTOR: &[u8; 4] = &[0x09, 0x5e, 0xa7, 0xb3];

/// Exact payload kind presented to the independent signing decoder.
#[derive(Clone, Copy, Debug)]
pub enum SigningPayload<'a> {
    /// EIP-2718 type-2 unsigned transaction bytes.
    Eip1559Transaction {
        /// Trusted public account selected by the wallet boundary.
        account: &'a str,
        /// Exact bytes whose typed envelope will be signed.
        payload: &'a [u8],
    },
    /// Exact supported EIP-712 JSON payload.
    Eip712TypedData {
        /// Trusted selected network used to verify the typed-data domain.
        network: EvmNetwork,
        /// Trusted public account selected by the wallet boundary.
        account: &'a str,
        /// Exact typed-data JSON whose canonical digest will be signed.
        payload: &'a str,
        /// Trusted current Unix time for deadline warnings.
        now_unix_seconds: u64,
    },
}

/// Assembles the exact canonical EIP-1559 signed envelope from wallet-owned
/// recoverable signature components.
///
/// # Errors
///
/// Returns a decoder error unless `unsigned_payload` is one complete canonical
/// type-2 envelope containing exactly the nine unsigned fields.
pub fn assemble_signed_eip1559_transaction(
    unsigned_payload: &[u8],
    r: &[u8; 32],
    s: &[u8; 32],
    y_parity: bool,
) -> Result<Zeroizing<String>, SigningPayloadDecodeError> {
    if unsigned_payload.first() != Some(&TRANSACTION_TYPE_EIP1559)
        || unsigned_payload.len() > MAX_SIGNING_PAYLOAD_BYTES
    {
        return Err(SigningPayloadDecodeError::UnsupportedTransactionType);
    }
    let (list, consumed) = decode_item(&unsigned_payload[1..])?;
    if list.kind != ItemKind::List || consumed + 1 != unsigned_payload.len() {
        return Err(SigningPayloadDecodeError::MalformedRlp);
    }
    let items = list_items(list.payload)?;
    if items.len() != EIP1559_FIELD_COUNT {
        return Err(SigningPayloadDecodeError::UnexpectedFieldCount);
    }
    let mut body = Vec::with_capacity(list.payload.len() + 70);
    for item in items {
        match item.kind {
            ItemKind::Bytes => encode_bytes(item.payload, &mut body),
            ItemKind::List => encode_list(item.payload, &mut body),
        }
    }
    encode_bytes(if y_parity { &[1] } else { &[] }, &mut body);
    encode_bytes(trim_integer(r), &mut body);
    encode_bytes(trim_integer(s), &mut body);
    let mut signed = Vec::with_capacity(body.len() + 8);
    signed.push(TRANSACTION_TYPE_EIP1559);
    encode_list(&body, &mut signed);
    Ok(Zeroizing::new(format_hex(&signed)))
}

fn trim_integer(value: &[u8]) -> &[u8] {
    let first = value
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(value.len());
    &value[first..]
}

fn format_hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut encoded = String::with_capacity(2 + bytes.len() * 2);
    encoded.push_str("0x");
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Independently decoded exact signing payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "decoded")]
pub enum DecodedSigningPayload {
    /// An unsigned EIP-1559 transaction envelope.
    Eip1559Transaction(Box<DecodedEip1559Transaction>),
    /// A supported or explicitly unknown EIP-712 payload.
    Eip712TypedData(DecodedAuthorization),
}

/// Whether the complete transaction action is recognized.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DecodedTransactionStatus {
    /// Every security-relevant action field has a trusted interpretation.
    Verified,
    /// The action is decoded but carries one or more prominent warnings.
    Warning,
    /// The action cannot be represented as verified benign intent.
    Unknown,
}

/// Stable transaction warning category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DecodedTransactionWarningCode {
    /// The destination is not a configured trusted contract.
    UnknownContract,
    /// The calldata selector or shape is not supported.
    UnknownSelector,
    /// A token action also transfers unexpected native value.
    UnexpectedNativeValue,
}

/// One trusted transaction warning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedTransactionWarning {
    code: DecodedTransactionWarningCode,
    severity: WarningSeverity,
    message: &'static str,
}

impl DecodedTransactionWarning {
    /// Stable warning code.
    #[must_use]
    pub const fn code(&self) -> DecodedTransactionWarningCode {
        self.code
    }

    /// Required warning prominence.
    #[must_use]
    pub const fn severity(&self) -> WarningSeverity {
        self.severity
    }

    /// Trusted warning text.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

/// Security-relevant action derived only from transaction destination, value and calldata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DecodedTransactionAction {
    /// Native ETH value transfer.
    NativeTransfer {
        /// Checksummed recipient decoded from the envelope.
        recipient: String,
        /// Exact value in wei.
        value_wei: String,
        /// Exact value denominated in ETH.
        amount_eth: String,
    },
    /// Configured ERC-20 transfer decoded from calldata.
    TokenTransfer {
        /// Independently decoded token transfer facts.
        transfer: DecodedTokenTransfer,
    },
    /// Configured ERC-20 approval decoded from calldata.
    TokenApproval {
        /// Independently decoded approval facts and warnings.
        approval: DecodedAuthorization,
    },
    /// Contract call whose security-relevant action is not fully known.
    UnknownContractCall {
        /// Checksummed destination contract.
        contract: String,
        /// Four-byte selector when present.
        selector: Option<String>,
    },
}

/// Exact EIP-1559 facts derived from the unsigned signing envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedEip1559Transaction {
    network: EvmNetwork,
    network_name: &'static str,
    caip2_id: &'static str,
    chain_id: u64,
    is_testnet: bool,
    account: String,
    transaction_type: u8,
    nonce: String,
    max_priority_fee_per_gas_wei: String,
    max_fee_per_gas_wei: String,
    gas_limit: String,
    destination: String,
    value_wei: String,
    data: String,
    maximum_execution_fee_wei: String,
    maximum_execution_fee_eth: String,
    status: DecodedTransactionStatus,
    action: DecodedTransactionAction,
    warnings: Vec<DecodedTransactionWarning>,
    raw_payload: String,
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

impl DecodedEip1559Transaction {
    /// Trusted network derived from the embedded chain ID.
    #[must_use]
    pub const fn network(&self) -> EvmNetwork {
        self.network
    }
    string_getter!("Trusted network display name.", network_name);
    string_getter!("CAIP-2 network identity.", caip2_id);
    /// Embedded EIP-155 chain ID.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    /// Whether the embedded chain identifies a configured testnet.
    #[must_use]
    pub const fn is_testnet(&self) -> bool {
        self.is_testnet
    }
    string_getter!("Trusted wallet-selected public account.", account);
    /// EIP-2718 transaction type.
    #[must_use]
    pub const fn transaction_type(&self) -> u8 {
        self.transaction_type
    }
    string_getter!("Exact nonce.", nonce);
    string_getter!(
        "Exact priority fee per gas in wei.",
        max_priority_fee_per_gas_wei
    );
    string_getter!("Exact maximum fee per gas in wei.", max_fee_per_gas_wei);
    string_getter!("Exact gas limit.", gas_limit);
    string_getter!("Checksummed destination.", destination);
    string_getter!("Exact native value in wei.", value_wei);
    string_getter!("Exact calldata.", data);
    string_getter!("Maximum execution fee in wei.", maximum_execution_fee_wei);
    string_getter!("Maximum execution fee in ETH.", maximum_execution_fee_eth);
    /// Recognition status for the decoded action.
    #[must_use]
    pub const fn status(&self) -> DecodedTransactionStatus {
        self.status
    }
    /// Typed action derived from destination, value and calldata.
    #[must_use]
    pub const fn action(&self) -> &DecodedTransactionAction {
        &self.action
    }
    /// Trusted warnings in display order.
    #[must_use]
    pub fn warnings(&self) -> &[DecodedTransactionWarning] {
        &self.warnings
    }
    string_getter!(
        "Exact unsigned payload rendered as lowercase hexadecimal.",
        raw_payload
    );
}

/// Prepared transaction shape accepted for exact post-decode comparison.
#[derive(Clone, Copy, Debug)]
pub enum PreparedTransactionCandidate<'a> {
    /// Prepared native ETH transfer.
    Native(&'a NativeTransferCandidate),
    /// Prepared configured-token transfer.
    Token(&'a TokenTransferCandidate),
}

impl<'a> PreparedTransactionCandidate<'a> {
    fn account(self) -> &'a str {
        match self {
            Self::Native(candidate) => candidate.sender(),
            Self::Token(candidate) => candidate.sender(),
        }
    }
}

/// Closed failures for exact signing-payload decoding and candidate comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SigningPayloadDecodeError {
    /// The payload exceeds the fixed decoder limit.
    PayloadTooLarge,
    /// The selected wallet account is malformed.
    InvalidAccount,
    /// The EIP-2718 transaction type is unsupported.
    UnsupportedTransactionType,
    /// The RLP envelope is malformed.
    MalformedRlp,
    /// The RLP envelope uses a non-canonical representation.
    NonCanonicalRlp,
    /// The type-2 transaction does not contain exactly nine fields.
    UnexpectedFieldCount,
    /// An integer exceeds uint256 or is non-canonical.
    InvalidInteger,
    /// The embedded chain ID is not supported.
    UnsupportedChain,
    /// Contract creation is not supported by the current approval decoder.
    UnsupportedContractCreation,
    /// Non-empty access lists are not supported by the current approval decoder.
    UnsupportedAccessList,
    /// Exact fee arithmetic overflowed uint256.
    FeeOverflow,
    /// Hexadecimal candidate data is malformed.
    InvalidCandidate,
    /// Decoded payload facts differ from the prepared candidate.
    CandidateMismatch,
    /// Candidate simulation evidence conflicts with decoded intent.
    SimulationMismatch,
    /// Typed-data decoding rejected a bounded trusted input.
    Authorization(AuthorizationDecodeError),
}

impl fmt::Display for SigningPayloadDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PayloadTooLarge => "Signing payload exceeds the 64 KiB inspection limit",
            Self::InvalidAccount => "Signing account is not a valid EVM address",
            Self::UnsupportedTransactionType => "Signing payload transaction type is unsupported",
            Self::MalformedRlp => "Signing payload RLP is malformed",
            Self::NonCanonicalRlp => "Signing payload RLP is not canonical",
            Self::UnexpectedFieldCount => "EIP-1559 signing payload has an unexpected field count",
            Self::InvalidInteger => "EIP-1559 signing payload contains an invalid integer",
            Self::UnsupportedChain => "Signing payload targets an unsupported chain",
            Self::UnsupportedContractCreation => "Contract creation signing is not supported",
            Self::UnsupportedAccessList => "Non-empty EIP-1559 access lists are not supported",
            Self::FeeOverflow => "Maximum transaction execution fee exceeds uint256",
            Self::InvalidCandidate => "Prepared transaction candidate is malformed",
            Self::CandidateMismatch => {
                "Decoded signing payload differs from the prepared candidate"
            }
            Self::SimulationMismatch => {
                "Decoded signing payload conflicts with simulation evidence"
            }
            Self::Authorization(error) => return error.fmt(formatter),
        })
    }
}

impl Error for SigningPayloadDecodeError {}

impl From<AuthorizationDecodeError> for SigningPayloadDecodeError {
    fn from(error: AuthorizationDecodeError) -> Self {
        Self::Authorization(error)
    }
}

/// Independently decodes an exact transaction or typed-data signing payload.
///
/// The API accepts no request summary, model explanation, asset label or action
/// hint. All action facts come from the exact payload plus trusted wallet account
/// and network configuration.
///
/// # Errors
///
/// Returns a closed error for malformed, oversized, non-canonical or unsupported
/// payloads and invalid trusted account context.
pub fn decode_signing_payload(
    payload: SigningPayload<'_>,
) -> Result<DecodedSigningPayload, SigningPayloadDecodeError> {
    match payload {
        SigningPayload::Eip1559Transaction { account, payload } => decode_eip1559(account, payload)
            .map(Box::new)
            .map(DecodedSigningPayload::Eip1559Transaction),
        SigningPayload::Eip712TypedData {
            network,
            account,
            payload,
            now_unix_seconds,
        } => decode_typed_data(network, Some(account), payload, now_unix_seconds)
            .map(DecodedSigningPayload::Eip712TypedData)
            .map_err(Into::into),
    }
}

/// Computes the Ethereum signing digest of exact typed transaction bytes.
///
/// For an EIP-1559 transaction this is raw
/// `Keccak-256(0x02 || rlp(unsigned_fields))`. Zeta-specific domain separation
/// must never be added to this digest.
#[must_use]
pub fn eip1559_signing_digest(payload: &[u8]) -> [u8; 32] {
    Keccak256::digest(payload).into()
}

/// Encodes the exact canonical type-2 payload represented by a prepared candidate.
///
/// # Errors
///
/// Returns an error only if internally stored candidate fields are malformed.
pub fn encode_prepared_transaction(
    candidate: PreparedTransactionCandidate<'_>,
) -> Result<Vec<u8>, SigningPayloadDecodeError> {
    let fields = match candidate {
        PreparedTransactionCandidate::Native(candidate) => CandidateFields {
            chain_id: candidate.chain_id(),
            nonce: candidate.nonce(),
            priority_fee: candidate.max_priority_fee_per_gas_wei(),
            max_fee: candidate.max_fee_per_gas_wei(),
            gas_limit: candidate.gas_limit(),
            destination: candidate.recipient(),
            value: candidate.value_wei(),
            data: candidate.data(),
        },
        PreparedTransactionCandidate::Token(candidate) => CandidateFields {
            chain_id: candidate.chain_id(),
            nonce: candidate.nonce(),
            priority_fee: candidate.max_priority_fee_per_gas_wei(),
            max_fee: candidate.max_fee_per_gas_wei(),
            gas_limit: candidate.gas_limit(),
            destination: candidate.decoded().token_contract(),
            value: "0",
            data: candidate.data(),
        },
    };
    encode_candidate(fields)
}

/// Independently decodes the payload and compares every signing field with the
/// prepared candidate and its bound simulation result.
///
/// # Errors
///
/// Returns an error for payload decoding failure, any candidate-field mutation,
/// or simulation evidence that targets another chain or conflicts with intent.
pub fn decode_and_verify_prepared_transaction(
    candidate: PreparedTransactionCandidate<'_>,
    payload: &[u8],
) -> Result<DecodedEip1559Transaction, SigningPayloadDecodeError> {
    let decoded = decode_eip1559(candidate.account(), payload)?;
    verify_candidate(candidate, &decoded)?;
    Ok(decoded)
}

pub(crate) fn decode_eip1559(
    account: &str,
    payload: &[u8],
) -> Result<DecodedEip1559Transaction, SigningPayloadDecodeError> {
    if payload.len() > MAX_SIGNING_PAYLOAD_BYTES {
        return Err(SigningPayloadDecodeError::PayloadTooLarge);
    }
    let account =
        PublicEvmAddress::new(account).map_err(|_| SigningPayloadDecodeError::InvalidAccount)?;
    if payload.first() != Some(&TRANSACTION_TYPE_EIP1559) {
        return Err(SigningPayloadDecodeError::UnsupportedTransactionType);
    }
    let (envelope, consumed) = decode_item(&payload[1..])?;
    if envelope.kind != ItemKind::List || consumed != payload.len() - 1 {
        return Err(SigningPayloadDecodeError::MalformedRlp);
    }
    let fields = list_items(envelope.payload)?;
    if fields.len() != EIP1559_FIELD_COUNT {
        return Err(SigningPayloadDecodeError::UnexpectedFieldCount);
    }
    let chain = integer(fields[0])?;
    let chain_id = chain
        .try_to_u64()
        .ok_or(SigningPayloadDecodeError::UnsupportedChain)?;
    let network = EvmNetwork::ALL
        .into_iter()
        .find(|network| network.descriptor().chain_id() == chain_id)
        .ok_or(SigningPayloadDecodeError::UnsupportedChain)?;
    let nonce = integer(fields[1])?;
    let priority_fee = integer(fields[2])?;
    let max_fee = integer(fields[3])?;
    let gas_limit = integer(fields[4])?;
    let destination = bytes(fields[5])?;
    if destination.len() != 20 {
        return Err(SigningPayloadDecodeError::UnsupportedContractCreation);
    }
    let destination = PublicEvmAddress::new(&hex_prefixed(destination))
        .map_err(|_| SigningPayloadDecodeError::MalformedRlp)?;
    let value = integer(fields[6])?;
    let data = bytes(fields[7])?;
    if fields[8].kind != ItemKind::List || !fields[8].payload.is_empty() {
        return Err(SigningPayloadDecodeError::UnsupportedAccessList);
    }
    if priority_fee > max_fee {
        return Err(SigningPayloadDecodeError::InvalidInteger);
    }
    let maximum_execution_fee = gas_limit
        .checked_mul(max_fee)
        .ok_or(SigningPayloadDecodeError::FeeOverflow)?;
    let (status, action, warnings) = classify(network, &account, &destination, value, data)?;
    let descriptor = network.descriptor();
    let maximum_execution_fee_wei = maximum_execution_fee.to_decimal();
    Ok(DecodedEip1559Transaction {
        network,
        network_name: descriptor.display_name(),
        caip2_id: descriptor.caip2_id(),
        chain_id,
        is_testnet: descriptor.is_testnet(),
        account: account.checksum(),
        transaction_type: TRANSACTION_TYPE_EIP1559,
        nonce: nonce.to_decimal(),
        max_priority_fee_per_gas_wei: priority_fee.to_decimal(),
        max_fee_per_gas_wei: max_fee.to_decimal(),
        gas_limit: gas_limit.to_decimal(),
        destination: destination.checksum(),
        value_wei: value.to_decimal(),
        data: hex_prefixed(data),
        maximum_execution_fee_eth: format_units(&maximum_execution_fee_wei, ETH_DECIMALS),
        maximum_execution_fee_wei,
        status,
        action,
        warnings,
        raw_payload: hex_prefixed(payload),
    })
}

fn classify(
    network: EvmNetwork,
    account: &PublicEvmAddress,
    destination: &PublicEvmAddress,
    value: Uint256,
    data: &[u8],
) -> Result<
    (
        DecodedTransactionStatus,
        DecodedTransactionAction,
        Vec<DecodedTransactionWarning>,
    ),
    SigningPayloadDecodeError,
> {
    if data.is_empty() {
        let value_wei = value.to_decimal();
        return Ok((
            DecodedTransactionStatus::Verified,
            DecodedTransactionAction::NativeTransfer {
                recipient: destination.checksum(),
                amount_eth: format_units(&value_wei, ETH_DECIMALS),
                value_wei,
            },
            Vec::new(),
        ));
    }
    let data_hex = hex_prefixed(data);
    let trusted_contract = trusted_assets(network)[1]
        .token_contract()
        .and_then(|contract| PublicEvmAddress::new(contract).ok());
    if trusted_contract.as_ref() != Some(destination) {
        return Ok(unknown_call(
            destination,
            data,
            DecodedTransactionWarningCode::UnknownContract,
            "Destination contract is not trusted for this network",
        ));
    }
    if data.starts_with(TRANSFER_SELECTOR)
        && value.is_zero()
        && let Ok(transfer) = decode_token_transfer(network, destination.as_str(), &data_hex)
    {
        return Ok((
            DecodedTransactionStatus::Verified,
            DecodedTransactionAction::TokenTransfer { transfer },
            Vec::new(),
        ));
    }
    if data.starts_with(APPROVE_SELECTOR) && value.is_zero() {
        let approval = decode_approval_calldata(
            network,
            destination.as_str(),
            Some(account.as_str()),
            &data_hex,
        )?;
        if approval.kind() == AuthorizationKind::Approval {
            let status = match approval.status() {
                AuthorizationStatus::Verified => DecodedTransactionStatus::Verified,
                AuthorizationStatus::Warning => DecodedTransactionStatus::Warning,
                AuthorizationStatus::Unknown => DecodedTransactionStatus::Unknown,
            };
            return Ok((
                status,
                DecodedTransactionAction::TokenApproval { approval },
                Vec::new(),
            ));
        }
    }
    if !value.is_zero() {
        return Ok(unknown_call(
            destination,
            data,
            DecodedTransactionWarningCode::UnexpectedNativeValue,
            "Contract call also transfers native ETH and cannot be verified as a token-only action",
        ));
    }
    Ok(unknown_call(
        destination,
        data,
        DecodedTransactionWarningCode::UnknownSelector,
        "Contract-call selector or calldata shape is not supported",
    ))
}

fn unknown_call(
    destination: &PublicEvmAddress,
    data: &[u8],
    code: DecodedTransactionWarningCode,
    message: &'static str,
) -> (
    DecodedTransactionStatus,
    DecodedTransactionAction,
    Vec<DecodedTransactionWarning>,
) {
    let selector = data.get(..4).map(hex_prefixed);
    (
        DecodedTransactionStatus::Unknown,
        DecodedTransactionAction::UnknownContractCall {
            contract: destination.checksum(),
            selector,
        },
        vec![DecodedTransactionWarning {
            code,
            severity: WarningSeverity::Unknown,
            message,
        }],
    )
}

fn verify_candidate(
    candidate: PreparedTransactionCandidate<'_>,
    decoded: &DecodedEip1559Transaction,
) -> Result<(), SigningPayloadDecodeError> {
    let (matches, simulation_matches) = match candidate {
        PreparedTransactionCandidate::Native(candidate) => (
            decoded.chain_id == candidate.chain_id()
                && decoded.account.eq_ignore_ascii_case(candidate.sender())
                && decoded.nonce == candidate.nonce()
                && decoded.max_priority_fee_per_gas_wei == candidate.max_priority_fee_per_gas_wei()
                && decoded.max_fee_per_gas_wei == candidate.max_fee_per_gas_wei()
                && decoded.gas_limit == candidate.gas_limit()
                && decoded
                    .destination
                    .eq_ignore_ascii_case(candidate.recipient())
                && decoded.value_wei == candidate.value_wei()
                && decoded.data.eq_ignore_ascii_case(candidate.data())
                && decoded.maximum_execution_fee_wei == candidate.maximum_execution_fee_wei()
                && matches!(
                &decoded.action,
                DecodedTransactionAction::NativeTransfer {
                    recipient,
                    value_wei,
                    amount_eth,
                } if recipient.eq_ignore_ascii_case(candidate.recipient())
                    && value_wei == candidate.value_wei()
                    && amount_eth == candidate.amount_eth()
                ),
            simulation_matches(
                candidate.simulation(),
                decoded,
                crate::SimulationEffectKind::NativeTransfer,
            ),
        ),
        PreparedTransactionCandidate::Token(candidate) => {
            let expected = candidate.decoded();
            (
                decoded.chain_id == candidate.chain_id()
                    && decoded.account.eq_ignore_ascii_case(candidate.sender())
                    && decoded.nonce == candidate.nonce()
                    && decoded.max_priority_fee_per_gas_wei
                        == candidate.max_priority_fee_per_gas_wei()
                    && decoded.max_fee_per_gas_wei == candidate.max_fee_per_gas_wei()
                    && decoded.gas_limit == candidate.gas_limit()
                    && decoded
                        .destination
                        .eq_ignore_ascii_case(expected.token_contract())
                    && decoded.value_wei == "0"
                    && decoded.data.eq_ignore_ascii_case(candidate.data())
                    && decoded.maximum_execution_fee_wei == candidate.maximum_execution_fee_wei()
                    && matches!(
                    &decoded.action,
                    DecodedTransactionAction::TokenTransfer { transfer }
                        if transfer == expected
                    ),
                simulation_matches(
                    candidate.simulation(),
                    decoded,
                    crate::SimulationEffectKind::TokenTransfer,
                ),
            )
        }
    };
    if !simulation_matches {
        return Err(SigningPayloadDecodeError::SimulationMismatch);
    }
    matches
        .then_some(())
        .ok_or(SigningPayloadDecodeError::CandidateMismatch)
}

fn simulation_matches(
    simulation: &crate::TransactionSimulation,
    decoded: &DecodedEip1559Transaction,
    expected_effect: crate::SimulationEffectKind,
) -> bool {
    simulation.chain_id() == decoded.chain_id
        && simulation.state() != SimulationState::Mismatch
        && simulation.effects().iter().all(|effect| {
            effect.kind() != expected_effect
                || effect.comparison() != SimulationComparison::Mismatch
        })
}

#[derive(Clone, Copy)]
pub(crate) struct CandidateFields<'a> {
    pub(crate) chain_id: u64,
    pub(crate) nonce: &'a str,
    pub(crate) priority_fee: &'a str,
    pub(crate) max_fee: &'a str,
    pub(crate) gas_limit: &'a str,
    pub(crate) destination: &'a str,
    pub(crate) value: &'a str,
    pub(crate) data: &'a str,
}

pub(crate) fn encode_candidate(
    fields: CandidateFields<'_>,
) -> Result<Vec<u8>, SigningPayloadDecodeError> {
    let destination = PublicEvmAddress::new(fields.destination)
        .map_err(|_| SigningPayloadDecodeError::InvalidCandidate)?;
    let data = decode_hex(fields.data).ok_or(SigningPayloadDecodeError::InvalidCandidate)?;
    let values = [
        Uint256::from_u64(fields.chain_id),
        decimal(fields.nonce)?,
        decimal(fields.priority_fee)?,
        decimal(fields.max_fee)?,
        decimal(fields.gas_limit)?,
    ];
    let value = decimal(fields.value)?;
    let mut list = Vec::new();
    for value in values {
        encode_integer(value, &mut list)?;
    }
    let destination =
        decode_hex(destination.as_str()).ok_or(SigningPayloadDecodeError::InvalidCandidate)?;
    encode_bytes(&destination, &mut list);
    encode_integer(value, &mut list)?;
    encode_bytes(&data, &mut list);
    encode_list(&[], &mut list);
    let mut payload = Vec::with_capacity(list.len() + 3);
    payload.push(TRANSACTION_TYPE_EIP1559);
    encode_list(&list, &mut payload);
    Ok(payload)
}

fn decimal(value: &str) -> Result<Uint256, SigningPayloadDecodeError> {
    Uint256::from_decimal(value).map_err(|_| SigningPayloadDecodeError::InvalidCandidate)
}

fn encode_integer(value: Uint256, output: &mut Vec<u8>) -> Result<(), SigningPayloadDecodeError> {
    if value.is_zero() {
        encode_bytes(&[], output);
        return Ok(());
    }
    let word = value.to_abi_word();
    let first = word
        .find(|character| character != '0')
        .unwrap_or(word.len());
    let first = first - first % 2;
    let bytes = decode_hex(&format!("0x{}", &word[first..]))
        .ok_or(SigningPayloadDecodeError::InvalidCandidate)?;
    encode_bytes(&bytes, output);
    Ok(())
}

fn integer(item: Item<'_>) -> Result<Uint256, SigningPayloadDecodeError> {
    let bytes = bytes(item)?;
    if bytes.len() > 32 || bytes.first() == Some(&0) {
        return Err(SigningPayloadDecodeError::InvalidInteger);
    }
    if bytes.is_empty() {
        return Ok(Uint256::ZERO);
    }
    let mut significant = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(significant, "{byte:02x}");
    }
    Uint256::from_abi_word(&format!("0x{significant:0>64}"))
        .map_err(|_| SigningPayloadDecodeError::InvalidInteger)
}

fn bytes(item: Item<'_>) -> Result<&[u8], SigningPayloadDecodeError> {
    if item.kind == ItemKind::Bytes {
        Ok(item.payload)
    } else {
        Err(SigningPayloadDecodeError::MalformedRlp)
    }
}

fn hex_prefixed(bytes: &[u8]) -> String {
    bytes.iter().fold(String::from("0x"), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    })
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let value = value.strip_prefix("0x")?;
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests;
