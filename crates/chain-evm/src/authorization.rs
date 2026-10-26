//! Read-only ERC-20 approval and narrowly supported EIP-712 permit decoding.

use std::{error::Error, fmt};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    BaseNetwork, PublicEvmAddress,
    rpc::{Uint256, format_units},
    trusted_assets,
};

const APPROVE_SELECTOR: &str = "095ea7b3";
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const SOON_EXPIRY_SECONDS: u64 = 60 * 60;
const MAX_UINT256_WORD: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

/// Closed decoded authorization kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthorizationKind {
    /// ERC-20 `approve(address,uint256)` calldata.
    Approval,
    /// Exact supported EIP-2612 permit typed data.
    Permit,
    /// Unknown or malformed calldata.
    UnknownCalldata,
    /// Unknown, malformed or unsupported typed data.
    UnknownTypedData,
}

/// Whether the decoder fully recognized the payload and its risk state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthorizationStatus {
    /// Fully decoded with no warnings.
    Verified,
    /// Fully decoded but requires one or more prominent warnings.
    Warning,
    /// Cannot be represented as verified benign intent.
    Unknown,
}

/// Warning prominence used by the trusted review surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WarningSeverity {
    /// Caution that does not indicate malformed intent.
    Caution,
    /// High-risk condition requiring prominent treatment.
    Critical,
    /// The payload or identity cannot be trusted or fully decoded.
    Unknown,
}

/// Stable machine-readable authorization warning category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthorizationWarningCode {
    /// Exact uint256 maximum allowance.
    UnlimitedApproval,
    /// Allowance at or above half the uint256 range.
    EffectivelyUnlimitedApproval,
    /// Token or verifying contract is not trusted on the selected chain.
    UnknownContract,
    /// Spender matches a known-dangerous structural pattern.
    SuspiciousSpender,
    /// Spender is absent from the reviewed allowlist.
    UnrecognizedSpender,
    /// Typed-data chain differs from the selected chain.
    ChainMismatch,
    /// Typed-data owner differs from the expected public owner.
    OwnerMismatch,
    /// Domain name or version is not the trusted USDC domain.
    UnrecognizedDomain,
    /// Authorization deadline has passed.
    ExpiredAuthorization,
    /// Authorization expires within the configured warning window.
    ExpiringAuthorization,
    /// Calldata selector is not supported.
    UnknownSelector,
    /// Typed-data structure is outside the exact supported schema.
    UnsupportedTypedData,
    /// A security-relevant field cannot be decoded exactly.
    UndecodableField,
}

/// One trusted decoded display field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationField {
    label: &'static str,
    value: String,
}

impl AuthorizationField {
    /// Trusted field label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }
    /// Decoded field value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// One trusted risk warning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationWarning {
    code: AuthorizationWarningCode,
    severity: WarningSeverity,
    message: &'static str,
}

impl AuthorizationWarning {
    /// Stable warning code.
    #[must_use]
    pub const fn code(&self) -> AuthorizationWarningCode {
        self.code
    }
    /// Trusted warning prominence.
    #[must_use]
    pub const fn severity(&self) -> WarningSeverity {
        self.severity
    }
    /// Trusted user-facing warning text.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

/// Immutable read-only decoded authorization result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedAuthorization {
    kind: AuthorizationKind,
    status: AuthorizationStatus,
    fields: Vec<AuthorizationField>,
    warnings: Vec<AuthorizationWarning>,
    raw_payload: String,
}

impl DecodedAuthorization {
    /// Decoded payload kind.
    #[must_use]
    pub const fn kind(&self) -> AuthorizationKind {
        self.kind
    }
    /// Overall verification state.
    #[must_use]
    pub const fn status(&self) -> AuthorizationStatus {
        self.status
    }
    /// Ordered security-relevant display fields.
    #[must_use]
    pub fn fields(&self) -> &[AuthorizationField] {
        &self.fields
    }
    /// Trusted warnings in display order.
    #[must_use]
    pub fn warnings(&self) -> &[AuthorizationWarning] {
        &self.warnings
    }
    /// Exact untrusted input retained for inspection.
    #[must_use]
    pub fn raw_payload(&self) -> &str {
        &self.raw_payload
    }
}

