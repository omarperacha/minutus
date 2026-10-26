//! Opt-in application transport and tokenizer qualification for the pinned MLX bundle.

use super::*;
use crate::{
    DownloadCancellation, ExactTokenizer, GenerationEvent, OllamaProvider, OllamaProviderConfig,
    OllamaSidecar, OllamaSidecarConfig, ReasoningStrength,
};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

#[test]
#[ignore = "requires the pinned MLX bundle and native Apple Silicon runtime"]
fn live_mlx_counts_rendered_prompt_and_streams_structured_tool_call() -> Result<(), Box<dyn Error>>
{
    let models =
        PathBuf::from(std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("missing model store")?);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sidecar = OllamaSidecar::new();
    let endpoint = sidecar.start(&OllamaSidecarConfig::new(
        root.join("src-tauri/ollama-runtime/ollama"),
        &models,
    ))?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(endpoint.address().into())?)?;
    let catalogue = crate::bundled_catalogue()?;
    let model = catalogue
        .models()
        .iter()
        .find(|m| m.is_mlx())
        .ok_or("missing MLX")?;
    let verified = provider.verify_model(model)?;
    assert!(!verified.runtime_profile(32_768).is_measured_baseline());
    let blob = model
        .ollama_distribution()
        .and_then(|d| d.tokenizer_blob())
        .ok_or("missing tokenizer")?;
    let path = models
        .join("blobs")
        .join(format!("sha256-{}", blob.sha256()));
    let counter =
        ExactTokenizer::new(root.join("src-tauri/binaries/zeta-tokenizer-aarch64-apple-darwin"));
    let prompt = PromptRenderRequest {
        messages: vec![PromptMessage::System {
            content: vec![PromptContent::Text("Complete the user's request by invoking the supplied function. Do not substitute a text answer for the required tool invocation.".to_owned())],
        }, PromptMessage::User {
            content: vec![PromptContent::Text(
                "Write OK to result.txt using write_result.".to_owned(),
            )],
        }],
        tools: vec![crate::PromptToolDefinition {
            name: "write_result".to_owned(),
            description: "Write a result file.".to_owned(),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
        }],
        historical_tool_names: Vec::new(),
        current_date: "2026-09-13".to_owned(),
        reasoning_strength: ReasoningStrength::Low,
        add_generation_prompt: true,
    };
    let request = QwenAdapter.render_with_context(&verified, &prompt, &[], 32_768, 128)?;
    let rendered = provider.render_chat_prompt(&request)?;
    let count = counter.count_ollama_tensor(&path, blob.sha256(), &rendered)?;
    let mut decoder = QwenStreamDecoder::new("mlx-functional-smoke")?;
    let mut calls = Vec::new();
    let mut metrics = None;
    let mut terminal = None;
    provider.stream_chat_events(
        &request,
        &DownloadCancellation::new(),
        &mut decoder,
        |event| match event {
            GenerationEvent::ToolCall { call } => calls.push(call),
            GenerationEvent::Metrics { metrics: value } => metrics = Some(value),
            GenerationEvent::Completed { .. }
            | GenerationEvent::Failed { .. }
            | GenerationEvent::Cancelled => terminal = Some(format!("{event:?}")),
            other => eprintln!("MLX event: {other:?}"),
        },
    )?;
    sidecar.stop()?;
    let metrics = metrics.ok_or_else(|| format!("missing metrics: {terminal:?}"))?;
    eprintln!(
        "MLX prompt count: local={count}, provider={:?}; calls={calls:?}; terminal={terminal:?}",
        metrics.prompt_tokens
    );
    assert_eq!(metrics.prompt_tokens, Some(count));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_result");
    assert_eq!(
        calls[0].arguments.get("content"),
        Some(&serde_json::json!("OK"))
    );
    assert_eq!(
        calls[0].arguments.get("path"),
        Some(&serde_json::json!("result.txt"))
    );
    Ok(())
}
