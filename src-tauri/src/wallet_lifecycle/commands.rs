//! IPC accepts user intent only; all secrets are requested by native owners.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri command parameters must be owned values for generated invocation handlers"
)]
use super::{WalletControl, persist_public};
use app_core::AppCore;
use ipc_types::{IpcError, PublicWalletState, WalletId};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub(crate) fn wallet_state(
    control: State<'_, WalletControl>,
) -> Result<PublicWalletState, IpcError> {
    control.public_state()
}
#[tauri::command]
pub(crate) fn wallet_inventory(
    control: State<'_, WalletControl>,
) -> Result<Vec<PublicWalletState>, IpcError> {
    control.inventory()
}
#[tauri::command]
pub(crate) fn select_wallet(
    core: State<'_, Arc<AppCore>>,
    control: State<'_, WalletControl>,
    wallet_id: WalletId,
) -> Result<PublicWalletState, IpcError> {
    control.select(&wallet_id)?;
    persist_public(&core, &control)?;
    control.public_state()
}
#[tauri::command]
pub(crate) fn lock_wallet(
    core: State<'_, Arc<AppCore>>,
    control: State<'_, WalletControl>,
) -> Result<PublicWalletState, IpcError> {
    control.lock();
    #[cfg(target_os = "macos")]
    if let Ok(app) = control.app() {
        super::native::abort_prompt(app);
    }
    persist_public(&core, &control)?;
    control.public_state()
}

macro_rules! native_command {
    ($name:ident,$module:ident,$action:ident,$operation:literal $(,$arg:expr)?) => {
        #[tauri::command]
        pub(crate) async fn $name(app:tauri::AppHandle,core:State<'_,Arc<AppCore>>,control:State<'_,WalletControl>)->Result<PublicWalletState,IpcError> {
            #[cfg(target_os="macos")]
            {
                let before=control.inventory().unwrap_or_default();
                let result=super::$module::$action(&app,control.inner().clone() $(,$arg)?).await;
                let _=persist_public(&core,&control);
                if let Err(error)=&result {
                    super::feedback::report_failure(&app,$operation,error).await;
                }
                result?;
                let after=control.inventory()?;
                for old in before {
                    if let Some(id)=old.wallet_id && !after.iter().any(|entry|entry.wallet_id.as_ref()==Some(&id)) {
                        core.remove_public_wallet(&id).map_err(crate::error_mapping::app_core_error)?;
                    }
                }
                control.public_state()
            }
            #[cfg(not(target_os="macos"))]
            {let _=(app,core,control);Err(super::wallet_error("Native password-wallet input is currently qualified for macOS",false))}
        }
    }
}
native_command!(create_wallet, onboarding, create, "Account creation");
native_command!(restore_wallet, onboarding, restore, "Account restoration");
native_command!(
    import_wallet_private_key,
    onboarding,
    import,
    "Private-key import"
);
native_command!(
    change_wallet_password,
    mutations,
    change_password,
    "Password change"
);
native_command!(
    remove_wallet_account,
    mutations,
    remove_account,
    "Account removal"
);
native_command!(delete_wallet_vault, mutations, delete, "Vault deletion");
native_command!(reset_wallet, mutations, reset, "Wallet recovery", false);
native_command!(
    reset_wallet_private_key,
    mutations,
    reset,
    "Wallet recovery",
    true
);

#[tauri::command]
pub(crate) async fn unlock_wallet(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    control: State<'_, WalletControl>,
) -> Result<PublicWalletState, IpcError> {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = control.unlock_native(&app).await {
            super::feedback::report_failure(&app, "Wallet unlock", &error).await;
            return Err(error);
        }
        persist_public(&core, &control)?;
        control.public_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, core, control);
        Err(super::wallet_error(
            "Native password-wallet input is currently qualified for macOS",
            false,
        ))
    }
}
