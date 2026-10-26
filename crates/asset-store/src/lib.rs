//! Private, bounded local image ingestion and managed asset files.
//!
//! This crate accepts bytes, never source paths or URLs. It validates and
//! normalizes supported images before publishing files under Rust-generated
//! opaque identifiers. Database coordination remains owned by `app-core` and
//! `app-storage`.

mod error;
mod media;
mod store;

pub use error::AssetStoreError;
pub use media::{MAX_DECODED_PIXELS, MAX_ENCODED_BYTES, MAX_INFERENCE_PIXELS};
pub use store::{AssetFileStore, ImportedAsset};
