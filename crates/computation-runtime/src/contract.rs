use std::{collections::BTreeSet, error::Error, fmt};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum UTF-8 bytes in one program source.
pub const MAX_SOURCE_BYTES: usize = 65_536;
/// Maximum number of inputs bound to one computation.
pub const MAX_INPUTS: usize = 16;
/// Maximum bytes in one bound input.
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes across every input bound to one computation.
pub const MAX_TOTAL_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum bytes in one completed output.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub(crate) const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_INPUT_ID_BYTES: usize = 64;

/// Version of the host/runner computation contract.
pub const CONTRACT_VERSION: &str = "zeta-contract-1";
/// Exact interpreter and Rust binding embedded in the runner.
pub const RUNTIME_VERSION: &str = "quickjs-ng-0.15.1+rquickjs-0.12.2+zeta-contract-1";

/// Format assigned by the trusted host to one pre-bound input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputFormat {
    /// Complete validated UTF-8 text.
    Utf8,
    /// Complete validated JSON text.
    Json,
    /// Opaque bytes exposed as a fresh `Uint8Array`.
    Bytes,
}

/// One host-bound input carried over the private protocol.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationInput {
    id: String,
    format: InputFormat,
    data_base64: String,
    sha256: String,
}

impl ComputationInput {
    /// Binds a request-local input ID to exact bytes and their digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the ID, byte length, UTF-8 or JSON format is
    /// outside the fixed runner contract.
    pub fn new(
        id: impl Into<String>,
        format: InputFormat,
        bytes: &[u8],
    ) -> Result<Self, ContractError> {
        let id = id.into();
        if !valid_input_id(&id) || bytes.len() > MAX_INPUT_BYTES {
            return Err(ContractError::InvalidInput);
        }
        validate_input_bytes(format, bytes).map_err(|()| ContractError::InvalidInput)?;
        Ok(Self {
            id,
            format,
            data_base64: STANDARD.encode(bytes),
            sha256: hex_digest(bytes),
        })
    }

    /// Returns the request-local input ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the declared input format.
    #[must_use]
    pub const fn format(&self) -> InputFormat {
        self.format
    }

    /// Returns a verified copy of the exact pathless input bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if an in-memory contract value was corrupted.
    pub fn content_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.decode().map_err(|()| ContractError::InvalidInput)
    }

    /// Returns the digest authenticated by the runner protocol.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn decode(&self) -> Result<Vec<u8>, ()> {
        if !valid_input_id(&self.id) || !valid_digest(&self.sha256) {
            return Err(());
        }
        let bytes = STANDARD.decode(&self.data_base64).map_err(|_| ())?;
        if bytes.len() > MAX_INPUT_BYTES || hex_digest(&bytes) != self.sha256 {
            return Err(());
        }
        validate_input_bytes(self.format, &bytes)?;
        Ok(bytes)
    }
}

/// Expected representation of the single completed value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputKind {
    /// An exact JavaScript string.
    Text,
    /// A JSON-serializable value.
    Json,
}

/// Model-declared output ceiling, always bounded by product policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputExpectation {
    kind: OutputKind,
    max_bytes: usize,
    max_items: usize,
}

impl OutputExpectation {
    /// Creates a bounded output expectation.
    ///
    /// # Errors
    ///
    /// Returns an error when a ceiling is zero, exceeds product policy or a
    /// text result declares more than one item.
    pub fn new(
        kind: OutputKind,
        max_bytes: usize,
        max_items: usize,
    ) -> Result<Self, ContractError> {
        let expectation = Self {
            kind,
            max_bytes,
            max_items,
        };
        expectation
            .validate()
            .map_err(|()| ContractError::InvalidOutput)?;
        Ok(expectation)
    }

    /// Returns the expected output kind.
    #[must_use]
    pub const fn kind(&self) -> OutputKind {
        self.kind
    }

    /// Returns the caller's bounded byte ceiling.
    #[must_use]
    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Returns the caller's bounded JSON item ceiling.
    #[must_use]
    pub const fn max_items(&self) -> usize {
        self.max_items
    }

    fn validate(&self) -> Result<(), ()> {
        if self.max_bytes == 0
            || self.max_bytes > MAX_OUTPUT_BYTES
            || self.max_items == 0
            || self.max_items > 4_096
            || (self.kind == OutputKind::Text && self.max_items != 1)
        {
            return Err(());
        }
        Ok(())
    }
}

/// Complete validated request sent to one fresh runner process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationRequest {
    request_id: String,
    contract_version: String,
    runtime_version: String,
    source: String,
    inputs: Vec<ComputationInput>,
    output: OutputExpectation,
}

impl ComputationRequest {
    /// Creates a request bound to the current runtime and contract versions.
    ///
    /// # Errors
    ///
    /// Returns an error when any identifier, source, input or output bound is
    /// invalid.
    pub fn new(
        request_id: impl Into<String>,
        source: impl Into<String>,
        inputs: Vec<ComputationInput>,
        output: OutputExpectation,
    ) -> Result<Self, ContractError> {
        let request = Self {
            request_id: request_id.into(),
            contract_version: CONTRACT_VERSION.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            source: source.into(),
            inputs,
            output,
        };
        request
            .validate()
            .map_err(|()| ContractError::InvalidRequest)?;
        Ok(request)
    }