/// Decodes ERC-20 approval calldata without signing, approval or broadcast authority.
///
/// Unknown contracts and selectors return an inspectable UNKNOWN result. Malformed
/// addresses or oversized payloads return a closed validation error.
///
/// # Errors
///
/// Returns an error when the optional owner or contract is not a valid address,
/// or the raw payload exceeds its fixed bound.
pub fn decode_approval_calldata(
    network: BaseNetwork,
    token_contract: &str,
    owner: Option<&str>,
    calldata: &str,
) -> Result<DecodedAuthorization, AuthorizationDecodeError> {
    ensure_bounded(calldata)?;
    let contract = PublicEvmAddress::new(token_contract)
        .map_err(|_| AuthorizationDecodeError::InvalidContract)?;
    let owner = owner
        .map(PublicEvmAddress::new)
        .transpose()
        .map_err(|_| AuthorizationDecodeError::InvalidOwner)?;
    let trusted = trusted_token(network);
    let mut warnings = Vec::new();
    if trusted.as_ref() != Some(&contract) {
        warnings.push(warning(
            AuthorizationWarningCode::UnknownContract,
            WarningSeverity::Unknown,
            "Unknown token contract: symbol and decimals are not trusted",
        ));
    }
    let Some(hex) = calldata.strip_prefix("0x") else {
        return Ok(unknown_calldata(calldata, warnings, "Calldata"));
    };
    if hex.len() < 8 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(unknown_calldata(calldata, warnings, "Calldata"));
    }
    if !hex[..8].eq_ignore_ascii_case(APPROVE_SELECTOR) {
        warnings.push(warning(
            AuthorizationWarningCode::UnknownSelector,
            WarningSeverity::Unknown,
            "Unknown contract-call selector; approval intent is not verified",
        ));
        return Ok(result(
            AuthorizationKind::UnknownCalldata,
            vec![
                field("Token contract", contract.checksum()),
                field("Selector", format!("0x{}", &hex[..8])),
            ],
            warnings,
            calldata,
        ));
    }
    if hex.len() != 136 || hex[8..32].bytes().any(|byte| byte != b'0') {
        warnings.push(undecodable());
        return Ok(result(
            AuthorizationKind::UnknownCalldata,
            vec![field("Token contract", contract.checksum())],
            warnings,
            calldata,
        ));
    }
    let Ok(spender) = PublicEvmAddress::new(&format!("0x{}", &hex[32..72])) else {
        warnings.push(undecodable());
        return Ok(result(
            AuthorizationKind::UnknownCalldata,
            vec![field("Token contract", contract.checksum())],
            warnings,
            calldata,
        ));
    };
    let Ok(allowance) = Uint256::from_abi_word(&format!("0x{}", &hex[72..136])) else {
        warnings.push(undecodable());
        return Ok(result(
            AuthorizationKind::UnknownCalldata,
            vec![field("Token contract", contract.checksum())],
            warnings,
            calldata,
        ));
    };
    allowance_warnings(allowance, &mut warnings);
    spender_warnings(&spender, &contract, owner.as_ref(), &mut warnings);
    let raw = allowance.to_decimal();
    let mut fields = vec![field("Token contract", contract.checksum())];
    if trusted.is_some() {
        fields.push(field("Token", "USDC · 6 decimals"));
    } else {
        fields.push(field("Token", "UNKNOWN"));
    }
    if let Some(owner) = owner {
        fields.push(field("Owner", owner.checksum()));
    }
    fields.push(field("Spender", spender.checksum()));
    fields.push(field(
        "Allowance",
        trusted.map_or_else(
            || "UNKNOWN (untrusted decimals)".to_owned(),
            |_| format!("{} USDC", format_units(&raw, 6)),
        ),
    ));
    fields.push(field("Raw allowance", raw));
    Ok(result(
        AuthorizationKind::Approval,
        fields,
        warnings,
        calldata,
    ))
}

