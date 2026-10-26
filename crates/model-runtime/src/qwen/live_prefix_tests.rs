//! Opt-in regression against the pinned provider renderer, not a template mock.

use super::*;
use crate::{
    DownloadCancellation, GenerationEvent, GenerationMetrics, OllamaError, OllamaProvider,
    OllamaProviderConfig, OllamaSidecar, OllamaSidecarConfig, ReasoningStrength,
};
use std::{error::Error, path::Path};

#[test]
#[ignore = "requires an isolated store containing the pinned Qwen bundle"]
fn live_qwen_routine_reasoning_transitions_preserve_rendered_prefix_and_kv()
-> Result<(), Box<dyn Error>> {
    let models =
        std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("ZETA_LIVE_OLLAMA_MODELS is required")?;
    let executable =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
    let sidecar = OllamaSidecar::new();
    let endpoint = sidecar.start(
        &OllamaSidecarConfig::new(executable, models).with_guardian_executable(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
        ),
    )?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(endpoint.address().into())?)?;
    let catalogue = crate::bundled_catalogue()?;
    let profile = catalogue
        .models()
        .iter()
        .find(|model| model.backend() == crate::InferenceBackend::Ollama)
        .ok_or("Qwen profile missing")?;
    let verified = provider.verify_model(profile)?;
    let mut prompt = PromptRenderRequest {
        messages: vec![
            PromptMessage::System {
                content: vec![PromptContent::Text(
                    "Stable provider-prefix probe. ".repeat(512),
                )],
            },
            PromptMessage::User {
                content: vec![PromptContent::Text(
                    "Reply with exactly OK; no tool is needed.".to_owned(),
                )],
            },
        ],
        tools: vec![crate::PromptToolDefinition {
            name: "cache_probe".to_owned(),
            description: "Return the supplied value.".to_owned(),
            parameters: serde_json::json!({"type":"object", "properties":{"value":{"type":"string"}}, "required":["value"], "additionalProperties":false}),
        }],
        historical_tool_names: Vec::new(),
        current_date: "2026-09-02".to_owned(),
        reasoning_strength: ReasoningStrength::Disabled,
        add_generation_prompt: true,
    };
    let low = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 32)?;
    prompt.reasoning_strength = ReasoningStrength::Medium;
    let medium = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 32)?;
    prompt.reasoning_strength = ReasoningStrength::High;
    let high = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 32)?;
    let low_text = provider.render_chat_prompt(&low)?;
    let medium_text = provider.render_chat_prompt(&medium)?;
    let high_text = provider.render_chat_prompt(&high)?;
    let end_of_history = low_text
        .rfind("<|im_start|>assistant\n")
        .ok_or("assistant generation prefix missing")?;
    assert!(end_of_history > 8_000);
    assert!(
        medium_text.starts_with(&low_text[..end_of_history]),
        "Routine thinking transitions changed retained history or the leading tool/system prefix"
    );
    // The pinned renderer prepends xhigh instructions only for high/max. This
    // documents why a transport-only reinspection must not implicitly select it.
    assert!(high_text.starts_with("<|im_start|>system\nReasoning effort is set to xhigh."));
    assert!(!low_text.starts_with("<|im_start|>system\nReasoning effort is set"));

    let cold = probe(&provider, &low, "low-cold")?;
    let warm = probe(&provider, &medium, "medium-warm")?;
    let repeated = probe(&provider, &medium, "medium-reinspection")?;
    let returned = probe(&provider, &low, "low-return")?;
    let ns = |metrics: &GenerationMetrics| {
        metrics
            .prompt_evaluation_ns
            .ok_or("prompt duration missing")
    };
    let cold_ns = ns(&cold)?;
    for metrics in [&warm, &repeated, &returned] {
        assert!(
            ns(metrics)?.saturating_mul(2) < cold_ns,
            "Routine profile transition did not materially reuse the resident prompt"
        );
    }
    eprintln!(
        "prompt_eval_ns: cold={cold_ns}, medium={}, repeat={}, low_return={}; prompt_tokens={:?}; provider_cached_tokens={:?}",
        ns(&warm)?,
        ns(&repeated)?,
        ns(&returned)?,
        [
            cold.prompt_tokens,
            warm.prompt_tokens,
            repeated.prompt_tokens,
            returned.prompt_tokens,
        ],
        [
            cold.cached_prompt_tokens,
            warm.cached_prompt_tokens,
            repeated.cached_prompt_tokens,
            returned.cached_prompt_tokens,
        ]
    );
    sidecar.stop()?;
    Ok(())
}

