//! Host-only single-use admission immediately before an irreversible network send.
use super::{WalletControl, cancelled_error, unix_now};
use ipc_types::{IpcError, WalletAvailability};
use std::sync::atomic::Ordering;
use wallet_core::{AppVaultDocument, VaultKeyId};

/// A signature alone does not bypass lifecycle/selection checks before submission.
pub(crate) struct WalletSubmission {
    control: WalletControl,
    document: AppVaultDocument,
    id: VaultKeyId,
    epoch: u64,
    expires_at: u64,
}
impl WalletSubmission {
    pub(super) fn new(
        control: WalletControl,
        document: AppVaultDocument,
        id: VaultKeyId,
        epoch: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            control,
            document,
            id,
            epoch,
            expires_at,
        }
    }
    /// Consumes final admission. After this boundary a send may have happened;
    /// lock/cancellation cannot justify a retry or report a known-unsent outcome.
    pub(crate) fn begin(self) -> Result<(), IpcError> {
        let (document, selected) = self.control.snapshot()?;
        let ready = self.control.public_state()?.availability == WalletAvailability::Ready;
        if document.as_ref() != Some(&self.document)
            || selected.as_ref() != Some(&self.id)
            || !ready
            || unix_now()? >= self.expires_at
            || self.control.revoked.load(Ordering::Acquire)
            || self.control.epoch.load(Ordering::Acquire) != self.epoch
        {
            return Err(cancelled_error());
        }
        Ok(())
    }
}
/// Trusted signing result; neither component is serialisable/frontend-visible.
pub(crate) struct WalletSigned {
    pub(crate) signature: wallet_core::EvmTransactionSignature,
    pub(crate) submission: WalletSubmission,
}