/// Decodes the exact supported EIP-2612 Permit schema.
///
/// Unsupported or malformed typed data remains available as an UNKNOWN result.
/// `now_unix_seconds` is supplied by the trusted host solely for expiry policy.
///
/// # Errors
///
/// Returns an error when the optional expected owner is malformed or the raw
/// payload exceeds its fixed bound.
pub fn decode_typed_data(
    network: BaseNetwork,
    expected_owner: Option<&str>,
    raw_typed_data: &str,
    now_unix_seconds: u64,
) -> Result<DecodedAuthorization, AuthorizationDecodeError> {
    ensure_bounded(raw_typed_data)?;
    let expected_owner = expected_owner
        .map(PublicEvmAddress::new)
        .transpose()
        .map_err(|_| AuthorizationDecodeError::InvalidOwner)?;
    let permit = match parse_permit_payload(raw_typed_data) {
        Ok(permit) => permit,
        Err((code, message)) => return Ok(unknown_typed(raw_typed_data, code, message)),
    };
    let mut warnings = Vec::new();
    let trusted = trusted_token(network);
    if trusted.as_ref() != Some(&permit.verifying) {
        warnings.push(warning(
            AuthorizationWarningCode::UnknownContract,
            WarningSeverity::Unknown,
            "Unknown verifying contract: this permit is not trusted USDC",
        ));
    }
    if permit.chain_id != Uint256::from_u64(network.descriptor().chain_id()) {
        warnings.push(warning(
            AuthorizationWarningCode::ChainMismatch,
            WarningSeverity::Critical,
            "Typed-data chain does not match the selected EVM network",
        ));
    }
    if expected_owner
        .as_ref()
        .is_some_and(|expected| expected != &permit.owner)
    {
        warnings.push(warning(
            AuthorizationWarningCode::OwnerMismatch,
            WarningSeverity::Critical,
            "Permit owner does not match the expected wallet account",
        ));
    }
    if permit.name != "USD Coin" || permit.version != "2" {
        warnings.push(warning(
            AuthorizationWarningCode::UnrecognizedDomain,
            WarningSeverity::Critical,
            "Permit domain name or version is not the trusted USDC domain",
        ));
    }
    allowance_warnings(permit.value, &mut warnings);
    spender_warnings(
        &permit.spender,
        &permit.verifying,
        Some(&permit.owner),
        &mut warnings,
    );
    expiry_warnings(permit.deadline, now_unix_seconds, &mut warnings);
    let raw_value = permit.value.to_decimal();
    let mut fields = vec![
        field("Domain name", permit.name),
        field("Domain version", permit.version),
        field("Domain chain ID", permit.chain_id.to_decimal()),
        field("Verifying contract", permit.verifying.checksum()),
        field("Primary type", "Permit"),
    ];
    fields.push(field(
        "Token",
        if trusted.is_some() {
            "USDC · 6 decimals"
        } else {
            "UNKNOWN"
        },
    ));
    fields.push(field("Owner", permit.owner.checksum()));
    fields.push(field("Spender", permit.spender.checksum()));
    fields.push(field(
        "Allowance",
        trusted.map_or_else(
            || "UNKNOWN (untrusted decimals)".to_owned(),
            |_| format!("{} USDC", format_units(&raw_value, 6)),
        ),
    ));
    fields.push(field("Raw allowance", raw_value));
    fields.push(field("Nonce", permit.nonce.to_decimal()));
    fields.push(field("Deadline", permit.deadline.to_decimal()));
    Ok(result(
        AuthorizationKind::Permit,
        fields,
        warnings,
        raw_typed_data,
    ))
}

struct PermitValues {
    name: String,
    version: String,
    chain_id: Uint256,
    verifying: PublicEvmAddress,
    owner: PublicEvmAddress,
    spender: PublicEvmAddress,
    value: Uint256,
    nonce: Uint256,
    deadline: Uint256,
}

