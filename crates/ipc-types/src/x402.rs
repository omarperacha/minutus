//! Frontend-safe receipts from trusted native x402 review.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TransactionProposalId;

/// Distinct result produced by the native x402 approval surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum X402ApprovalDecision {
    /// User approved the exact payment in the native surface.
    Approved,
    /// User explicitly denied it.
    Denied,
    /// The native surface closed without approval.
    Dismissed,
    /// Trusted host time reached the exact review deadline.
    Expired,
}

/// Display-only receipt; never an approval commitment or signing capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct X402ApprovalReviewOutcome {
    /// Exact opaque review reference consumed by the native surface.
    pub review_id: TransactionProposalId,
    /// Distinct terminal native decision.
    pub decision: X402ApprovalDecision,
}
