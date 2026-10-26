//! Public wallet state visible to the ordinary webview.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::WalletId;

/// Public availability of the isolated wallet helper.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WalletAvailability {
    /// No wallet has been configured.
    Unavailable,
    /// Public metadata exists but signing is locked.
    Locked,
    /// The wallet helper is ready for native approval requests.
    Ready,
}

/// Public wallet metadata containing no recovery or private-key material.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicWalletState {
    /// Opaque wallet identifier when configured.
    pub wallet_id: Option<WalletId>,
    /// Public account address when configured.
    pub address: Option<String>,
    /// Active CAIP-2 network identifier when selected.
    pub network: Option<String>,
    /// Current public availability.
    pub availability: WalletAvailability,
}
