use std::error::Error;

use crate::{ArtifactFormat, bundled_catalogue};

use super::*;

#[test]
fn gguf_is_recommended_and_mlx_requires_an_apple_silicon_experiment() -> Result<(), Box<dyn Error>>
{
    let catalogue = bundled_catalogue()?;
    let mlx = catalogue
        .models()
        .iter()
        .find(|m| m.is_mlx())
        .ok_or("missing MLX")?;
    let gguf = catalogue
        .models()
        .iter()
        .find(|m| m.key().id() == "qwen/qwen3.8-27b")
        .ok_or("missing GGUF")?;
    assert!(!mlx.is_recommended(ModelPlatform::AppleSilicon));
    assert!(gguf.is_recommended(ModelPlatform::AppleSilicon));
    for platform in [ModelPlatform::AppleSilicon, ModelPlatform::Other] {
        assert!(gguf.supports_runtime(platform, false));
        assert!(gguf.supports_runtime(platform, true));
        assert!(!mlx.supports_runtime(platform, false));
    }
    assert!(mlx.supports_runtime(ModelPlatform::AppleSilicon, true));
    assert!(!mlx.supports_runtime(ModelPlatform::Other, true));
    assert!(!mlx.supports_platform(ModelPlatform::Other));
    assert!(!mlx.is_recommended(ModelPlatform::Other));
    assert!(gguf.is_recommended(ModelPlatform::Other));
    assert_eq!(mlx.artifact().format(), ArtifactFormat::Safetensors);
    let distribution = mlx.ollama_distribution().ok_or("missing distribution")?;
    assert_eq!(distribution.tensor_bundle_bytes(), Some(18_174_721_847));
    assert_eq!(
        distribution.tensor_blobs().ok_or("missing blobs")?.len(),
        1210
    );
    assert_eq!(
        distribution
            .tokenizer_blob()
            .ok_or("missing tokenizer")?
            .bytes(),
        12_809_320
    );
    assert!(
        mlx.multimodal_profile()
            .ok_or("missing vision")?
            .projector_artifact_id()
            .is_none()
    );
    assert!(mlx.companions().is_empty());
    Ok(())
}

#[test]
fn mutated_tensor_manifest_and_gguf_tensor_mismatch_are_rejected() -> Result<(), Box<dyn Error>> {
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../../../catalogue/v4.json"))?;
    document["models"][2]["backend"]["ollama"]["tensorBundleManifest"] = serde_json::json!("{}");
    assert!(crate::ModelCatalogue::parse_json(&document.to_string()).is_err());
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../../../catalogue/v4.json"))?;
    document["models"][2]["artifact"]["format"] = serde_json::json!("gguf");
    assert!(crate::ModelCatalogue::parse_json(&document.to_string()).is_err());
    Ok(())
}
