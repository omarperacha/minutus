//! Restart-safe EVM ownership, dispatch and monitoring coordination.
mod dispatch;
mod monitoring;
mod nonce;
mod turn_admission;

pub use nonce::EvmNonceReservation;
