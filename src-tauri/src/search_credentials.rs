//! Native macOS Brave Search credential setup and Keychain storage.

use ipc_types::{IpcError, IpcErrorCode};
use web_retrieval::BraveSearchCredential;

const KEYCHAIN_SERVICE: &str = "com.flowzed.zeta.search";
const KEYCHAIN_ACCOUNT: &str = "brave-search-api";

#[cfg(target_os = "macos")]
pub(crate) fn load() -> Result<BraveSearchCredential, IpcError> {
    let bytes =
        security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|_| missing_error())?;
    let value = String::from_utf8(bytes).map_err(|_| missing_error())?;
    BraveSearchCredential::new(value).map_err(|_| missing_error())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn load() -> Result<BraveSearchCredential, IpcError> {
    Err(missing_error())
}

#[tauri::command]
pub(crate) fn brave_search_configured() -> bool {
    #[cfg(target_os = "macos")]
    {
        use security_framework::item::{ItemClass, ItemSearchOptions};

        ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(KEYCHAIN_SERVICE)
            .account(KEYCHAIN_ACCOUNT)
            .load_attributes(true)
            .skip_authenticated_items(true)
            .search()
            .is_ok_and(|items| !items.is_empty())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn configure_brave_search(app: tauri::AppHandle) -> Result<bool, IpcError> {
    use objc2::{MainThreadMarker, rc::autoreleasepool};
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSSecureTextField};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use tokio::sync::oneshot;

    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Add Brave Search API key"));
            alert.setInformativeText(&NSString::from_str(
                "Paste the subscription token from your Brave Search API dashboard. Minutus stores it in macOS Keychain and never adds it to chats or model context.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Save securely"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSSecureTextField::initWithFrame(
                mtm.alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 24.0)),
            );
            field.setPlaceholderString(Some(&NSString::from_str("Brave subscription token")));
            alert.setAccessoryView(Some(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                return Ok(false);
            }
            let value = field.stringValue().to_string();
            field.setStringValue(&NSString::new());
            BraveSearchCredential::new(value.clone()).map_err(|_| IpcError {
                code: IpcErrorCode::InvalidRequest,
                message: "The Brave Search API key is not valid".to_owned(),
                retryable: false,
            })?;
            security_framework::passwords::set_generic_password(
                KEYCHAIN_SERVICE,
                KEYCHAIN_ACCOUNT,
                value.as_bytes(),
            )
            .map_err(|_| IpcError {
                code: IpcErrorCode::Internal,
                message: "Minutus could not save the Brave Search API key to macOS Keychain"
                    .to_owned(),
                retryable: true,
            })?;
            Ok(true)
        });
        let _ = sender.send(result);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.await.map_err(|_| IpcError::internal())?
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub(crate) async fn configure_brave_search(_app: tauri::AppHandle) -> Result<bool, IpcError> {
    Err(missing_error())
}

#[tauri::command]
pub(crate) fn remove_brave_search() -> Result<bool, IpcError> {
    #[cfg(target_os = "macos")]
    {
        if !brave_search_configured() {
            return Ok(false);
        }
        security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|_| IpcError {
                code: IpcErrorCode::Internal,
                message: "Minutus could not remove the Brave Search API key from macOS Keychain"
                    .to_owned(),
                retryable: true,
            })?;
        Ok(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(missing_error())
    }
}

fn missing_error() -> IpcError {
    IpcError {
        code: IpcErrorCode::Unsupported,
        message: "Brave Search is not configured in this device's Keychain".to_owned(),
        retryable: false,
    }
}