fn parse_permit_payload(
    raw: &str,
) -> Result<PermitValues, (AuthorizationWarningCode, &'static str)> {
    let root = serde_json::from_str::<Value>(raw).map_err(|_| {
        (
            AuthorizationWarningCode::UndecodableField,
            "Typed data is not valid JSON",
        )
    })?;
    let root = root.as_object().ok_or((
        AuthorizationWarningCode::UnsupportedTypedData,
        "Typed data root is not an object",
    ))?;
    if !is_exact_permit_schema(root) {
        return Err((
            AuthorizationWarningCode::UnsupportedTypedData,
            "Unsupported EIP-712 structure or primary type",
        ));
    }
    permit_values(root).map_err(|()| {
        (
            AuthorizationWarningCode::UndecodableField,
            "One or more Permit fields cannot be decoded exactly",
        )
    })
}

fn permit_values(root: &Map<String, Value>) -> Result<PermitValues, ()> {
    let domain = root.get("domain").and_then(Value::as_object).ok_or(())?;
    let message = root.get("message").and_then(Value::as_object).ok_or(())?;
    Ok(PermitValues {
        name: bounded_string(domain.get("name")).ok_or(())?,
        version: bounded_string(domain.get("version")).ok_or(())?,
        chain_id: parse_uint(domain.get("chainId")).ok_or(())?,
        verifying: parse_address(domain.get("verifyingContract")).ok_or(())?,
        owner: parse_address(message.get("owner")).ok_or(())?,
        spender: parse_address(message.get("spender")).ok_or(())?,
        value: parse_uint(message.get("value")).ok_or(())?,
        nonce: parse_uint(message.get("nonce")).ok_or(())?,
        deadline: parse_uint(message.get("deadline")).ok_or(())?,
    })
}

fn is_exact_permit_schema(root: &Map<String, Value>) -> bool {
    if !has_exact_keys(root, &["types", "primaryType", "domain", "message"])
        || root.get("primaryType").and_then(Value::as_str) != Some("Permit")
    {
        return false;
    }
    let Some(types) = root.get("types").and_then(Value::as_object) else {
        return false;
    };
    let Some(domain) = root.get("domain").and_then(Value::as_object) else {
        return false;
    };
    let Some(message) = root.get("message").and_then(Value::as_object) else {
        return false;
    };
    has_exact_keys(types, &["EIP712Domain", "Permit"])
        && exact_type(
            types.get("EIP712Domain"),
            &[
                ("name", "string"),
                ("version", "string"),
                ("chainId", "uint256"),
                ("verifyingContract", "address"),
            ],
        )
        && exact_type(
            types.get("Permit"),
            &[
                ("owner", "address"),
                ("spender", "address"),
                ("value", "uint256"),
                ("nonce", "uint256"),
                ("deadline", "uint256"),
            ],
        )
        && has_exact_keys(domain, &["name", "version", "chainId", "verifyingContract"])
        && has_exact_keys(message, &["owner", "spender", "value", "nonce", "deadline"])
}

fn exact_type(value: Option<&Value>, expected: &[(&str, &str)]) -> bool {
    let Some(fields) = value.and_then(Value::as_array) else {
        return false;
    };
    fields.len() == expected.len()
        && fields.iter().zip(expected).all(|(field, (name, kind))| {
            field.as_object().is_some_and(|object| {
                has_exact_keys(object, &["name", "type"])
                    && object.get("name").and_then(Value::as_str) == Some(*name)
                    && object.get("type").and_then(Value::as_str) == Some(*kind)
            })
        })
}

fn has_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn parse_address(value: Option<&Value>) -> Option<PublicEvmAddress> {
    value
        .and_then(Value::as_str)
        .and_then(|value| PublicEvmAddress::new(value).ok())
}

