//! External model references: registration, availability, and runtime resolution.
mod access;
mod availability;
mod modules;
mod registration;
mod runtime;

pub(crate) use access::DirectFileAccess;
pub use access::ExternalModelAccess;

#[cfg(test)]
mod tests;
