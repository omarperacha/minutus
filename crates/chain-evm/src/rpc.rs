//! Bounded credential-redacting JSON-RPC transport and exact uint256 values.

use std::{cmp::Ordering, fmt, fmt::Write as _, io::Read, time::Duration};

use reqwest::{StatusCode, blocking::Client, redirect::Policy};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::EvmRpcUrl;

const MAX_RPC_RESPONSE_BYTES: u64 = 64 * 1024;
const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

pub(crate) struct RpcClient<'a> {
    client: Client,
    endpoint: &'a EvmRpcUrl,
}

impl<'a> RpcClient<'a> {
    pub(crate) fn new(endpoint: &'a EvmRpcUrl) -> Result<Self, RpcError> {
        let client = Client::builder()
            .connect_timeout(RPC_CONNECT_TIMEOUT)
            .timeout(RPC_REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .user_agent("Minutus/0.1 Base wallet")
            .build()
            .map_err(|_| RpcError::Transport)?;
        Ok(Self { client, endpoint })
    }

    pub(crate) fn string_result(&self, method: &str, params: &Value) -> Result<String, RpcError> {
        self.value_result(method, params)?
            .as_str()
            .map(str::to_owned)
            .ok_or(RpcError::MalformedResponse)
    }

    pub(crate) fn value_result(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        self.value_result_with_timeout(method, params, RPC_REQUEST_TIMEOUT)
    }

    pub(crate) fn value_result_with_timeout(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let response = self.response_with_timeout(method, params, timeout)?;
        if let Some(error) = response.error {
            return Err(RpcError::Rejected(error.code));
        }
        response.result.ok_or(RpcError::MalformedResponse)
    }

    pub(crate) fn call_outcome(&self, params: &Value) -> Result<RpcCallOutcome, RpcError> {
        let response = self.response("eth_call", params)?;
        if let Some(error) = response.error {
            let revert_data = error.data.as_ref().and_then(revert_data);
            let is_revert = error.code == 3
                || error
                    .message
                    .as_deref()
                    .is_some_and(|message| message.to_ascii_lowercase().contains("revert"))
                || revert_data.is_some();
            return if is_revert {
                Ok(RpcCallOutcome::Reverted {
                    code: error.code,
                    data: revert_data,
                })
            } else {
                Err(RpcError::Rejected(error.code))
            };
        }
        let result = response
            .result
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or(RpcError::MalformedResponse)?;
        Ok(RpcCallOutcome::Succeeded(result))
    }

    fn response(&self, method: &str, params: &Value) -> Result<RpcResponse, RpcError> {
        self.response_with_timeout(method, params, RPC_REQUEST_TIMEOUT)
    }

    fn response_with_timeout(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<RpcResponse, RpcError> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let mut response = self
            .client
            .post(self.endpoint.expose_sensitive())
            .timeout(timeout.min(RPC_REQUEST_TIMEOUT))
            .json(&request)
            .send()
            .map_err(|_| RpcError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(RpcError::HttpStatus(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RPC_RESPONSE_BYTES)
        {
            return Err(RpcError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(MAX_RPC_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| RpcError::Transport)?;
        if bytes.len() as u64 > MAX_RPC_RESPONSE_BYTES {
            return Err(RpcError::ResponseTooLarge);
        }
        decode_response(&bytes)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcResponse {
    jsonrpc: String,
    id: u64,
    result: Option<Value>,
    error: Option<RpcResponseError>,
}

#[derive(Deserialize)]
struct RpcResponseError {
    code: i64,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}

fn decode_response(bytes: &[u8]) -> Result<RpcResponse, RpcError> {
    let response: RpcResponse =
        serde_json::from_slice(bytes).map_err(|_| RpcError::MalformedResponse)?;
    if response.jsonrpc != "2.0" || response.id != 1 {
        return Err(RpcError::MalformedResponse);
    }
    if response.result.is_some() == response.error.is_some() {
        return Err(RpcError::MalformedResponse);
    }
    Ok(response)
}

fn revert_data(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => valid_revert_hex(value).then(|| value.to_ascii_lowercase()),
        Value::Object(object) => object.get("data").and_then(revert_data),
        _ => None,
    }
}

fn valid_revert_hex(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("0x") else {
        return false;
    };
    !hex.is_empty()
        && hex.len() <= 8 * 1024
        && hex.len().is_multiple_of(2)
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RpcCallOutcome {
    Succeeded(String),
    Reverted { code: i64, data: Option<String> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RpcError {
    Transport,
    HttpStatus(u16),
    ResponseTooLarge,
    MalformedResponse,
    Rejected(i64),
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport => formatter.write_str("The EVM RPC endpoint could not be reached"),
            Self::HttpStatus(status) => {
                write!(formatter, "The EVM RPC endpoint returned HTTP {status}")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("The EVM RPC response exceeded its safety limit")
            }
            Self::MalformedResponse => {
                formatter.write_str("The EVM RPC endpoint returned a malformed response")
            }
            Self::Rejected(code) => write!(
                formatter,
                "The EVM RPC endpoint rejected the request (code {code})"
            ),
        }
    }
}

/// Exact unsigned 256-bit integer stored least-significant limb first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Uint256([u64; 4]);

impl Uint256 {
    pub(crate) const ZERO: Self = Self([0; 4]);

    pub(crate) const fn from_u64(value: u64) -> Self {
        Self([value, 0, 0, 0])
    }

    pub(crate) fn from_quantity(value: &str) -> Result<Self, Uint256Error> {
        let hex = value.strip_prefix("0x").ok_or(Uint256Error)?;
        if hex.is_empty()
            || hex.len() > 64
            || (hex.len() > 1 && hex.starts_with('0'))
            || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Uint256Error);
        }
        Self::from_hex(hex)
    }

    pub(crate) fn from_abi_word(value: &str) -> Result<Self, Uint256Error> {
        let hex = value.strip_prefix("0x").ok_or(Uint256Error)?;
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Uint256Error);
        }
        Self::from_hex(hex)
    }

    pub(crate) fn from_decimal(value: &str) -> Result<Self, Uint256Error> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Uint256Error);
        }
        value.bytes().try_fold(Self::ZERO, |number, digit| {
            number
                .checked_mul_small(10)
                .and_then(|scaled| scaled.checked_add(Self::from_u64(u64::from(digit - b'0'))))
                .ok_or(Uint256Error)
        })
    }