fn parse_uint(value: Option<&Value>) -> Option<Uint256> {
    match value? {
        Value::String(value) if value.starts_with("0x") => Uint256::from_quantity(value).ok(),
        Value::String(value) => Uint256::from_decimal(value).ok(),
        Value::Number(value) => value.as_u64().map(Uint256::from_u64),
        _ => None,
    }
}

fn bounded_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
}

fn trusted_token(network: BaseNetwork) -> Option<PublicEvmAddress> {
    trusted_assets(network)[1]
        .token_contract()
        .and_then(|value| PublicEvmAddress::new(value).ok())
}

fn allowance_warnings(value: Uint256, warnings: &mut Vec<AuthorizationWarning>) {
    if value.to_abi_word() == MAX_UINT256_WORD {
        warnings.push(warning(
            AuthorizationWarningCode::UnlimitedApproval,
            WarningSeverity::Critical,
            "Unlimited token approval grants the spender the maximum uint256 allowance",
        ));
    } else if value.most_significant_bit() {
        warnings.push(warning(
            AuthorizationWarningCode::EffectivelyUnlimitedApproval,
            WarningSeverity::Critical,
            "Effectively unlimited token approval grants an exceptionally large allowance",
        ));
    }
}

fn spender_warnings(
    spender: &PublicEvmAddress,
    token: &PublicEvmAddress,
    owner: Option<&PublicEvmAddress>,
    warnings: &mut Vec<AuthorizationWarning>,
) {
    if spender.as_str() == "0x0000000000000000000000000000000000000000"
        || spender == token
        || owner == Some(spender)
    {
        warnings.push(warning(
            AuthorizationWarningCode::SuspiciousSpender,
            WarningSeverity::Critical,
            "Spender matches a suspicious structural pattern (zero, token, or owner address)",
        ));
    } else {
        warnings.push(warning(
            AuthorizationWarningCode::UnrecognizedSpender,
            WarningSeverity::Caution,
            "Spender is not present in Minutus's reviewed spender allowlist",
        ));
    }
}

fn expiry_warnings(deadline: Uint256, now: u64, warnings: &mut Vec<AuthorizationWarning>) {
    let Some(deadline) = deadline.try_to_u64() else {
        return;
    };
    if deadline <= now {
        warnings.push(warning(
            AuthorizationWarningCode::ExpiredAuthorization,
            WarningSeverity::Critical,
            "Authorization deadline has expired",
        ));
    } else if deadline <= now.saturating_add(SOON_EXPIRY_SECONDS) {
        warnings.push(warning(
            AuthorizationWarningCode::ExpiringAuthorization,
            WarningSeverity::Caution,
            "Authorization expires within one hour",
        ));
    }
}

fn result(
    kind: AuthorizationKind,
    fields: Vec<AuthorizationField>,
    warnings: Vec<AuthorizationWarning>,
    raw: &str,
) -> DecodedAuthorization {
    let status = if matches!(
        kind,
        AuthorizationKind::UnknownCalldata | AuthorizationKind::UnknownTypedData
    ) || warnings
        .iter()
        .any(|warning| warning.severity == WarningSeverity::Unknown)
    {
        AuthorizationStatus::Unknown
    } else if warnings.is_empty() {
        AuthorizationStatus::Verified
    } else {
        AuthorizationStatus::Warning
    };
    DecodedAuthorization {
        kind,
        status,
        fields,
        warnings,
        raw_payload: raw.to_owned(),
    }
}

fn unknown_calldata(
    raw: &str,
    mut warnings: Vec<AuthorizationWarning>,
    label: &'static str,
) -> DecodedAuthorization {
    warnings.push(undecodable());
    result(
        AuthorizationKind::UnknownCalldata,
        vec![field(label, "UNKNOWN")],
        warnings,
        raw,
    )
}

fn unknown_typed(
    raw: &str,
    code: AuthorizationWarningCode,
    message: &'static str,
) -> DecodedAuthorization {
    result(
        AuthorizationKind::UnknownTypedData,
        Vec::new(),
        vec![warning(code, WarningSeverity::Unknown, message)],
        raw,
    )
}

