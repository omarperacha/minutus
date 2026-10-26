//! Raw-GGUF Glimmer manifests without Ollama's copying/rewrite/create API.
use super::ollama::{private_directory, write_metadata};
use crate::{ExternalModelError, ModelManifest};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

const CONFIG: &[u8] = b"{\"model_format\":\"gguf\",\"model_family\":\"muse-glimmer\",\"model_families\":[\"muse-glimmer\"],\"model_type\":\"27.9B\",\"file_type\":\"Q4_K_M\",\"parser\":\"glimmer\",\"architecture\":\"amd64\",\"os\":\"linux\"}\n";
const TEMPLATE: &[u8] = b"{{ .Prompt }}";

/// Writes small local manifest/configuration/template blobs referencing raw weights.
///
/// # Errors
/// Rejects unqualified profiles, absent projector identities and store failures.
pub fn register_external_glimmer_metadata(
    store: &Path,
    model: &ModelManifest,
    with_projector: bool,
) -> Result<(), ExternalModelError> {
    if model.key().id() != "meta-models/muse-glimmer-30b"
        || model.artifact().sha256()
            != "4cc57c0f51040a226e5a72cc47b7613f7772950e460a665f7083de89f183f60e"
    {
        return Err(ExternalModelError::Unsupported);
    }
    let blobs = store.join("blobs");
    private_directory(&blobs)?;
    let config_digest = format!("{:x}", Sha256::digest(CONFIG));
    let template_digest = format!("{:x}", Sha256::digest(TEMPLATE));
    write_metadata(
        &blobs.join(format!("sha256-{config_digest}")),
        CONFIG,
        &config_digest,
    )?;
    write_metadata(
        &blobs.join(format!("sha256-{template_digest}")),
        TEMPLATE,
        &template_digest,
    )?;
    let mut layers = vec![
        json!({ "mediaType": "application/vnd.ollama.image.model", "digest": format!("sha256:{}", model.artifact().sha256()), "size": model.artifact().installed_bytes() }),
    ];
    if with_projector {
        let projector = model
            .multimodal_profile()
            .and_then(crate::MultimodalProfile::projector_artifact_id)
            .and_then(|id| model.companion(id))
            .ok_or(ExternalModelError::Unsupported)?;
        layers.push(json!({ "mediaType": "application/vnd.ollama.image.projector", "digest": format!("sha256:{}",projector.artifact().sha256()), "size": projector.artifact().installed_bytes() }));
    }
    layers.push(json!({ "mediaType": "application/vnd.ollama.image.template", "digest": format!("sha256:{template_digest}"), "size": TEMPLATE.len() }));
    let manifest = serde_json::to_vec(&json!({ "schemaVersion": 2, "mediaType": "application/vnd.docker.distribution.manifest.v2+json", "config": { "mediaType": "application/vnd.docker.container.image.v1+json", "digest": format!("sha256:{config_digest}"), "size": CONFIG.len() }, "layers": layers })).map_err(|_| ExternalModelError::Unsupported)?;
    let directory = store.join("manifests/registry.ollama.ai/zeta/muse-glimmer-30b");
    private_directory(&directory)?;
    write_metadata(
        &directory.join("4cc57c0f5104-v2"),
        &manifest,
        &format!("{:x}", Sha256::digest(&manifest)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_manifests_reference_original_digest_and_selected_projector_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = root.path().canonicalize()?;
        let catalogue = crate::bundled_catalogue()?;
        let model = catalogue
            .models()
            .iter()
            .find(|model| model.key().id() == "meta-models/muse-glimmer-30b")
            .ok_or("missing Glimmer")?;
        let path = store.join("manifests/registry.ollama.ai/zeta/muse-glimmer-30b/4cc57c0f5104-v2");
        for (projector, layer_count) in [(false, 2), (true, 3)] {
            register_external_glimmer_metadata(&store, model, projector)?;
            let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let layers = manifest["layers"].as_array().ok_or("missing layers")?;
            assert_eq!(layers.len(), layer_count);
            assert_eq!(
                layers[0]["digest"],
                format!("sha256:{}", model.artifact().sha256())
            );
            assert!(
                !store
                    .join("blobs")
                    .join(format!("sha256-{}", model.artifact().sha256()))
                    .exists()
            );
            // Only configuration and template files are created, never weight blobs.
            assert_eq!(std::fs::read_dir(store.join("blobs"))?.count(), 2);
        }
        Ok(())
    }
}
