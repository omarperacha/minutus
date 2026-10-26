use serde_json::{Value, json};

use super::*;

fn valid_document() -> Result<Value, serde_json::Error> {
    serde_json::from_str(include_str!("../../catalogue/v4.json"))
}

fn parse_value(value: &Value) -> Result<ModelCatalogue, CatalogueError> {
    ModelCatalogue::parse_json(&value.to_string())
}

#[test]
fn bundled_glimmer_release_has_pinned_values() -> Result<(), Box<dyn Error>> {
    let catalogue = bundled_catalogue()?;
    assert_eq!(catalogue.schema_version(), CATALOGUE_SCHEMA_VERSION);
    assert_eq!(catalogue.models().len(), 3);
    let model = &catalogue.models()[0];
    assert_eq!(model.key().id(), "meta-models/muse-glimmer-30b");
    assert_eq!(model.recommended_memory_bytes(), 24 * 1024 * 1024 * 1024);
    assert_eq!(model.defaults().context_tokens(), 32_768);
    assert_eq!(model.defaults().output_tokens(), 4_096);
    assert!(model.is_integrated());
    assert_eq!(model.capabilities().context_limit_tokens(), 131_072);
    assert!(model.capabilities().native_image_input());
    assert!(model.capabilities().structured_tool_calls());
    assert!(model.capabilities().separate_reasoning_output());
    assert_eq!(model.backend(), InferenceBackend::LlamaCpp);
    assert_eq!(model.artifact().download_bytes(), 16_756_683_904);
    assert_eq!(model.artifact().installed_bytes(), 16_756_683_904);
    assert_eq!(
        model.artifact().sha256(),
        "4cc57c0f51040a226e5a72cc47b7613f7772950e460a665f7083de89f183f60e"
    );
    assert_eq!(catalogue.model(model.key()), Some(model));
    assert_eq!(
        catalogue.artifact(model.artifact().key()),
        Some(model.artifact())
    );
    assert_eq!(model.companions().len(), 2);
    let projector = &model.companions()[0];
    assert_eq!(projector.kind(), CompanionKind::VisionProjector);
    assert!(!projector.required());
    assert_eq!(projector.artifact().installed_bytes(), 1_400_328_928);
    assert_eq!(
        projector.artifact().sha256(),
        "f48b452316f9b213758e8659444029b961a24a07f99a1abb2a9f88b06f7c00c6"
    );
    assert_eq!(
        projector.compatibility_version(),
        model.companion_compatibility_version()
    );
    assert_eq!(projector.memory_requirement_bytes(), 2 * 1024 * 1024 * 1024);
    assert_eq!(model.companions()[1].kind(), CompanionKind::DraftModel);
    let multimodal = model
        .multimodal_profile()
        .ok_or_else(|| std::io::Error::other("missing multimodal profile"))?;
    assert_eq!(
        multimodal.projector_artifact_id(),
        Some(projector.artifact().key().id())
    );
    assert_eq!(multimodal.image_tokens(), 1_024);
    assert_eq!(multimodal.max_images(), 4);
    Ok(())
}

#[test]
fn bundled_qwen_release_has_pinned_provider_identity() -> Result<(), Box<dyn Error>> {
    let catalogue = bundled_catalogue()?;
    let model = &catalogue.models()[1];
    assert_eq!(model.key().id(), "qwen/qwen3.8-27b");
    assert_eq!(
        model.key().version(),
        "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643"
    );
    assert_eq!(
        model.integration_status(),
        ModelIntegrationStatus::Integrated
    );
    assert!(model.is_integrated());
    assert!(model.is_manageable());
    assert_eq!(model.backend(), InferenceBackend::Ollama);
    assert_eq!(model.backend_compatibility_version(), "0.32.12");
    assert_eq!(model.recommended_memory_bytes(), 30 * 1024 * 1024 * 1024);
    assert_eq!(model.defaults().context_tokens(), 32_768);
    assert_eq!(model.defaults().output_tokens(), 8_192);
    assert_eq!(model.capabilities().context_limit_tokens(), 262_144);
    assert!(model.capabilities().native_text_input());
    assert!(model.capabilities().native_image_input());
    assert!(model.capabilities().structured_tool_calls());
    assert!(model.capabilities().separate_reasoning_output());
    assert_eq!(model.artifact().installed_bytes(), 16_810_714_464);
    assert_eq!(
        model.artifact().sha256(),
        "f5f1dd8920d417aac2718b0bda3403da274301efdd6760b4f0f4b864ff2ad57d"
    );
    assert_eq!(model.companions().len(), 1);
    let multimodal = model
        .multimodal_profile()
        .ok_or_else(|| std::io::Error::other("missing Qwen multimodal admission profile"))?;
    assert_eq!(multimodal.image_tokens(), 4_096);
    assert_eq!(multimodal.max_images(), 4);
    assert!(model.companions()[0].required());
    assert_eq!(model.companions()[0].kind(), CompanionKind::VisionProjector);
    assert_eq!(
        model.companions()[0].artifact().installed_bytes(),
        931_146_016
    );
    let ollama = model
        .ollama_distribution()
        .ok_or_else(|| std::io::Error::other("missing Ollama distribution"))?;
    assert_eq!(ollama.tag(), "qwen3.8:27b");
    assert_eq!(ollama.renderer(), "qwen3.8");
    assert_eq!(ollama.parser(), "qwen3.5");
    assert_eq!(ollama.config_bytes(), 215);
    assert_eq!(ollama.parameters_bytes(), 114);
    assert_eq!(ollama.license_bytes(), 11_345);
    Ok(())
}

