//! Native lifecycle registration and composition into the shared wallet owner.
use super::WalletControl;
#[cfg(all(target_os = "macos", not(feature = "password-vault-prototype")))]
use super::native;
use tauri::Emitter as _;

impl WalletControl {
    /// Queue non-secret cache invalidation after Rust has revoked authority.
    pub(super) fn notify_locked(&self) {
        if let Some(app) = self.app.get() {
            let target = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _ = target.emit("wallet-locked", ());
            });
        }
    }
}
/// Compose non-secret lifecycle revocation into the ordinary wallet owner.
#[cfg_attr(
    any(feature = "password-vault-prototype", not(target_os = "macos")),
    expect(
        clippy::unnecessary_wraps,
        reason = "Ordinary macOS installation propagates notification errors; the isolated harness already owns registration"
    )
)]
pub(crate) fn install_host(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager as _;
    app.state::<WalletControl>().bind_app(app);
    #[cfg(all(target_os = "macos", not(feature = "password-vault-prototype")))]
    {
        let host = app.clone();
        desktop_lifecycle::install_lock_notifications(move |event| {
            if event == desktop_lifecycle::LifecycleEvent::SecurityBoundary {
                host.state::<WalletControl>().handle_system_sleep();
                native::abort_prompt(&host);
            }
        })?;
    }
    Ok(())
}
