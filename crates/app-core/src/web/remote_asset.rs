//! Project-bound handoff from permissioned web retrieval to managed assets.

use std::collections::BTreeMap;

use app_storage::AssetReference;
use ipc_types::{AssetId, JsonValue, ProjectId, ToolResultPart};
use web_retrieval::{
    RemoteImageResponse, ValidatedWebMediaType, WebBrokerResponse, WebResponseProvenance,
};

use crate::{AppCore, AppCoreError};

/// Bounded, credential-safe provenance for one imported remote image.
///
/// The exact source URL remains host-only because it may contain sensitive
/// path or query values. This projection contains no response bytes or paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteAssetProvenance {
    source_origin: String,
    media_type: ValidatedWebMediaType,
    byte_count: u64,
    content_sha256: String,
    retrieved_at_unix_millis: u64,
    elapsed_millis: u64,
}

impl RemoteAssetProvenance {
    fn from_web(provenance: &WebResponseProvenance) -> Result<Self, AppCoreError> {
        let media_type = provenance
            .media_type()
            .ok_or(AppCoreError::RemoteImageHandoff(
                web_retrieval::RemoteImageHandoffError::UnsupportedMediaType,
            ))?;
        Ok(Self {
            source_origin: provenance.final_origin().to_owned(),
            media_type,
            byte_count: u64::try_from(provenance.byte_count())
                .map_err(|_| asset_store::AssetStoreError::LimitExceeded)?,
            content_sha256: provenance.content_sha256().to_owned(),
            retrieved_at_unix_millis: provenance.retrieved_at_unix_millis(),
            elapsed_millis: provenance.elapsed_millis(),
        })
    }

    /// Returns the credential-safe final origin without source path or query.
    #[must_use]
    pub fn source_origin(&self) -> &str {
        &self.source_origin
    }

    /// Returns the declared media type admitted by the network response policy.
    #[must_use]
    pub const fn media_type(&self) -> ValidatedWebMediaType {
        self.media_type
    }

    /// Returns the exact encoded remote byte count.
    #[must_use]
    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    /// Returns the lowercase SHA-256 digest of the encoded remote bytes.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    /// Returns the host-recorded retrieval time as Unix milliseconds.
    #[must_use]
    pub const fn retrieved_at_unix_millis(&self) -> u64 {
        self.retrieved_at_unix_millis
    }

    /// Returns the bounded end-to-end retrieval duration.
    #[must_use]
    pub const fn elapsed_millis(&self) -> u64 {
        self.elapsed_millis
    }
}

/// Opaque project-bound receipt for one validated managed remote image.
///
/// It deliberately exposes neither original bytes nor Rust-owned storage keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteAssetReceipt {
    project_id: ProjectId,
    asset_id: AssetId,
    width: u32,
    height: u32,
    inference_byte_count: u64,
    inference_content_sha256: String,
    inference_width: u32,
    inference_height: u32,
    provenance: RemoteAssetProvenance,
}

impl RemoteAssetReceipt {
    /// Returns the project under which the remote retrieval was initiated.
    #[must_use]
    pub const fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    /// Returns the opaque managed-asset identifier used by conversation parts.
    #[must_use]
    pub const fn asset_id(&self) -> &AssetId {
        &self.asset_id
    }

    /// Returns the validated display width after orientation correction.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the validated display height after orientation correction.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the normalized inference derivative byte count.
    #[must_use]
    pub const fn inference_byte_count(&self) -> u64 {
        self.inference_byte_count
    }

    /// Returns the normalized inference derivative digest.
    #[must_use]
    pub fn inference_content_sha256(&self) -> &str {
        &self.inference_content_sha256
    }

    /// Returns normalized inference derivative dimensions.
    #[must_use]
    pub const fn inference_dimensions(&self) -> (u32, u32) {
        (self.inference_width, self.inference_height)
    }

    /// Returns bounded credential-safe remote provenance.
    #[must_use]
    pub const fn provenance(&self) -> &RemoteAssetProvenance {
        &self.provenance
    }

