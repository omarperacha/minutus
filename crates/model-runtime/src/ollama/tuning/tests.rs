use super::*;
use crate::{
    GenerationBinding, PromptContent, PromptMessage, PromptRenderRequest, QwenAdapter,
    ReasoningStrength, bundled_catalogue,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn model() -> VerifiedOllamaModel {
    let mut model = VerifiedOllamaModel::pinned_for_adapter("qwen3.8:27b", QWEN_MANIFEST);
    model.tuning_policy = QwenTuningPolicy::M1ProBaselineV1;
    model
}

#[test]
fn qualification_is_exact_and_rollback_retains_manifest_settings() {
    let baseline = model();
    assert!(baseline.runtime_profile(32_768).is_measured_baseline());
    assert_eq!(
        baseline.runtime_profile(32_768).parallel_contexts(),
        Some(1)
    );
    assert_eq!(
        baseline.runtime_profile(32_768).manifest_sha256(),
        QWEN_MANIFEST
    );
    assert!(!baseline.runtime_profile(65_536).is_measured_baseline());
    for changed in ["tag", "manifest", "version", "policy"] {
        let mut model = baseline.clone();
        match changed {
            "tag" => model.tag = "other:27b".to_owned(),
            "manifest" => model.manifest_sha256 = "a".repeat(64),
            "version" => model.provider_version = "0.32.15".to_owned(),
            _ => model.tuning_policy = QwenTuningPolicy::Existing,
        }
        assert!(!model.runtime_profile(32_768).is_measured_baseline());
    }
}

#[test]
fn profile_changes_invalidate_prepared_bindings_and_derived_cache_identity() -> TestResult {
    let catalogue = bundled_catalogue()?;
    let manifest = catalogue
        .models()
        .iter()
        .find(|m| m.key().id() == "qwen/qwen3.8-27b")
        .ok_or("missing Qwen")?;
    let binding = GenerationBinding::qwen_ollama(
        manifest.key().id(),
        manifest.key().version(),
        manifest.artifact().sha256(),
        crate::PINNED_OLLAMA_VERSION,
        manifest.capabilities().clone(),
    );
    let baseline = model();
    let expected = binding
        .clone()
        .with_qwen_runtime_profile(baseline.runtime_profile(32_768));
    let mut rollback = baseline.clone();
    rollback.tuning_policy = QwenTuningPolicy::Existing;
    let mut projector_changed = baseline.clone();
    projector_changed.manifest_sha256 = "b".repeat(64);
    for profile in [
        rollback.runtime_profile(32_768),
        baseline.runtime_profile(16_384),
        projector_changed.runtime_profile(32_768),
    ] {
        let changed = binding.clone().with_qwen_runtime_profile(profile);
        assert_ne!(expected, changed);
        assert_ne!(expected.identity_sha256(), changed.identity_sha256());
    }
    assert_eq!(
        expected,
        binding.with_qwen_runtime_profile(baseline.runtime_profile(32_768))
    );
    Ok(())
}

#[test]
fn ordinary_reasoning_and_image_turns_keep_runner_options_and_rollback_omits_override() -> TestResult
{
    let baseline = model();
    let mut rollback = baseline.clone();
    rollback.tuning_policy = QwenTuningPolicy::Existing;
    for model in [&baseline, &rollback] {
        for reasoning in [
            ReasoningStrength::Disabled,
            ReasoningStrength::Low,
            ReasoningStrength::Medium,
            ReasoningStrength::High,
        ] {
            for with_image in [false, true] {
                let mut content = vec![PromptContent::Text("Synthetic profile probe".to_owned())];
                let mut images = Vec::new();
                if with_image {
                    content.push(PromptContent::Image);
                    images.push(b"\x89PNG\r\n\x1a\n".to_vec());
                }
                let prompt = PromptRenderRequest {
                    messages: vec![PromptMessage::User { content }],
                    tools: Vec::new(),
                    historical_tool_names: Vec::new(),
                    current_date: "2026-09-08".to_owned(),
                    reasoning_strength: reasoning,
                    add_generation_prompt: true,
                };
                let generation =
                    QwenAdapter.render_with_context(model, &prompt, &images, 32_768, 64)?;
                let render = generation.render_only();
                let mut a = serde_json::to_value(generation)?;
                let mut b = serde_json::to_value(render)?;
                assert_eq!(a["options"]["num_ctx"], 32_768);
                assert_eq!(a["options"]["num_predict"], 64);
                assert_eq!(a["shift"], true);
                assert_eq!(a["truncate"], false);
                let disabled = reasoning == ReasoningStrength::Disabled;
                assert_eq!(
                    a["options"]["temperature"],
                    if disabled { 0.7 } else { 1.0 }
                );
                assert_eq!(a["options"]["top_p"], if disabled { 0.8 } else { 0.95 });
                assert_eq!(
                    a["options"]["presence_penalty"],
                    if disabled { 1.5 } else { 0.0 }
                );
                assert_eq!(a["options"]["top_k"], 20);
                assert_eq!(a["options"]["min_p"], 0.0);
                assert_eq!(a["options"]["repeat_penalty"], 1.0);
                if model.runtime_profile(32_768).is_measured_baseline() {
                    assert_eq!(a["options"]["draft_num_predict"], 4);
                } else {
                    assert!(a["options"].get("draft_num_predict").is_none());
                }
                assert_eq!(b["_debug_render_only"], true);
                a.as_object_mut().ok_or("object missing")?.remove("stream");
                let object = b.as_object_mut().ok_or("object missing")?;
                object.remove("stream");
                object.remove("_debug_render_only");
                assert_eq!(a, b);
            }
        }
    }
    Ok(())
}