    fn from_hex(hex: &str) -> Result<Self, Uint256Error> {
        hex.bytes().try_fold(Self::ZERO, |number, digit| {
            let nibble = hex_digit(digit)?;
            number
                .checked_mul_small(16)
                .and_then(|scaled| scaled.checked_add(Self::from_u64(u64::from(nibble))))
                .ok_or(Uint256Error)
        })
    }

    pub(crate) const fn is_zero(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        let mut output = [0_u64; 4];
        let mut carry = false;
        for (index, target) in output.iter_mut().enumerate() {
            let (partial, first_carry) = self.0[index].overflowing_add(other.0[index]);
            let (sum, second_carry) = partial.overflowing_add(u64::from(carry));
            *target = sum;
            carry = first_carry || second_carry;
        }
        (!carry).then_some(Self(output))
    }

    // Each truncating cast intentionally retains the low limb after the high
    // limb has been carried separately and checked for overflow.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn checked_mul(self, other: Self) -> Option<Self> {
        let mut output = [0_u64; 4];
        for left in 0..4 {
            let mut carry = 0_u128;
            for right in 0..(4 - left) {
                let index = left + right;
                let total = u128::from(self.0[left]) * u128::from(other.0[right])
                    + u128::from(output[index])
                    + carry;
                output[index] = total as u64;
                carry = total >> 64;
            }
            if carry != 0 || ((4 - left)..4).any(|right| self.0[left] != 0 && other.0[right] != 0) {
                return None;
            }
        }
        Some(Self(output))
    }

    #[allow(clippy::cast_possible_truncation)]
    fn checked_mul_small(self, multiplier: u64) -> Option<Self> {
        let mut output = [0_u64; 4];
        let mut carry = 0_u128;
        for (index, target) in output.iter_mut().enumerate() {
            let total = u128::from(self.0[index]) * u128::from(multiplier) + carry;
            *target = total as u64;
            carry = total >> 64;
        }
        (carry == 0).then_some(Self(output))
    }

    pub(crate) fn to_quantity(self) -> String {
        if self.is_zero() {
            return "0x0".to_owned();
        }
        let highest = (0..4)
            .rev()
            .find(|index| self.0[*index] != 0)
            .unwrap_or_default();
        let mut output = format!("0x{:x}", self.0[highest]);
        for index in (0..highest).rev() {
            let _ = write!(output, "{:016x}", self.0[index]);
        }
        output
    }

    pub(crate) fn to_abi_word(self) -> String {
        format!(
            "{:016x}{:016x}{:016x}{:016x}",
            self.0[3], self.0[2], self.0[1], self.0[0]
        )
    }

    pub(crate) fn to_decimal(self) -> String {
        if self.is_zero() {
            return "0".to_owned();
        }
        let mut value = self;
        let mut digits = Vec::with_capacity(78);
        while !value.is_zero() {
            let (quotient, remainder) = value.div_small(10);
            digits.push(char::from(b'0' + remainder));
            value = quotient;
        }
        digits.iter().rev().collect()
    }

    pub(crate) fn byte_len(self) -> usize {
        (0..4)
            .rev()
            .find(|index| self.0[*index] != 0)
            .map_or(0, |index| {
                index * 8 + (8 - self.0[index].leading_zeros() as usize / 8)
            })
    }

    pub(crate) const fn try_to_u64(self) -> Option<u64> {
        if self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0 {
            Some(self.0[0])
        } else {
            None
        }
    }

    pub(crate) const fn most_significant_bit(self) -> bool {
        self.0[3] & (1_u64 << 63) != 0
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn div_small(self, divisor: u8) -> (Self, u8) {
        let mut output = [0_u64; 4];
        let mut remainder = 0_u128;
        for index in (0..4).rev() {
            let value = (remainder << 64) | u128::from(self.0[index]);
            output[index] = (value / u128::from(divisor)) as u64;
            remainder = value % u128::from(divisor);
        }
        (Self(output), remainder as u8)
    }
}

