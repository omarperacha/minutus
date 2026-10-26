//! Installed-model lifecycle, selection, loading, and provider coordination.

mod external;
pub(crate) use external::DirectFileAccess;
pub use external::ExternalModelAccess;
mod lifecycle;
pub(super) mod ollama_service;
#[cfg(test)]
mod selection_tests;
pub(super) mod tuning;

#[cfg(test)]
pub(crate) use lifecycle::{
    complete_bundle_companions, resolve_new_conversation_selection, validate_inference_limits,
};