    /// Projects the receipt into path-free tool-result parts.
    ///
    /// The asset part contains only the opaque managed ID. The JSON part is
    /// host-authored bounded provenance and omits the exact source URL.
    #[must_use]
    pub fn tool_result_parts(&self) -> [ToolResultPart; 2] {
        let provenance = BTreeMap::from([
            (
                "byteCount".to_owned(),
                JsonValue::String(self.provenance.byte_count.to_string()),
            ),
            (
                "contentSha256".to_owned(),
                JsonValue::String(self.provenance.content_sha256.clone()),
            ),
            (
                "elapsedMillis".to_owned(),
                JsonValue::String(self.provenance.elapsed_millis.to_string()),
            ),
            (
                "mediaType".to_owned(),
                JsonValue::String(self.provenance.media_type.as_str().to_owned()),
            ),
            (
                "retrievedAtUnixMillis".to_owned(),
                JsonValue::String(self.provenance.retrieved_at_unix_millis.to_string()),
            ),
            (
                "sourceOrigin".to_owned(),
                JsonValue::String(self.provenance.source_origin.clone()),
            ),
        ]);
        [
            ToolResultPart::Asset {
                asset_id: self.asset_id.clone(),
            },
            ToolResultPart::Json {
                value: JsonValue::Object(BTreeMap::from([(
                    "remoteProvenance".to_owned(),
                    JsonValue::Object(provenance),
                )])),
            },
        ]
    }
}

impl AppCore {
    /// Imports one completed permissioned remote image through managed storage.
    ///
    /// The response must already have been produced by consuming the exact
    /// network authorization. It is consumed here and cannot be reused as a URL
    /// or byte-bearing inference input. The returned opaque asset follows the
    /// ordinary conversation-reference, deletion and orphan-reconciliation path.
    ///
    /// # Errors
    ///
    /// Returns an error when the project is absent, the response is not a
    /// completed static image, or full media validation/normalization/persistence
    /// fails. No asset reference is returned on failure.
    pub fn import_remote_image(
        &self,
        project_id: &ProjectId,
        response: WebBrokerResponse,
        imported_at: &str,
    ) -> Result<RemoteAssetReceipt, AppCoreError> {
        let image = RemoteImageResponse::try_from(response)?;
        let provenance = RemoteAssetProvenance::from_web(image.provenance())?;
        self.persist_remote_image(
            project_id,
            image.untrusted_encoded_bytes(),
            imported_at,
            provenance,
        )
    }

    fn persist_remote_image(
        &self,
        project_id: &ProjectId,
        bytes: &[u8],
        imported_at: &str,
        provenance: RemoteAssetProvenance,
    ) -> Result<RemoteAssetReceipt, AppCoreError> {
        let _operation = self
            .asset_operations
            .lock()
            .map_err(|_| AppCoreError::AssetOperationsUnavailable)?;
        if self
            .with_storage(|storage| storage.project(project_id))?
            .is_none()
        {
            return Err(AppCoreError::ProjectNotFound);
        }

        let imported = self.asset_files.import_image(bytes, imported_at)?;
        validate_provenance(imported.metadata(), &provenance)?;
        self.with_storage(|storage| storage.save_asset(imported.metadata()))?;
        let asset = imported.commit();
        Ok(RemoteAssetReceipt {
            project_id: project_id.clone(),
            asset_id: asset.id,
            width: asset.original.width,
            height: asset.original.height,
            inference_byte_count: asset.derivative.byte_size,
            inference_content_sha256: asset.derivative.content_digest,
            inference_width: asset.derivative.width,
            inference_height: asset.derivative.height,
            provenance,
        })
    }
}