impl Ord for Uint256 {
    fn cmp(&self, other: &Self) -> Ordering {
        for index in (0..4).rev() {
            match self.0[index].cmp(&other.0[index]) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Uint256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Uint256Error;

fn hex_digit(byte: u8) -> Result<u8, Uint256Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Uint256Error),
    }
}

pub(crate) fn format_units(raw: &str, decimals: u8) -> String {
    let decimals = usize::from(decimals);
    if decimals == 0 {
        return raw.to_owned();
    }
    let padded = format!("{:0>width$}", raw, width = decimals + 1);
    let split = padded.len() - decimals;
    let (whole, fraction) = padded.split_at(split);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint256_round_trips_maximum_across_hex_and_decimal() -> Result<(), Uint256Error> {
        let maximum = Uint256::from_quantity(
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )?;
        assert_eq!(
            maximum.to_quantity(),
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        assert_eq!(
            maximum.to_decimal(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
        assert_eq!(Uint256::from_decimal(&maximum.to_decimal())?, maximum);
        assert!(maximum.checked_add(Uint256::from_u64(1)).is_none());
        Ok(())
    }

    #[test]
    fn uint256_arithmetic_detects_overflow() {
        let large = Uint256([0, 0, 0, 1]);
        assert_eq!(
            Uint256::from_u64(21_000).checked_mul(Uint256::from_u64(5_000_000)),
            Some(Uint256::from_u64(105_000_000_000))
        );
        assert!(large.checked_mul(large).is_none());
    }

    #[test]
    fn malformed_rpc_envelopes_are_rejected() {
        assert_eq!(
            decode_response(br#"{"jsonrpc":"2.0","id":1,"result":7}"#)
                .and_then(|response| response.result.ok_or(RpcError::MalformedResponse)),
            Ok(Value::from(7))
        );
        assert_eq!(
            decode_response(br#"{"jsonrpc":"2.0","id":2,"result":"0x0"}"#).map(|_| Value::Null),
            Err(RpcError::MalformedResponse)
        );
        assert_eq!(
            decode_response(br#"{"jsonrpc":"2.0","id":1,"result":"0x","error":{"code":3}}"#)
                .map(|_| Value::Null),
            Err(RpcError::MalformedResponse)
        );
    }

    #[test]
    fn revert_data_is_bounded_and_extracted_from_provider_wrappers() {
        assert_eq!(
            revert_data(&json!({"data": {"data": "0x08C379A0"}})),
            Some("0x08c379a0".to_owned())
        );
        assert_eq!(revert_data(&Value::String("not hex".to_owned())), None);
        assert_eq!(
            revert_data(&Value::String(format!("0x{}", "00".repeat(4_097)))),
            None
        );
    }
}
