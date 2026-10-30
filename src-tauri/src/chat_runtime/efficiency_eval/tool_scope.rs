//! Additional deny-only scope for the isolated debug evaluator. This cannot
//! approve a tool, create a wallet, allow network RPC or mint production authority.
use ipc_types::IpcError;
use tauri::Manager as _;

pub(super) struct RestrictedTools {
    _private: (),
}
impl RestrictedTools {
    pub(super) const fn new() -> Self {
        Self { _private: () }
    }
}
pub(crate) fn tool_denial(app: &tauri::AppHandle, name: &str) -> Option<IpcError> {
    app.try_state::<RestrictedTools>()?;
    let wallet_absent = app
        .try_state::<crate::wallet_lifecycle::WalletControl>()
        .is_some_and(|wallet| wallet.public_state().is_ok_and(|state| no_wallet(&state)));
    if allowed(name, wallet_absent) {
        None
    } else {
        Some(IpcError::permission_denied(
            "This isolated evaluation denies outbound, native-dialog and external-effect tools before dispatch; no request was sent",
        ))
    }
}
fn no_wallet(state: &ipc_types::PublicWalletState) -> bool {
    state.availability == ipc_types::WalletAvailability::Unavailable
        && state.wallet_id.is_none()
        && state.address.is_none()
}
fn allowed(name: &str, wallet_absent: bool) -> bool {
    matches!(
        name,
        "orchestration.search_tools"
            | "orchestration.submit_plan"
            | "orchestration.submit_forecast"
            | "orchestration.complete_stage"
            | "orchestration.refine_stage"
            | "orchestration.recall"
    ) || (name == "contract.request_workflow" && wallet_absent)
}
#[cfg(test)]
mod tests {
    #[test]
    fn only_proven_absence_can_reach_the_empty_wallet_fixture() {
        let mut state = ipc_types::PublicWalletState {
            wallet_id: None,
            address: None,
            network: None,
            availability: ipc_types::WalletAvailability::Unavailable,
        };
        assert!(super::no_wallet(&state));
        state.availability = ipc_types::WalletAvailability::Locked;
        assert!(!super::no_wallet(&state));
        state.availability = ipc_types::WalletAvailability::Ready;
        assert!(!super::no_wallet(&state));
        state.availability = ipc_types::WalletAvailability::Unavailable;
        state.wallet_id = Some(ipc_types::WalletId("fixture".into()));
        assert!(!super::no_wallet(&state));
        state.wallet_id = None;
        state.address = Some("0x1111111111111111111111111111111111111111".into());
        assert!(!super::no_wallet(&state));
    }

    use super::*;
    #[test]
    fn evaluation_scope_has_no_permissive_effect_or_namespace_override() {
        for name in [
            "contract.inspect",
            "contract.read",
            "wallet.request_transfer",
            "web.search",
            "web.retrieve",
            "filesystem.read_file",
            "filesystem.write_file",
            "computation.run",
            "orchestration.transform_artifact",
            "orchestration.future_effect",
        ] {
            assert!(!allowed(name, true));
            assert!(!allowed(name, false));
        }
        assert!(allowed("orchestration.search_tools", false));
        assert!(allowed("orchestration.submit_plan", false));
        assert!(allowed("contract.request_workflow", true));
        assert!(!allowed("contract.request_workflow", false));
    }
}
