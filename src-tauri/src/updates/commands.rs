//! Trusted settings actions; no updater authority is exposed to agent tools.

use ipc_types::{IpcError, IpcErrorCode};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use super::{ReleaseUpdateControl, download, policy};

fn failure() -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: "The update could not be verified or saved. Your installed app is unchanged."
            .to_owned(),
        retryable: true,
    }
}

#[tauri::command]
pub(crate) fn release_updates_configured() -> bool {
    policy::public_key(policy::PUBLIC_KEY).is_ok()
}

#[tauri::command]
pub(crate) async fn check_release_update(
    app: AppHandle,
    control: State<'_, ReleaseUpdateControl>,
) -> Result<Option<String>, IpcError> {
    let mut pending = control.0.try_lock().map_err(|_| failure())?;
    *pending = None;
    let current = app.package_info().version.to_string();
    let release = tauri::async_runtime::spawn_blocking(move || download::check(&current))
        .await
        .map_err(|_| failure())?
        .map_err(|()| failure())?;
    let version = release.as_ref().map(|value| value.version.clone());
    *pending = release;
    Ok(version)
}

#[tauri::command]
pub(crate) async fn save_verified_release_update(
    app: AppHandle,
    control: State<'_, ReleaseUpdateControl>,
) -> Result<bool, IpcError> {
    let mut pending = control.0.try_lock().map_err(|_| failure())?;
    let release = pending.take().ok_or_else(failure)?;
    let current = app.package_info().version.to_string();
    policy::validate_release(&release, &current).map_err(|()| failure())?;
    let version = release.version.clone();
    let artifact =
        tauri::async_runtime::spawn_blocking(move || download::verified_artifact(&release))
            .await
            .map_err(|_| failure())?
            .map_err(|()| failure())?;
    // Save Panel grants only the file the person chose. No self-replacement,
    // privilege escalation, app-folder grants or sandbox exceptions are needed.
    tauri::async_runtime::spawn_blocking(move || {
        let selection = app
            .dialog()
            .file()
            .set_file_name(format!("Minutus-{version}-arm64.dmg"))
            .add_filter("Disk image", &["dmg"])
            .blocking_save_file();
        let Some(selection) = selection else {
            return Ok(false);
        };
        let path = selection.into_path().map_err(|_| failure())?;
        let mut source = artifact.reopen().map_err(|_| failure())?;
        let mut destination = std::fs::File::create(&path).map_err(|_| failure())?;
        std::io::copy(&mut source, &mut destination).map_err(|_| failure())?;
        destination.sync_all().map_err(|_| failure())?;
        // Preserve ordinary Gatekeeper handling when Finder opens the installer.
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| failure())?
            .as_secs();
        let quarantine = format!("0081;{timestamp:x};Minutus;");
        xattr::set(&path, "com.apple.quarantine", quarantine.as_bytes()).map_err(|_| failure())?;
        Ok(true)
    })
    .await
    .map_err(|_| failure())?
}