fn probe(
    provider: &OllamaProvider,
    request: &OllamaChatRequest,
    id: &str,
) -> Result<GenerationMetrics, Box<dyn Error>> {
    // Admission and inference deliberately share every runner-affecting option.
    let _ = provider.render_chat_prompt(request)?;
    let mut decoder = QwenStreamDecoder::new(id)?;
    let mut metrics = None;
    let mut terminal = None;
    provider.stream_chat_events(
        request,
        &DownloadCancellation::new(),
        &mut decoder,
        |event| match event {
            GenerationEvent::Metrics { metrics: value } => metrics = Some(value),
            GenerationEvent::Failed { .. }
            | GenerationEvent::Cancelled
            | GenerationEvent::Completed { .. } => terminal = Some(format!("{event:?}")),
            _ => {}
        },
    )?;
    metrics.ok_or_else(|| format!("{id}: provider metrics missing; terminal={terminal:?}").into())
}

#[test]
#[ignore = "requires an isolated store containing the pinned Qwen bundle"]
fn live_qwen_cancelled_stream_then_retry_reports_prefix_timing() -> Result<(), Box<dyn Error>> {
    let models =
        std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("ZETA_LIVE_OLLAMA_MODELS is required")?;
    let executable =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
    let sidecar = OllamaSidecar::new();
    let endpoint = sidecar.start(
        &OllamaSidecarConfig::new(executable, models).with_guardian_executable(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
        ),
    )?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(endpoint.address().into())?)?;
    let catalogue = crate::bundled_catalogue()?;
    let profile = catalogue
        .models()
        .iter()
        .find(|model| model.backend() == crate::InferenceBackend::Ollama)
        .ok_or("Qwen profile missing")?;
    let verified = provider.verify_model(profile)?;
    let prompt = PromptRenderRequest {
        messages: vec![
            PromptMessage::System {
                content: vec![PromptContent::Text(
                    "Stable interrupted-stream prefix probe. ".repeat(512),
                )],
            },
            PromptMessage::User {
                content: vec![PromptContent::Text(
                    "List 100 prime numbers, one per line.".to_owned(),
                )],
            },
        ],
        tools: Vec::new(),
        historical_tool_names: Vec::new(),
        current_date: "2026-10-01".to_owned(),
        reasoning_strength: ReasoningStrength::Medium,
        add_generation_prompt: true,
    };
    let request = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 256)?;
    let cold = probe(&provider, &request, "cancel-probe-cold")?
        .prompt_evaluation_ns
        .ok_or("cold prompt timing missing")?;
    let cancellation = DownloadCancellation::new();
    let trigger = cancellation.clone();
    let mut decoder = QwenStreamDecoder::new("cancel-probe-interrupted")?;
    let mut saw_delta = false;
    let interrupted = provider.stream_chat_events(&request, &cancellation, &mut decoder, |event| {
        if matches!(
            event,
            GenerationEvent::ReasoningDelta { .. } | GenerationEvent::ContentPreviewDelta { .. }
        ) {
            saw_delta = true;
            trigger.cancel();
        }
    });
    assert!(
        saw_delta,
        "the interrupted request never produced model text"
    );
    assert!(matches!(interrupted, Err(OllamaError::Cancelled)));
    let retry = probe(&provider, &request, "cancel-probe-retry")?
        .prompt_evaluation_ns
        .ok_or("retry prompt timing missing")?;
    eprintln!("cancelled-stream prompt_eval_ns: cold={cold}, retry={retry}");
    sidecar.stop()?;
    Ok(())
}

/// Uses real normalized pixels and the production renderer. This measures
/// multimodal continuation cost, not transcription accuracy or a full agent run.
#[test]
#[ignore = "requires isolated pinned Qwen store and ZETA_LIVE_IMAGE"]
fn live_qwen_image_history_survives_control_and_search_continuations() -> Result<(), Box<dyn Error>>
{
    let pixels =
        std::fs::read(std::env::var_os("ZETA_LIVE_IMAGE").ok_or("missing normalized image")?)?;
    control_continuation(&[pixels])
}

