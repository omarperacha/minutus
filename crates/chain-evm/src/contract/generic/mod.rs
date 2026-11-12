//! Generic compiled source correspondence and complete typed state effects.
mod abi;
mod blob;
mod candidate;
mod candidate_read;
mod cohort;
mod completion;
mod condition;
mod effect_envelope;
mod effects;
mod envelope;
mod error;
mod identity;
pub(crate) mod layout;
mod logs;
mod prediction;
mod preparation;
mod proxy;
mod refresh;
mod source_broker;
mod source_metadata;

pub use candidate::GenericContractCandidate;
pub use cohort::{GenericLiveCohort, select_generic_live_cohort};
pub use completion::{
    GenericConditionObservation, VerifiedGenericCompletion, VerifiedGenericConditions,
    VerifiedGenericGoals, VerifiedGenericPrerequisites, bind_generic_prerequisites,
    bind_refreshed_generic_prerequisites, verify_generic_candidate_conditions,
    verify_generic_completion, verify_generic_conditions, verify_generic_conditions_at_cohort,
    verify_generic_goals, verify_generic_prerequisites, verify_refreshed_generic_conditions,
};
pub use condition::{
    GenericComparison, GenericCondition, admit_generic_condition, generic_condition_type,
};
pub use effect_envelope::{
    GenericEffectEnvelope, GenericEffectInvocation, GenericEffectOperation, GenericEffectRelation,
    GenericInvocationRequirements, GenericRequiredCall, admit_generic_transition,
    admit_generic_transition_in_scope, inspect_generic_invocation,
};
pub use effects::{
    GenericCallEffect, GenericEffects, GenericNativeAccountMode, GenericNativeTransfer,
    GenericStorageEffect,
};
pub use error::GenericError;
pub use identity::{GenericCodeIdentity, GenericContractIdentity};
pub use layout::{TypedFieldChange, TypedStorageValue};
pub use logs::GenericLogEffect;
pub use prediction::verify_predicted_generic_conditions;
pub use preparation::{GenericSourceObservation, inspect_generic_source};
pub use preparation::{
    generic_contract_intent, prepare_generic_contract, prepare_generic_contract_at_cohort,
    prepare_generic_contract_in_scope, qualify_generic_source, qualify_generic_source_at,
    qualify_generic_write_source,
};
pub use refresh::{RefreshedGenericCandidate, ValidatedGenericExecution, refresh_generic_contract};
pub use source_broker::{
    GenericSourceProvider, SourceFailure, SourceFailureReason, SourceFailureStage,
};
pub use source_metadata::{
    GenericSourceContractMetadata, GenericSourceImmutableMetadata, GenericSourceMetadata,
    GenericSourceRole,
};
