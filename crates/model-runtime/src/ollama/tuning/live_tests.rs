//! Bounded opt-in check of real preflight/generation KV reuse for both startup policies.

use super::*;
use crate::{
    DownloadCancellation, GenerationEvent, GenerationMetrics, OllamaProvider, OllamaProviderConfig,
    OllamaSidecar, OllamaSidecarConfig, PromptContent, PromptMessage, PromptRenderRequest,
    QwenAdapter, QwenStreamDecoder, ReasoningStrength,
};
use std::{error::Error, path::Path};

#[test]
#[ignore = "requires the pinned Qwen bundle in an isolated store on an idle M1 Pro/32 GB"]
fn live_qwen_tuning_baseline_and_rollback_preserve_kv() -> Result<(), Box<dyn Error>> {
    let models = std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("isolated store required")?;
    let executable =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
    let guardian = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop");
    let catalogue = crate::bundled_catalogue()?;
    let model = catalogue
        .models()
        .iter()
        .find(|m| m.key().id() == "qwen/qwen3.8-27b")
        .ok_or("Qwen missing")?;
    for policy in [
        QwenTuningPolicy::M1ProBaselineV1,
        QwenTuningPolicy::Existing,
    ] {
        let sidecar = OllamaSidecar::new();
        let endpoint = sidecar.start(
            &OllamaSidecarConfig::new(&executable, &models).with_guardian_executable(&guardian),
        )?;
        let provider = OllamaProvider::new(
            OllamaProviderConfig::loopback(endpoint.address().into())?.with_qwen_tuning(policy),
        )?;
        let verified = provider.verify_model(model)?;
        let prompt = PromptRenderRequest {
            messages: vec![
                PromptMessage::System {
                    content: vec![PromptContent::Text(
                        "Stable synthetic prefix for runner reuse. ".repeat(96),
                    )],
                },
                PromptMessage::User {
                    content: vec![PromptContent::Text("Reply only with OK.".to_owned())],
                },
            ],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-09-08".to_owned(),
            reasoning_strength: ReasoningStrength::Low,
            add_generation_prompt: true,
        };
        let request = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 16)?;
        let cold = run(&provider, &request, "cold")?;
        let repeated = run(&provider, &request, "repeat")?;
        let cold_ns = cold.prompt_evaluation_ns.ok_or("cold timing missing")?;
        let repeated_ns = repeated
            .prompt_evaluation_ns
            .ok_or("repeat timing missing")?;
        eprintln!(
            "{}: cold={cold:?}, repeated={repeated:?}",
            verified.runtime_profile(32_768).id()
        );
        assert!(
            repeated_ns.saturating_mul(2) < cold_ns,
            "preflight discarded reusable KV state"
        );
        sidecar.stop()?;
    }
    Ok(())
}

fn run(
    provider: &OllamaProvider,
    request: &OllamaChatRequest,
    id: &str,
) -> Result<GenerationMetrics, Box<dyn Error>> {
    let _ = provider.render_chat_prompt(request)?;
    let mut metrics = None;
    let mut completed = false;
    let mut failure = None;
    provider.stream_chat_events(
        request,
        &DownloadCancellation::new(),
        &mut QwenStreamDecoder::new(id)?,
        |event| match event {
            GenerationEvent::Metrics { metrics: value } => metrics = Some(value),
            GenerationEvent::Completed { .. } => completed = true,
            GenerationEvent::Failed { failure: value } => failure = Some(value),
            _ => {}
        },
    )?;
    eprintln!("{id}: metrics={metrics:?}, failure={failure:?}");
    assert!(completed, "{id} failed: {failure:?}");
    metrics.ok_or_else(|| "metrics missing".into())
}