#[test]
#[ignore = "requires an isolated store containing the pinned Qwen bundle"]
fn live_qwen_text_history_survives_control_and_search_continuations() -> Result<(), Box<dyn Error>>
{
    control_continuation(&[])
}

fn control_continuation(pixels: &[Vec<u8>]) -> Result<(), Box<dyn Error>> {
    let models =
        std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("missing isolated model store")?;
    let sidecar = OllamaSidecar::new();
    let executable =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ollama-runtime/ollama");
    let endpoint = sidecar.start(
        &OllamaSidecarConfig::new(executable, models).with_guardian_executable(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeta-desktop"),
        ),
    )?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(endpoint.address().into())?)?;
    let catalogue = crate::bundled_catalogue()?;
    let profile = catalogue
        .models()
        .iter()
        .find(|model| model.backend() == crate::InferenceBackend::Ollama)
        .ok_or("Qwen profile missing")?;
    let verified = provider.verify_model(profile)?;
    let names = [
        "filesystem.inspect_image",
        "filesystem.read_text",
        "orchestration.complete_stage",
        "orchestration.search_tools",
        "filesystem.write_delimited_table",
    ];
    let mut user_content = vec![PromptContent::Text(
        "This is a cache transport test. Reply only OK, without tools or transcribing the image."
            .to_owned(),
    )];
    user_content.extend(pixels.iter().map(|_| PromptContent::Image));
    let mut prompt = PromptRenderRequest {
        messages: vec![
            PromptMessage::System { content: vec![PromptContent::Text("Stable image-continuation probe. ".repeat(512))] },
            PromptMessage::User { content: user_content },
        ],
        tools: names.iter().map(|name| crate::PromptToolDefinition {
            name: (*name).to_owned(), description: "Transport probe; do not invoke.".to_owned(),
            parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        }).collect(),
        historical_tool_names: Vec::new(), current_date: "2026-09-03".to_owned(),
        reasoning_strength: ReasoningStrength::Medium, add_generation_prompt: true,
    };
    let mut timings = Vec::new();
    let mut previous_rendered: Option<String> = None;
    for (index, operation) in names.iter().enumerate() {
        if index > 0 {
            let call_id = format!("probe-{index}");
            prompt.messages.push(PromptMessage::Assistant {
                reasoning: None,
                content: Vec::new(),
                tool_calls: vec![PromptToolCall {
                    call_id: call_id.clone(),
                    name: (*operation).to_owned(),
                    arguments: BTreeMap::new(),
                }],
            });
            prompt.messages.push(PromptMessage::Tool {
                call_id,
                content: vec![PromptContent::Text(
                    "Transport-only fixture result. Reply only OK.".to_owned(),
                )],
            });
            prompt.historical_tool_names.push((*operation).to_owned());
        }
        if index == names.len() - 1 {
            prompt.reasoning_strength = ReasoningStrength::Disabled;
        }
        let request = QwenAdapter.render_with_context(&verified, &prompt, pixels, 32_768, 64)?;
        let rendered = provider.render_chat_prompt(&request)?;
        if let Some(previous) = &previous_rendered {
            let boundary = previous
                .rfind("<|im_start|>assistant\n")
                .ok_or("missing assistant boundary")?;
            assert!(
                rendered.starts_with(&previous[..boundary]),
                "image history prefix moved"
            );
        }
        previous_rendered = Some(rendered);
        let metrics = probe(&provider, &request, &format!("image-probe-{index}"))?;
        timings.push(
            metrics
                .prompt_evaluation_ns
                .ok_or("prompt timing missing")?,
        );
    }
    eprintln!(
        "continuation images={} prompt_eval_ns: {timings:?}",
        pixels.len()
    );
    sidecar.stop()?;
    for warm in &timings[1..] {
        assert!(
            warm.saturating_mul(2) < timings[0],
            "multimodal continuation failed to materially reuse the prefix"
        );
    }
    Ok(())
}
