//! Closed structured diagnostic event schema.

use serde::{Deserialize, Serialize};

/// Trusted native component producing an operational event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    /// Native application composition.
    Application,
    /// Ordinary application persistence.
    Storage,
    /// Local model process supervision.
    ModelRuntime,
    /// Isolated sandbox computation supervision.
    Computation,
    /// Agent state-machine coordination.
    Agent,
    /// Workspace capability broker.
    Filesystem,
    /// Isolated wallet boundary.
    Wallet,
    /// Chain and transaction policy.
    Transaction,
    /// Bounded network and x402 client.
    Network,
}

/// Allow-listed operation category with no arguments or result contents.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Initialize a component.
    Initialize,
    /// Shut down a component.
    Shutdown,
    /// Read non-secret local state.
    Read,
    /// Persist non-secret local state.
    Persist,
    /// Execute a capability-checked tool.
    ExecuteTool,
    /// Validate untrusted input.
    Validate,
    /// Apply a human permission decision.
    ApplyPermission,
    /// Prepare a diagnostic export.
    PrepareExport,
    /// Write an explicitly requested diagnostic export.
    WriteExport,
}

/// Bounded result category that contains no error message.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOutcome {
    /// Operation completed as intended.
    Succeeded,
    /// Operation was rejected by policy or validation.
    Rejected,
    /// Operation was cancelled without an effect.
    Cancelled,
    /// Operation failed without logging internal data.
    Failed,
}

/// Versioned, content-free effective inference configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceTuningProfile {
    /// Existing manifest policy, without a hardware qualification claim.
    QwenExistingV1,
    /// Measured M1 Pro/32 GiB, pinned bundle, 32k context and draft depth four.
    QwenM1ProMtp4V1,
}

/// Content-free category for one failed chat generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationFailureCategory {
    /// Model prompt construction or exact context admission failed.
    PromptPreparation,
    /// Local model generation or streaming failed.
    Inference,
    /// The bounded operation allowance was exhausted.
    ToolLimit,
    /// A model-authored tool request failed closed validation.
    ToolValidation,
    /// A model-authored call targeted a tool hidden by the current phase.
    OrchestrationPhase,
    /// Safe-boundary conversation persistence failed.
    Persistence,
}

/// Local model lifecycle phase without model inputs or outputs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPhase {
    /// Model artifact validation.
    ValidateArtifact,
    /// Local runtime startup.
    Start,
    /// Local runtime health check.
    HealthCheck,
    /// Generation lifecycle, excluding prompt and response content.
    Generate,
    /// Generation cancellation.
    Cancel,
    /// Local runtime shutdown.
    Stop,
}

/// Permission category without paths, origins, transaction data or reasons.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCapability {
    /// Read within an already selected workspace grant.
    WorkspaceRead,
    /// Write an already staged workspace change.
    WorkspaceWrite,
    /// Make one bounded network request.
    NetworkRequest,
    /// Review one transaction candidate.
    TransactionReview,
}

/// Allow-listed diagnostic events.
///
/// Variants intentionally contain no arbitrary string, byte array, JSON value,
/// path, URL, address, digest, signature, prompt or result-content field.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DiagnosticEvent {
    /// Native application initialized its core schemas.
    ApplicationStarted {
        /// Frontend IPC schema version.
        ipc_schema_version: u16,
        /// Ordinary persistence schema version.
        storage_schema_version: u32,
    },
    /// Native application is shutting down normally.
    ApplicationStopped,
    /// User-controlled local logging state changed.
    LoggingChanged {
        /// New local logging state.
        enabled: bool,
    },
    /// One component operation completed.
    OperationCompleted {
        /// Trusted component category.
        component: Component,
        /// Allow-listed operation category.
        operation: Operation,
        /// Bounded result with no diagnostic message.
        outcome: DiagnosticOutcome,
        /// Coarse operation time in milliseconds.
        duration_ms: u64,
    },
    /// Local model lifecycle event without model content.
    ModelLifecycle {
        /// Lifecycle phase.
        phase: ModelPhase,
        /// Bounded result.
        outcome: DiagnosticOutcome,
    },
    /// Human permission result without request content.
    PermissionResolved {
        /// Capability category only.
        capability: PermissionCapability,
        /// Bounded result.
        outcome: DiagnosticOutcome,
    },
    /// Effective settings for one generation, with no prompt or host paths.
    InferenceProfileSelected {
        /// Closed, versioned configuration identity.
        profile: InferenceTuningProfile,
        /// Host-admitted context window.
        context_tokens: u32,
    },
    /// One chat generation failed without retaining request or response content.
    GenerationFailed {
        /// Trusted coarse failure category.
        category: GenerationFailureCategory,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiagnosticRecord {
    pub(crate) schema_version: u16,
    pub(crate) timestamp_unix_ms: u64,
    pub(crate) sequence: u64,
    pub(crate) data: DiagnosticEvent,
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn inference_profile_diagnostics_accept_only_closed_content_free_fields()
    -> Result<(), serde_json::Error> {
        let event = DiagnosticEvent::InferenceProfileSelected {
            profile: InferenceTuningProfile::QwenM1ProMtp4V1,
            context_tokens: 32_768,
        };
        let mut encoded = serde_json::to_value(event)?;
        assert_eq!(
            serde_json::from_value::<DiagnosticEvent>(encoded.clone())?,
            event
        );
        encoded["prompt"] = serde_json::json!("synthetic forbidden field");
        assert!(serde_json::from_value::<DiagnosticEvent>(encoded).is_err());
        assert!(serde_json::from_str::<InferenceTuningProfile>("\"arbitrary text\"").is_err());
        Ok(())
    }
}
