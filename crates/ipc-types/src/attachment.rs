//! Frontend-safe receipts for managed image attachments.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::AssetId;

/// Opaque receipt returned after a local image has been validated and imported.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageAttachmentReceipt {
    /// Stable managed-asset identifier used when sending the message.
    pub asset_id: AssetId,
    /// Display width after orientation correction.
    pub width: u32,
    /// Display height after orientation correction.
    pub height: u32,
}
