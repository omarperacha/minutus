//! Model-neutral agent runtime contracts.
//!
//! Tool authority starts here: callers may project trusted registry entries to
//! any model adapter, but model-authored names and arguments must return through
//! [`ToolRegistry::validate_request`] before an executor sees them.

mod activity;
mod agent_state;
mod compact_result;
mod context_admission;
mod definition;
mod error;
mod execution_profile;
mod ledger;
mod ledger_projection;
mod ledger_types;
mod loop_control;
mod permission;
mod prompt_safety;
mod registry;
mod request;
mod result;
mod rich_evidence;
mod schema;
mod state_machine;
mod system_instructions;
mod working_context;

pub use activity::{AgentActivity, AgentActivityKind, AgentActivityStatus};
pub use agent_state::{
    AgentFailure, AgentState, AgentStateKind, AgentStateMachineError, RunCancellationReason,
    ToolRequestIdentity, ToolTerminalStatus, TransitionToken,
};
pub use compact_result::{
    CompactModelSource, CompactRehydrationContext, CompactResultError, CompactResultStatus,
    CompactToolResultProjection, CompactToolResultRecord, ComputationAppliedLimits,
    ComputationInputProvenance, ComputationPromptSafetyFinding, ComputationPromptSafetyTarget,
    ComputationProvenanceRecord, ComputationTerminalStatus, HostSourceBinding, ModelVisibleExcerpt,
    RetainedToolPayload,
};
pub use context_admission::{
    AdmittedModelPass, AdmittedToolRound, ContextAdmissionController, ContextAdmissionError,
    ContextAdmissionPolicy, ContextTokenEstimate, ToolResultAdmission,
};
pub use execution_profile::{
    EXECUTION_PROFILE_POLICY_VERSION, ExecutionComplexity, ExecutionProfile,
    ExecutionProfileDecision, ExecutionProfileSignals, ExecutionRisk, ExecutionWorkKind,
};

pub use definition::{
    ApprovalRequirement, ModelToolDefinition, ProjectCapability, RiskLevel, ToolClassification,
    ToolDefinition, ToolLimits,
};
pub use error::ToolRegistryError;
pub use ledger::{ExecutionLedger, ExecutionLedgerSnapshot};
pub use ledger_projection::{
    ModelFailureFact, ModelLedgerProjection, ModelMutationChange, ModelMutationFact,
    ModelObservationFact, ModelPartialOutcome, ModelPermissionFact, ModelWorkspaceFact,
};
pub use ledger_types::{
    ExecutionBudgets, ExecutionLedgerError, FactOrigin, LedgerCapability, LedgerNextAction,
    LedgerPermissionDecision, LedgerRequest, LedgerResource, LedgerTerminalState, MutationChange,
    MutationFact, NewMutationChange, NewObservation, ObservationFact, ObservedValues,
    PartialOutcome, PermissionFact, ReuseContext, UnresolvedFailure, WorkspaceResolution,
};
pub use loop_control::{
    AgentLoopController, AgentLoopSnapshot, FinalAnswerReason, GenerationUsage, LoopControlError,
    LoopDirective, LoopLimits, RoundAllowance, ToolCallIntent, ToolCallPlan, ToolExecutionUsage,
};
pub use permission::{
    DurableCapability, ExecutionAuthorization, PermissionBinding, PermissionDecisionReceipt,
    PermissionMediationError, PermissionMediator, PermissionMediatorSnapshot, PermissionPrompt,
    PermissionResourceKind, PermissionResourceScope,
};
pub use prompt_safety::{
    BoundedResourceReference, InjectionSignal, PromptInjectionDetector, PromptInjectionIndicator,
    PromptSafetyError, UntrustedContentSource, UntrustedSourceKind,
};
pub use registry::ToolRegistry;
pub use request::{UntrustedToolRequest, ValidatedToolRequest};
pub use result::{
    CancellationReason, OpaqueLocalReference, OpaqueReferenceKind, ResultContent,
    SupportedResultType, ToolFailure, ToolOutcome, ToolResultEnvelope, ValidatedAssetReference,
    ValidatedImageMediaType,
};
pub use rich_evidence::{RichEvidenceCheckpoint, RichEvidenceSource};
pub use state_machine::{
    AgentStateMachine, AgentStateSnapshot, CancellationReceipt, CancellationTarget, RecoveryAction,
    RestartDisposition, StateTransitionRecord, UnsafeInterruption,
};
pub use system_instructions::{
    AGENT_SYSTEM_POLICY_VERSION, AgentSystemInstructionError, AgentSystemInstructionIdentity,
    AgentSystemInstructions, FlowZedKnowledgeSource, build_agent_system_instructions,
};
pub use working_context::{
    DialogueContinuityCheckpoint, SemanticCompactionCheckpoint, UntrustedWorkingContextSummary,
    WorkingContextCompactor, WorkingContextError, WorkingContextPlan, WorkingContextPolicy,
    WorkingContextProjection, WorkingContextRound,
};
