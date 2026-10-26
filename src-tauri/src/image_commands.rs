//! Managed workspace image selection, import, preview, and cleanup commands.

use super::{
    AppCore, AppCoreError, Arc, AssetId, DialogExt, File, ImageAttachmentReceipt, IpcError,
    IpcErrorCode, MAX_CHAT_ASSETS, MAX_IMAGE_IMPORT_BYTES, Path, Read, State, ValidateIpc,
    app_core_error, current_timestamp,
};

#[tauri::command]
pub(crate) async fn pick_chat_images(
    app: tauri::AppHandle,
    core: State<'_, Arc<AppCore>>,
    limit: u8,
) -> Result<Option<Vec<ImageAttachmentReceipt>>, IpcError> {
    let limit = usize::from(limit);
    if limit == 0 || limit > MAX_CHAT_ASSETS {
        return Err(IpcError::invalid_request("The attachment limit is invalid"));
    }
    let selected = app
        .dialog()
        .file()
        .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
        .blocking_pick_files();
    let Some(selected) = selected else {
        return Ok(None);
    };
    if selected.len() > limit {
        return Err(IpcError::payload_too_large(format!(
            "Select no more than {limit} images"
        )));
    }
    let paths = selected
        .into_iter()
        .map(|selected| {
            selected.into_path().map_err(|_| {
                IpcError::invalid_request("The selected item is not a local image file")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let imported_at = current_timestamp()?;
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let mut receipts = Vec::with_capacity(paths.len());
        for path in paths {
            let result = read_bounded_image(&path)
                .and_then(|bytes| import_image_receipt(&core, &bytes, &imported_at));
            match result {
                Ok(receipt) => receipts.push(receipt),
                Err(error) => {
                    for receipt in &receipts {
                        let _ = core.discard_unreferenced_image(&receipt.asset_id);
                    }
                    return Err(error);
                }
            }
        }
        Ok(Some(receipts))
    })
    .await
    .map_err(|_| IpcError::internal())?
}

#[tauri::command]
pub(crate) async fn import_chat_image(
    core: State<'_, Arc<AppCore>>,
    request: tauri::ipc::Request<'_>,
) -> Result<ImageAttachmentReceipt, IpcError> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        tauri::ipc::InvokeBody::Json(_) => {
            return Err(IpcError::invalid_request("Image bytes are required"));
        }
    };
    if bytes.len() > MAX_IMAGE_IMPORT_BYTES {
        return Err(image_limit_error());
    }
    let bytes = bytes.clone();
    let imported_at = current_timestamp()?;
    let core = Arc::clone(core.inner());
    tauri::async_runtime::spawn_blocking(move || import_image_receipt(&core, &bytes, &imported_at))
        .await
        .map_err(|_| IpcError::internal())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn managed_image_preview(
    core: State<'_, Arc<AppCore>>,
    asset_id: AssetId,
) -> Result<tauri::ipc::Response, IpcError> {
    asset_id.validate()?;
    core.inference_asset(&asset_id)
        .map(tauri::ipc::Response::new)
        .map_err(image_import_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn discard_chat_image(
    core: State<'_, Arc<AppCore>>,
    asset_id: AssetId,
) -> Result<bool, IpcError> {
    asset_id.validate()?;
    core.discard_unreferenced_image(&asset_id)
        .map_err(image_import_error)
}

pub(crate) fn import_image_receipt(
    core: &AppCore,
    bytes: &[u8],
    imported_at: &str,
) -> Result<ImageAttachmentReceipt, IpcError> {
    let asset = core
        .import_image(bytes, imported_at)
        .map_err(image_import_error)?;
    Ok(ImageAttachmentReceipt {
        asset_id: asset.id,
        width: asset.original.width,
        height: asset.original.height,
    })
}

pub(crate) fn read_bounded_image(path: &Path) -> Result<Vec<u8>, IpcError> {
    let file = File::open(path)
        .map_err(|_| IpcError::invalid_request("The selected image could not be read"))?;
    let metadata = file
        .metadata()
        .map_err(|_| IpcError::invalid_request("The selected image could not be read"))?;
    if !metadata.is_file() {
        return Err(IpcError::invalid_request(
            "The selected item is not a regular image file",
        ));
    }
    if metadata.len() > MAX_IMAGE_IMPORT_BYTES as u64 {
        return Err(image_limit_error());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take((MAX_IMAGE_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| IpcError::invalid_request("The selected image could not be read"))?;
    if bytes.len() > MAX_IMAGE_IMPORT_BYTES {
        return Err(image_limit_error());
    }
    Ok(bytes)
}

pub(crate) fn image_limit_error() -> IpcError {
    IpcError::payload_too_large("Images must be 20 MiB or smaller")
}

pub(crate) fn image_import_error(error: AppCoreError) -> IpcError {
    match error {
        AppCoreError::AssetNotFound => IpcError::invalid_request("The image is unavailable"),
        AppCoreError::AssetStore(_) => IpcError {
            code: IpcErrorCode::Unsupported,
            message: "Use a valid static PNG, JPEG or WebP image within the supported limits"
                .to_owned(),
            retryable: false,
        },
        other => app_core_error(other),
    }
}
