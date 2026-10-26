//! Disabled-by-default native-only password-wallet test composition.

mod control;
mod flow;
mod native;
mod recovery;
mod synthetic;
#[cfg(test)]
mod tests;

use control::Control;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tauri::{
    Manager as _,
    menu::{Menu, MenuItem, Submenu},
};

pub(crate) fn initialize(
    app: &mut tauri::App,
    data: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = data.join("password-vault-prototype");
    let control = Arc::new(Control::initialize(&directory)?);
    app.manage(control);
    let lifecycle_app = app.handle().clone();
    desktop_lifecycle::install_lock_notifications(move |event| {
        if !Control::should_lock_for(event) {
            return;
        }
        lock(&lifecycle_app);
    })?;
    let item = MenuItem::with_id(
        app,
        "password-vault-prototype",
        "Test password wallet…",
        true,
        None::<&str>,
    )?;
    let submenu = Submenu::with_items(app, "Developer", true, &[&item])?;
    let menu = if let Some(menu) = app.menu() {
        menu
    } else {
        Menu::new(app)?
    };
    menu.append(&submenu)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == "password-vault-prototype" {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                flow::open(&app).await;
            });
        }
    });
    // The ordinary controller remains empty and its actions are disabled in
    // this explicitly selected test build. Existing vault files are never read.
    Ok(directory.join("legacy-disabled"))
}

pub(crate) fn lock(app: &tauri::AppHandle) {
    if let Some(control) = app.try_state::<Arc<Control>>() {
        let control = control.inner().clone();
        if control.queue_lock() {
            tauri::async_runtime::spawn_blocking(move || control.complete_queued_lock());
        }
        native::abort_prompt(app);
    }
}

pub(crate) fn enforce_inactivity(app: &tauri::AppHandle, now: Instant) {
    if let Some(control) = app.try_state::<Arc<Control>>() {
        control.enforce_inactivity(now);
    }
}