fn field(label: &'static str, value: impl Into<String>) -> AuthorizationField {
    AuthorizationField {
        label,
        value: value.into(),
    }
}
fn warning(
    code: AuthorizationWarningCode,
    severity: WarningSeverity,
    message: &'static str,
) -> AuthorizationWarning {
    AuthorizationWarning {
        code,
        severity,
        message,
    }
}
fn undecodable() -> AuthorizationWarning {
    warning(
        AuthorizationWarningCode::UndecodableField,
        WarningSeverity::Unknown,
        "Security-relevant calldata fields cannot be decoded exactly",
    )
}

fn ensure_bounded(payload: &str) -> Result<(), AuthorizationDecodeError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        Err(AuthorizationDecodeError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

/// Closed decoder input failures. Unknown payload content is returned as data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDecodeError {
    /// Contract address input is malformed.
    InvalidContract,
    /// Optional expected owner address is malformed.
    InvalidOwner,
    /// Raw payload exceeds the fixed inspection bound.
    PayloadTooLarge,
}

impl fmt::Display for AuthorizationDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidContract => "Enter a valid 0x-prefixed token contract",
            Self::InvalidOwner => "Enter a valid 0x-prefixed owner address",
            Self::PayloadTooLarge => "Authorization payload exceeds the 64 KiB inspection limit",
        })
    }
}
impl Error for AuthorizationDecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    const OWNER: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";
    const SPENDER: &str = "0xde709f2102306220921060314715629080e2fb77";

    fn approval(spender: &str, amount_word: &str) -> String {
        format!(
            "0x{APPROVE_SELECTOR}{:0>64}{}",
            spender.trim_start_matches("0x"),
            amount_word.trim_start_matches("0x")
        )
    }

    fn permit(chain_id: u64, verifying: &str, deadline: u64) -> String {
        serde_json::json!({
            "types": {
                "EIP712Domain": [{"name":"name","type":"string"},{"name":"version","type":"string"},{"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],
                "Permit": [{"name":"owner","type":"address"},{"name":"spender","type":"address"},{"name":"value","type":"uint256"},{"name":"nonce","type":"uint256"},{"name":"deadline","type":"uint256"}]
            },
            "primaryType": "Permit",
            "domain": {"name":"USD Coin","version":"2","chainId":chain_id,"verifyingContract":verifying},
            "message": {"owner":OWNER,"spender":SPENDER,"value":"1250000","nonce":"7","deadline":deadline}
        }).to_string()
    }

    #[test]
    fn approval_decodes_trusted_fields_and_warns_for_unrecognized_spender()
    -> Result<(), AuthorizationDecodeError> {
        let decoded = decode_approval_calldata(
            BaseNetwork::Mainnet,
            TOKEN,
            Some(OWNER),
            &approval(SPENDER, &Uint256::from_u64(1_250_000).to_abi_word()),
        )?;
        assert_eq!(decoded.kind(), AuthorizationKind::Approval);
        assert_eq!(decoded.status(), AuthorizationStatus::Warning);
        assert!(
            decoded
                .fields()
                .iter()
                .any(|field| field.label() == "Allowance" && field.value() == "1.25 USDC")
        );
        assert_eq!(
            decoded.warnings()[0].code(),
            AuthorizationWarningCode::UnrecognizedSpender
        );
        Ok(())
    }

    #[test]
    fn unlimited_and_unknown_selector_are_prominent() -> Result<(), AuthorizationDecodeError> {
        let unlimited = decode_approval_calldata(
            BaseNetwork::Mainnet,
            TOKEN,
            Some(OWNER),
            &approval(SPENDER, MAX_UINT256_WORD),
        )?;
        assert!(unlimited.warnings().iter().any(|warning| warning.code()
            == AuthorizationWarningCode::UnlimitedApproval
            && warning.severity() == WarningSeverity::Critical));
        let unknown =
            decode_approval_calldata(BaseNetwork::Mainnet, TOKEN, Some(OWNER), "0xdeadbeef")?;
        assert_eq!(unknown.status(), AuthorizationStatus::Unknown);
        assert!(
            unknown
                .warnings()
                .iter()
                .any(|warning| warning.code() == AuthorizationWarningCode::UnknownSelector)
        );
        Ok(())
    }

    #[test]
    fn effectively_unlimited_and_structurally_suspicious_are_distinct()
    -> Result<(), AuthorizationDecodeError> {
        let effective = format!("8{}", "0".repeat(63));
        let decoded = decode_approval_calldata(
            BaseNetwork::Mainnet,
            TOKEN,
            Some(OWNER),
            &approval("0x0000000000000000000000000000000000000000", &effective),
        )?;
        assert!(decoded.warnings().iter().any(|warning| {
            warning.code() == AuthorizationWarningCode::EffectivelyUnlimitedApproval
        }));
        assert!(
            decoded
                .warnings()
                .iter()
                .any(|warning| { warning.code() == AuthorizationWarningCode::SuspiciousSpender })
        );
        Ok(())
    }

    #[test]
    fn permit_decodes_security_fields_and_expiry() -> Result<(), AuthorizationDecodeError> {
        let decoded = decode_typed_data(
            BaseNetwork::Mainnet,
            Some(OWNER),
            &permit(8453, TOKEN, 1_500),
            1_000,
        )?;
        assert_eq!(decoded.kind(), AuthorizationKind::Permit);
        assert!(
            decoded
                .fields()
                .iter()
                .any(|field| field.label() == "Nonce" && field.value() == "7")
        );
        assert!(
            decoded
                .warnings()
                .iter()
                .any(|warning| warning.code() == AuthorizationWarningCode::ExpiringAuthorization)
        );
        Ok(())
    }

    #[test]
    fn permit_flags_chain_unknown_contract_and_unsupported_shape()
    -> Result<(), AuthorizationDecodeError> {
        let decoded = decode_typed_data(
            BaseNetwork::Mainnet,
            Some(OWNER),
            &permit(84532, "0x0000000000000000000000000000000000000001", 2_000),
            1_000,
        )?;
        assert_eq!(decoded.status(), AuthorizationStatus::Unknown);
        assert!(
            decoded
                .warnings()
                .iter()
                .any(|warning| warning.code() == AuthorizationWarningCode::ChainMismatch)
        );
        assert!(
            decoded
                .warnings()
                .iter()
                .any(|warning| warning.code() == AuthorizationWarningCode::UnknownContract)
        );
        let unsupported =
            decode_typed_data(BaseNetwork::Mainnet, None, r#"{"primaryType":"Order"}"#, 0)?;
        assert_eq!(unsupported.kind(), AuthorizationKind::UnknownTypedData);
        assert_eq!(unsupported.status(), AuthorizationStatus::Unknown);
        Ok(())
    }

    #[test]
    fn expired_permit_and_undecodable_typed_field_remain_prominent()
    -> Result<(), AuthorizationDecodeError> {
        let expired = decode_typed_data(
            BaseNetwork::Mainnet,
            Some(OWNER),
            &permit(8453, TOKEN, 999),
            1_000,
        )?;
        assert!(expired.warnings().iter().any(|warning| {
            warning.code() == AuthorizationWarningCode::ExpiredAuthorization
                && warning.severity() == WarningSeverity::Critical
        }));
        let mut malformed = serde_json::from_str::<Value>(&permit(8453, TOKEN, 2_000))
            .map_err(|_| AuthorizationDecodeError::PayloadTooLarge)?;
        malformed["message"]["nonce"] = Value::String("not-a-number".to_owned());
        let unknown = decode_typed_data(
            BaseNetwork::Mainnet,
            Some(OWNER),
            &malformed.to_string(),
            1_000,
        )?;
        assert_eq!(unknown.status(), AuthorizationStatus::Unknown);
        assert_eq!(
            unknown.warnings()[0].code(),
            AuthorizationWarningCode::UndecodableField
        );
        Ok(())
    }
}