fn validate_provenance(
    asset: &AssetReference,
    provenance: &RemoteAssetProvenance,
) -> Result<(), AppCoreError> {
    if asset.original.byte_size != provenance.byte_count
        || asset.original.content_digest != provenance.content_sha256
        || asset.original.media_type != provenance.media_type.as_str()
    {
        return Err(asset_store::AssetStoreError::InvalidMedia.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, io::Cursor};

    use app_storage::Project;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use ipc_types::{
        Conversation, ConversationId, Message, MessageId, MessagePart, MessageRole, ToolCallId,
        ToolOutcome, ToolResult,
    };
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;

    const NOW: &str = "2026-08-22T12:00:00Z";

    fn png() -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
            .write_to(&mut bytes, ImageFormat::Png)?;
        Ok(bytes.into_inner())
    }

    fn provenance(bytes: &[u8]) -> RemoteAssetProvenance {
        RemoteAssetProvenance {
            source_origin: "https://images.example".to_owned(),
            media_type: ValidatedWebMediaType::Png,
            byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            content_sha256: format!("{:x}", Sha256::digest(bytes)),
            retrieved_at_unix_millis: 1,
            elapsed_millis: 2,
        }
    }

    fn project(app: &AppCore) -> Result<ProjectId, AppCoreError> {
        let project_id = ProjectId("project-web-assets".to_owned());
        app.with_storage(|storage| {
            storage.save_project(&Project {
                id: project_id.clone(),
                name: "Web assets".to_owned(),
                created_at: NOW.to_owned(),
                updated_at: NOW.to_owned(),
            })
        })?;
        Ok(project_id)
    }

    #[test]
    fn remote_image_uses_managed_normalization_and_safe_receipt() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let app = AppCore::initialize(directory.path().join("Zeta"))?;
        let project_id = project(&app)?;
        let bytes = png()?;
        let receipt = app.persist_remote_image(&project_id, &bytes, NOW, provenance(&bytes))?;

        assert_eq!(receipt.project_id(), &project_id);
        assert_eq!((receipt.width(), receipt.height()), (8, 6));
        assert_eq!(
            receipt.provenance().source_origin(),
            "https://images.example"
        );
        assert_eq!(receipt.provenance().byte_count(), bytes.len() as u64);
        assert!(
            app.inference_asset(receipt.asset_id())?
                .starts_with(b"\x89PNG")
        );

        let rendered = format!("{receipt:?}");
        assert!(!rendered.contains("storage_key"));
        assert!(!rendered.contains("token="));
        assert!(!rendered.contains("originals/"));
        assert!(!rendered.contains("inference/"));

        let parts = receipt.tool_result_parts();
        let serialized = serde_json::to_string(&parts)?;
        assert!(serialized.contains("remoteProvenance"));
        assert!(serialized.contains("https://images.example"));
        assert!(!serialized.contains("storage_key"));
        assert!(!serialized.contains("originals/"));
        assert!(!serialized.contains("inference/"));

        let conversation_id = ConversationId("conversation-remote-image".to_owned());
        app.save_conversation(&Conversation {
            id: conversation_id.clone(),
            project_id,
            title: "Remote image".to_owned(),
            messages: vec![Message {
                id: MessageId("message-remote-image".to_owned()),
                conversation_id: conversation_id.clone(),
                role: MessageRole::Tool,
                parts: vec![MessagePart::ToolResult {
                    result: ToolResult {
                        call_id: ToolCallId("call-remote-image".to_owned()),
                        outcome: ToolOutcome::Completed {
                            content: parts.into_iter().collect(),
                        },
                    },
                }],
                created_at: NOW.to_owned(),
            }],
            created_at: NOW.to_owned(),
            updated_at: NOW.to_owned(),
        })?;
        assert!(app.delete_conversation(&conversation_id)?);
        assert!(matches!(
            app.inference_asset(receipt.asset_id()),
            Err(AppCoreError::AssetNotFound)
        ));
        Ok(())
    }

    #[test]
    fn rejection_and_provenance_mismatch_leave_no_managed_asset() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let app = AppCore::initialize(directory.path().join("Zeta"))?;
        let project_id = project(&app)?;

        for rejected in [
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
            b"PK\x03\x04archive".as_slice(),
            b"MZexecutable".as_slice(),
            b"%PDF-1.7 active document".as_slice(),
        ] {
            assert!(
                app.persist_remote_image(&project_id, rejected, NOW, provenance(rejected))
                    .is_err()
            );
        }
        let bytes = png()?;
        let mut wrong = provenance(&bytes);
        wrong.content_sha256 = "0".repeat(64);
        assert!(
            app.persist_remote_image(&project_id, &bytes, NOW, wrong)
                .is_err()
        );
        assert!(app.with_storage(|storage| storage.assets())?.is_empty());
        for managed_directory in ["originals", "inference", "staging"] {
            assert!(
                fs::read_dir(directory.path().join("Zeta/assets").join(managed_directory))?
                    .next()
                    .is_none()
            );
        }
        Ok(())
    }

    #[test]
    fn remote_image_requires_an_existing_project_before_publication() -> Result<(), Box<dyn Error>>
    {
        let directory = TempDir::new()?;
        let app = AppCore::initialize(directory.path().join("Zeta"))?;
        let bytes = png()?;
        assert!(matches!(
            app.persist_remote_image(
                &ProjectId("missing".to_owned()),
                &bytes,
                NOW,
                provenance(&bytes)
            ),
            Err(AppCoreError::ProjectNotFound)
        ));
        assert!(app.with_storage(|storage| storage.assets())?.is_empty());
        Ok(())
    }

    #[test]
    fn startup_reconciliation_reaps_an_unattached_remote_asset() -> Result<(), Box<dyn Error>> {
        let directory = TempDir::new()?;
        let data_directory = directory.path().join("Zeta");
        let bytes = png()?;
        let asset_id = {
            let app = AppCore::initialize(&data_directory)?;
            let project_id = project(&app)?;
            app.persist_remote_image(&project_id, &bytes, NOW, provenance(&bytes))?
                .asset_id()
                .clone()
        };

        let reopened = AppCore::initialize(&data_directory)?;
        assert!(matches!(
            reopened.inference_asset(&asset_id),
            Err(AppCoreError::AssetNotFound)
        ));
        assert!(
            reopened
                .with_storage(|storage| storage.assets())?
                .is_empty()
        );
        Ok(())
    }
}
