//! Public selected-account display persistence, never encrypted authority.
use super::WalletControl;
use ipc_types::IpcError;
pub(super) fn persist_public(
    core: &app_core::AppCore,
    control: &WalletControl,
) -> Result<(), IpcError> {
    let state = control.public_state()?;
    let Some(id) = state.wallet_id else {
        return Ok(());
    };
    let updated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| IpcError::internal())?;
    let wallet = if let (Some(address), Some(network)) = (state.address, state.network) {
        Some(app_core::PublicWalletMetadata {
            id,
            address,
            network,
            locked: state.availability != ipc_types::WalletAvailability::Ready,
            updated_at,
        })
    } else {
        // Revocation must still mark previously authenticated display records
        // locked, without copying an unauthenticated envelope address into them.
        core.public_wallet(&id)
            .map_err(crate::error_mapping::app_core_error)?
            .map(|mut previous| {
                previous.locked = true;
                previous.updated_at = updated_at;
                previous
            })
    };
    if let Some(wallet) = wallet {
        core.save_public_wallet(&wallet)
            .map_err(crate::error_mapping::app_core_error)?;
    }
    Ok(())
}
