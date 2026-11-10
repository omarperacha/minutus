//! Generic compiled source correspondence and complete typed state effects.
mod abi;
mod blob;
mod candidate;
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
pub use completion::{
    GenericConditionObservation, VerifiedGenericCompletion, VerifiedGenericConditions,
    VerifiedGenericGoals, VerifiedGenericPrerequisites, bind_generic_prerequisites,
    verify_generic_completion, verify_generic_conditions, verify_generic_goals,
    verify_generic_prerequisites,
};
pub use condition::{
    GenericComparison, GenericCondition, admit_generic_condition, generic_condition_type,
};
pub use effect_envelope::{
    GenericEffectEnvelope, GenericEffectOperation, GenericEffectRelation, admit_generic_transition,
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
    generic_contract_intent, prepare_generic_contract, qualify_generic_source,
    qualify_generic_source_at,
};
pub use refresh::{RefreshedGenericCandidate, ValidatedGenericExecution, refresh_generic_contract};
pub use source_broker::GenericSourceProvider;
pub use source_metadata::{
    GenericSourceContractMetadata, GenericSourceMetadata, GenericSourceRole,
};
