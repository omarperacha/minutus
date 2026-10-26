//! Frontend-safe transaction proposal summaries.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{TransactionProposalId, WalletId};

/// High-level transaction action rendered for review.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TransactionAction {
    /// Native or token transfer.
    Transfer,
    /// Smart-contract interaction.
    ContractCall,
    /// Typed structured-data signature request.
    TypedData,
    /// x402 payment.
    X402Payment,
}

/// One trusted, human-readable transaction field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionField {
    /// Short field label.
    pub label: String,
    /// Decoded display value.
    pub value: String,
    /// Optional warning produced by trusted policy code.
    pub warning: Option<String>,
}

/// Public result of transaction simulation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SimulationState {
    /// Simulation has not run.
    NotRun,
    /// The exact candidate simulated successfully.
    Succeeded,
    /// Simulation failed or reverted.
    Failed,
}

/// Sanitized simulation summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionSimulation {
    /// Current simulation state.
    pub state: SimulationState,
    /// Human-readable summary without raw RPC data.
    pub summary: Option<String>,
}

/// Decoded proposal safe to display in the ordinary webview.
///
/// Raw signing payloads, signatures and private material are intentionally not
/// fields of this type. Final authorization remains in the trusted native
/// wallet surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionProposal {
    /// Stable proposal identifier.
    pub id: TransactionProposalId,
    /// Public wallet metadata used for the candidate.
    pub wallet_id: WalletId,
    /// CAIP-2 network identifier.
    pub network: String,
    /// Public sender account.
    pub account: String,
    /// High-level action.
    pub action: TransactionAction,
    /// Trusted one-line summary.
    pub summary: String,
    /// Ordered decoded display fields.
    pub fields: Vec<TransactionField>,
    /// Estimated fee rendered as a decimal display string.
    pub estimated_fee: Option<String>,
    /// Simulation result bound to this proposal.
    pub simulation: TransactionSimulation,
    /// RFC 3339 expiry timestamp.
    pub expires_at: String,
}
