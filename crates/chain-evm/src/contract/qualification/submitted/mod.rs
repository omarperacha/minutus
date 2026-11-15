//! Receipt-only reconciliation of an exact already-submitted type-2 transaction.
//! This grants no write, source, execution-effect or task-completion authority.
mod record;
mod verification;

pub use record::VerifiedSubmittedTransaction;
pub use verification::verify_submitted_transaction;
