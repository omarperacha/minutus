use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{HostSourceBinding, InjectionSignal, ledger_types::validate_digest};

use super::CompactResultError;

const PROVENANCE_VERSION: u16 = 1;
const MAX_UNTRUSTED_ARGUMENT_BYTES: usize = 256 * 1024;
const MAX_SOURCE_BYTES: usize = 65_536;
const MAX_INPUTS: usize = 16;
const MAX_FINDINGS: usize = 36;
const MAX_INPUT_ID_BYTES: usize = 64;

/// Closed computation outcome retained independently of model-facing wording.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComputationTerminalStatus {
    /// Evaluation completed with one validated value.
    Completed,
    /// The fixed contract rejected the request.
    InvalidRequest,
    /// The fixed module could not be compiled.
    CompileFailed,
    /// Evaluation failed without exposing diagnostics.
    RuntimeFailed,
    /// The independent wall deadline elapsed.
    WallLimit,
    /// Host-observed CPU use crossed the ceiling.
    CpuLimit,
    /// Interpreter or process memory crossed the ceiling.
    MemoryLimit,
    /// The declared output ceiling was crossed.
    OutputLimit,
    /// Trusted host cancellation stopped the process.
    Cancelled,
    /// Required isolation policy could not be applied.
    PolicyUnavailable,
    /// The isolated process or authenticated protocol failed.
    RunnerFailed,
}

/// Exact product and request ceilings applied to one computation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationAppliedLimits {
    /// Maximum accumulated process CPU time.
    pub cpu_time_ms: u64,
    /// Maximum end-to-end elapsed time.
    pub wall_time_ms: u64,
    /// Maximum host-observed process memory.
    pub process_memory_bytes: u64,
    /// Maximum interpreter-managed heap.
    pub javascript_heap_bytes: u64,
    /// Maximum interpreter stack.
    pub javascript_stack_bytes: u64,
    /// Maximum model-authored source size.
    pub source_bytes: u64,
    /// Maximum bytes across bound inputs.
    pub total_input_bytes: u64,
    /// Request-specific output byte ceiling.
    pub output_bytes: u64,
    /// Request-specific structured item ceiling.
    pub output_items: u64,
}

/// Host-only source binding and shape for one exact runner input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationInputProvenance {
    id: String,
    format: String,
    byte_size: u64,
    source: HostSourceBinding,
}

impl ComputationInputProvenance {
    /// Creates one bounded input provenance entry.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed identity, format, size or source binding.
    pub fn new(
        id: impl Into<String>,
        format: impl Into<String>,
        byte_size: u64,
        source: HostSourceBinding,
    ) -> Result<Self, CompactResultError> {
        let value = Self {
            id: id.into(),
            format: format.into(),
            byte_size,
            source,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), CompactResultError> {
        if !valid_input_id(&self.id)
            || !matches!(self.format.as_str(), "utf8" | "json" | "bytes")
            || self.source.validate().is_err()
        {
            return Err(CompactResultError::InvalidRecord);
        }
        Ok(())
    }

    /// Returns the request-local input identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the complete host-only source binding.
    #[must_use]
    pub const fn source(&self) -> &HostSourceBinding {
        &self.source
    }
}

/// Untrusted computation boundary inspected by the trusted prompt-safety detector.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ComputationPromptSafetyTarget {
    /// Model-authored JavaScript source.
    Source,
    /// Complete schema-validated tool arguments.
    Arguments,
    /// Exact contents of one bound textual input.
    Input {
        /// Request-local input identity, never a host path.
        id: String,
    },
    /// Validated completed output.
    Output,
}

/// Authority-free prompt-injection categories retained with provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationPromptSafetyFinding {
    target: ComputationPromptSafetyTarget,
    signals: Vec<InjectionSignal>,
}

impl ComputationPromptSafetyFinding {
    /// Creates one deterministic, non-empty safety finding.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed input identity or signal categories.
    pub fn new(
        target: ComputationPromptSafetyTarget,
        mut signals: Vec<InjectionSignal>,
    ) -> Result<Self, CompactResultError> {
        signals.sort_unstable();
        signals.dedup();
        let finding = Self { target, signals };
        finding.validate()?;
        Ok(finding)
    }

    fn validate(&self) -> Result<(), CompactResultError> {
        if self.signals.is_empty()
            || self.signals.len() > 4
            || !self.signals.windows(2).all(|pair| pair[0] < pair[1])
            || matches!(&self.target, ComputationPromptSafetyTarget::Input { id } if !valid_input_id(id))
        {
            return Err(CompactResultError::InvalidRecord);
        }
        Ok(())
    }
}

/// Persisted host-only record of exact untrusted inputs and sanitized output identity.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputationProvenanceRecord {
    version: u16,
    contract_version: String,
    runtime_version: String,
    untrusted_source: String,
    untrusted_arguments: Value,
    script_sha256: String,
    inputs: Vec<ComputationInputProvenance>,
    limits: ComputationAppliedLimits,
    terminal_status: ComputationTerminalStatus,
    sanitized_output_sha256: Option<String>,
    prompt_safety_findings: Vec<ComputationPromptSafetyFinding>,
}