    /// Returns the host-owned request identity.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the untrusted module source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the pre-bound inputs.
    #[must_use]
    pub fn inputs(&self) -> &[ComputationInput] {
        &self.inputs
    }

    /// Returns the declared output expectation.
    #[must_use]
    pub const fn output(&self) -> &OutputExpectation {
        &self.output
    }

    pub(crate) fn validate(&self) -> Result<(), ()> {
        if !valid_request_id(&self.request_id)
            || self.contract_version != CONTRACT_VERSION
            || self.runtime_version != RUNTIME_VERSION
            || self.source.is_empty()
            || self.source.len() > MAX_SOURCE_BYTES
            || self.source.as_bytes().contains(&0)
            || self.inputs.len() > MAX_INPUTS
        {
            return Err(());
        }
        self.output.validate()?;
        let mut ids = BTreeSet::new();
        let total = self.inputs.iter().try_fold(0_usize, |total, input| {
            if !ids.insert(input.id.as_str()) {
                return Err(());
            }
            total.checked_add(input.decode()?.len()).ok_or(())
        })?;
        if total > MAX_TOTAL_INPUT_BYTES {
            return Err(());
        }
        Ok(())
    }
}

/// One validated completed value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ComputationOutput {
    /// Exact UTF-8 text.
    Text(String),
    /// Valid JSON data.
    Json(serde_json::Value),
}

/// Closed terminal status returned by the runner.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunnerStatus {
    /// The output completed and validated.
    Completed,
    /// The private request failed contract validation.
    InvalidRequest,
    /// The source could not be compiled as the fixed module shape.
    CompileFailed,
    /// Evaluation or the exported function failed.
    RuntimeFailed,
    /// Evaluation crossed its interpreter wall deadline.
    WallLimit,
    /// The process crossed its host-owned CPU-time allowance.
    CpuLimit,
    /// The interpreter or process crossed its memory allowance.
    MemoryLimit,
    /// The completed value crossed its declared ceiling.
    OutputLimit,
    /// The host cancelled the request and reaped its process.
    Cancelled,
    /// Required operating-system limit enforcement was unavailable.
    PolicyUnavailable,
}

/// Authenticated one-shot runner response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationResponse {
    request_id: String,
    runtime_version: String,
    status: RunnerStatus,
    output: Option<ComputationOutput>,
}

impl ComputationResponse {
    pub(crate) fn terminal(
        request_id: impl Into<String>,
        status: RunnerStatus,
        output: Option<ComputationOutput>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            status,
            output,
        }
    }

    /// Returns the request identity echoed by the runner.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the trusted terminal category.
    #[must_use]
    pub const fn status(&self) -> RunnerStatus {
        self.status
    }

    /// Returns the completed output when status is `completed`.
    #[must_use]
    pub const fn output(&self) -> Option<&ComputationOutput> {
        self.output.as_ref()
    }

    pub(crate) fn validate(&self, expected_request_id: &str) -> Result<(), ()> {
        if self.request_id != expected_request_id
            || self.runtime_version != RUNTIME_VERSION
            || (self.status == RunnerStatus::Completed) != self.output.is_some()
        {
            return Err(());
        }
        Ok(())
    }
}

/// Typed failure while constructing a host-side computation contract value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    /// A request-local input identity, encoding or bound is invalid.
    InvalidInput,
    /// A declared output kind or ceiling is invalid.
    InvalidOutput,
    /// A request identity, version, source or aggregate bound is invalid.
    InvalidRequest,
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput => formatter.write_str("computation input is invalid"),
            Self::InvalidOutput => formatter.write_str("computation output expectation is invalid"),
            Self::InvalidRequest => formatter.write_str("computation request is invalid"),
        }
    }
}

impl Error for ContractError {}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn valid_input_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && value.len() <= MAX_INPUT_ID_BYTES
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_input_bytes(format: InputFormat, bytes: &[u8]) -> Result<(), ()> {
    match format {
        InputFormat::Bytes => Ok(()),
        InputFormat::Utf8 => std::str::from_utf8(bytes).map(|_| ()).map_err(|_| ()),
        InputFormat::Json => {
            let text = std::str::from_utf8(bytes).map_err(|_| ())?;
            let value = serde_json::from_str::<serde_json::Value>(text).map_err(|_| ())?;
            validate_json_bounds(&value, 32, 100_000)
        }
    }
}

pub(crate) fn validate_json_bounds(
    root: &serde_json::Value,
    max_depth: usize,
    max_items: usize,
) -> Result<(), ()> {
    let mut stack = vec![(root, 1_usize)];
    let mut items = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(());
        }
        items = items.checked_add(1).ok_or(())?;
        if items > max_items {
            return Err(());
        }
        match value {
            serde_json::Value::Array(values) => {
                stack.extend(values.iter().rev().map(|value| (value, depth + 1)));
            }
            serde_json::Value::Object(values) => {
                stack.extend(values.values().rev().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
