//! Bounded caller-declared ABI interpretation. No source/effect qualification.
mod call;
mod layout;
mod types;
mod values;

pub use call::DeclaredContractCall;

const MAX_BYTES: usize = 8192;
const MAX_ITEMS: usize = 32;
const MAX_NODES: usize = 256;
const MAX_BLOB: usize = 1024;

#[cfg(test)]
mod tests;