#[test]
fn malformed_missing_unknown_and_future_schemas_fail_closed() -> Result<(), Box<dyn Error>> {
    assert!(matches!(
        ModelCatalogue::parse_json("{"),
        Err(CatalogueError::Json(_))
    ));
    let mut missing = valid_document()?;
    missing["models"][0]
        .as_object_mut()
        .map(|model| model.remove("artifact"));
    assert!(matches!(
        parse_value(&missing),
        Err(CatalogueError::Json(_))
    ));
    let mut unknown = valid_document()?;
    unknown["models"][0]["surprise"] = json!(true);
    assert!(matches!(
        parse_value(&unknown),
        Err(CatalogueError::Json(_))
    ));
    let mut future = valid_document()?;
    future["schemaVersion"] = json!(CATALOGUE_SCHEMA_VERSION + 1);
    assert!(matches!(
        parse_value(&future),
        Err(CatalogueError::UnsupportedSchema { .. })
    ));
    Ok(())
}

#[test]
fn companion_compatibility_memory_and_roles_fail_closed() -> Result<(), Box<dyn Error>> {
    for (pointer, value) in [
        (
            "/models/0/companions/0/compatibilityVersion",
            json!("other"),
        ),
        ("/models/0/companions/0/memoryRequirementBytes", json!(0)),
        ("/models/0/companions/1/kind", json!("visionProjector")),
        (
            "/models/0/multimodalProfile/projectorArtifactId",
            json!("draft-model-q4-k-m"),
        ),
        ("/models/0/multimodalProfile/imageTokens", json!(0)),
        ("/models/0/multimodalProfile/maxImages", json!(5)),
    ] {
        let mut invalid = valid_document()?;
        *invalid
            .pointer_mut(pointer)
            .ok_or_else(|| std::io::Error::other("test JSON pointer is missing"))? = value;
        assert!(matches!(
            parse_value(&invalid),
            Err(CatalogueError::InvalidField { .. })
        ));
    }
    Ok(())
}

#[test]
fn duplicate_model_releases_fail_closed() -> Result<(), Box<dyn Error>> {
    let mut duplicate = valid_document()?;
    let model = duplicate["models"][0].clone();
    if let Some(models) = duplicate["models"].as_array_mut() {
        models.push(model);
    }
    assert!(matches!(
        parse_value(&duplicate),
        Err(CatalogueError::DuplicateModel(_))
    ));
    Ok(())
}

#[test]
fn unsafe_urls_paths_checksums_sizes_and_limits_fail_closed() -> Result<(), Box<dyn Error>> {
    for (pointer, value) in [
        (
            "/models/0/artifact/downloadUrl",
            json!("http://example.test/model.gguf"),
        ),
        ("/models/0/artifact/fileName", json!("../model.gguf")),
        ("/models/0/artifact/sha256", json!("ABC")),
        ("/models/0/artifact/downloadBytes", json!(0)),
        ("/models/0/recommendedMemoryBytes", json!(1)),
        ("/models/0/defaults/contextTokens", json!(0)),
        ("/models/0/defaults/outputTokens", json!(99_999)),
        (
            "/models/0/licenseUrl",
            json!("https://user:pass@example.test/license"),
        ),
    ] {
        let mut invalid = valid_document()?;
        let target = invalid
            .pointer_mut(pointer)
            .ok_or_else(|| std::io::Error::other("test JSON pointer is missing"))?;
        *target = value;
        assert!(
            matches!(
                parse_value(&invalid),
                Err(CatalogueError::InvalidField { .. })
            ),
            "pointer {pointer} unexpectedly passed"
        );
    }
    Ok(())
}