impl ComputationProvenanceRecord {
    /// Creates one bounded record. Source and arguments remain host-only.
    ///
    /// # Errors
    ///
    /// Returns an error when any untrusted input, digest, binding, limit,
    /// terminal association or safety finding is malformed or oversized.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        contract_version: impl Into<String>,
        runtime_version: impl Into<String>,
        untrusted_source: impl Into<String>,
        untrusted_arguments: Value,
        inputs: Vec<ComputationInputProvenance>,
        limits: ComputationAppliedLimits,
        terminal_status: ComputationTerminalStatus,
        sanitized_output_sha256: Option<String>,
        prompt_safety_findings: Vec<ComputationPromptSafetyFinding>,
    ) -> Result<Self, CompactResultError> {
        let untrusted_source = untrusted_source.into();
        let record = Self {
            version: PROVENANCE_VERSION,
            contract_version: contract_version.into(),
            runtime_version: runtime_version.into(),
            script_sha256: format!("{:x}", Sha256::digest(untrusted_source.as_bytes())),
            untrusted_source,
            untrusted_arguments,
            inputs,
            limits,
            terminal_status,
            sanitized_output_sha256,
            prompt_safety_findings,
        };
        record.validate()?;
        Ok(record)
    }

    pub(crate) fn validate(&self) -> Result<(), CompactResultError> {
        if self.version != PROVENANCE_VERSION
            || self.contract_version.is_empty()
            || self.contract_version.len() > 160
            || self.runtime_version.is_empty()
            || self.runtime_version.len() > 256
            || self.untrusted_source.is_empty()
            || self.untrusted_source.len() > MAX_SOURCE_BYTES
            || self.untrusted_source.as_bytes().contains(&0)
            || serde_json::to_vec(&self.untrusted_arguments)?.len() > MAX_UNTRUSTED_ARGUMENT_BYTES
            || validate_digest(&self.script_sha256).is_err()
            || self.script_sha256
                != format!("{:x}", Sha256::digest(self.untrusted_source.as_bytes()))
            || self.inputs.len() > MAX_INPUTS
            || self.prompt_safety_findings.len() > MAX_FINDINGS
            || self.inputs.iter().any(|input| input.validate().is_err())
            || self
                .prompt_safety_findings
                .iter()
                .any(|finding| finding.validate().is_err())
        {
            return Err(CompactResultError::InvalidRecord);
        }
        let limits = &self.limits;
        if [
            limits.cpu_time_ms,
            limits.wall_time_ms,
            limits.process_memory_bytes,
            limits.javascript_heap_bytes,
            limits.javascript_stack_bytes,
            limits.source_bytes,
            limits.total_input_bytes,
            limits.output_bytes,
            limits.output_items,
        ]
        .contains(&0)
            || u64::try_from(self.untrusted_source.len()).unwrap_or(u64::MAX) > limits.source_bytes
            || self
                .inputs
                .iter()
                .try_fold(0_u64, |total, input| total.checked_add(input.byte_size))
                .is_none_or(|total| total > limits.total_input_bytes)
        {
            return Err(CompactResultError::InvalidRecord);
        }
        let unique_ids = self
            .inputs
            .iter()
            .map(|input| &input.id)
            .collect::<BTreeSet<_>>();
        if unique_ids.len() != self.inputs.len()
            || self
                .sanitized_output_sha256
                .as_deref()
                .is_some_and(|digest| validate_digest(digest).is_err())
            || (self.terminal_status == ComputationTerminalStatus::Completed)
                != self.sanitized_output_sha256.is_some()
        {
            return Err(CompactResultError::InvalidRecord);
        }
        Ok(())
    }

    pub(crate) fn source_bindings(&self) -> impl Iterator<Item = &HostSourceBinding> {
        self.inputs.iter().map(|input| &input.source)
    }

    #[must_use]
    /// Returns SHA-256 of the exact model-authored source.
    pub fn script_sha256(&self) -> &str {
        &self.script_sha256
    }
    #[must_use]
    /// Returns host-only bound input provenance.
    pub fn inputs(&self) -> &[ComputationInputProvenance] {
        &self.inputs
    }
    #[must_use]
    /// Returns the exact limits applied to the request.
    pub const fn limits(&self) -> &ComputationAppliedLimits {
        &self.limits
    }
    #[must_use]
    /// Returns the trusted terminal category.
    pub const fn terminal_status(&self) -> ComputationTerminalStatus {
        self.terminal_status
    }
    #[must_use]
    /// Returns the digest of the validated output, if completed.
    pub fn sanitized_output_sha256(&self) -> Option<&str> {
        self.sanitized_output_sha256.as_deref()
    }
    #[must_use]
    /// Returns advisory prompt-injection findings for untrusted boundaries.
    pub fn prompt_safety_findings(&self) -> &[ComputationPromptSafetyFinding] {
        &self.prompt_safety_findings
    }
}

impl fmt::Debug for ComputationProvenanceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComputationProvenanceRecord")
            .field("script_sha256", &self.script_sha256)
            .field("input_count", &self.inputs.len())
            .field("terminal_status", &self.terminal_status)
            .field("sanitized_output_sha256", &self.sanitized_output_sha256)
            .finish_non_exhaustive()
    }
}

fn valid_input_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_INPUT_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
