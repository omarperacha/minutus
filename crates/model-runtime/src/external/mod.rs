//! User-owned catalogue files: read-only verification and private runtime references.
mod glimmer;
mod ollama;
pub use glimmer::register_external_glimmer_metadata;
mod verification;
pub use ollama::{
    link_external_blob, register_external_qwen_metadata, unlink_external_blob,
    unregister_external_model_metadata,
};
pub use verification::{
    ExternalFileVerification, ExternalModelError, external_file_stamp, verify_external_file,
};
