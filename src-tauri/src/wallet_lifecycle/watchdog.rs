//! Timer fallback for inactivity and missed system-sleep observations.
use super::WalletControl;
use tauri::Manager as _;

pub(crate) fn start_watchdog(app: &tauri::AppHandle) {
    let wallet_watchdog_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let cadence = std::time::Duration::from_secs(15);
        let sleep_gap = std::time::Duration::from_secs(45);
        let mut interval = tokio::time::interval(cadence);
        let mut last_tick = std::time::Instant::now();
        loop {
            interval.tick().await;
            let now = std::time::Instant::now();
            let control = wallet_watchdog_app.state::<WalletControl>();
            if now.saturating_duration_since(last_tick) >= sleep_gap {
                control.handle_system_sleep();
                #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
                crate::password_wallet::lock(&wallet_watchdog_app);
            } else {
                control.enforce_inactivity(now);
                #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
                crate::password_wallet::enforce_inactivity(&wallet_watchdog_app, now);
            }
            last_tick = now;
        }
    });
}
