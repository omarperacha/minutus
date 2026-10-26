//! Manual end-to-end smoke test for the pinned Glimmer vision profile.

use std::{env, error::Error, fs, path::PathBuf};

use model_runtime::{
    CompanionKind, CompanionLoadConfig, GenerationEvent, GenerationRequest, InferenceRuntimeStatus,
    LocalInferenceRuntime, ModelLoadConfig, PromptContent, PromptMessage, PromptRenderRequest,
    ReasoningStrength, render_model_prompt,
};

const MODEL_BYTES: u64 = 16_756_683_904;
const MODEL_SHA256: &str = "4cc57c0f51040a226e5a72cc47b7613f7772950e460a665f7083de89f183f60e";
const PROJECTOR_BYTES: u64 = 1_400_328_928;
const PROJECTOR_SHA256: &str = "f48b452316f9b213758e8659444029b961a24a07f99a1abb2a9f88b06f7c00c6";
const COMPATIBILITY_VERSION: &str = "43c7eadd41352a299ea8e0a36b3157978dd63596";

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let executable = required_path(arguments.next(), "llama-server path")?;
    let model = required_path(arguments.next(), "primary GGUF path")?;
    let projector = required_path(arguments.next(), "projector GGUF path")?;
    let image_path = required_path(arguments.next(), "local PNG path")?;
    if arguments.next().is_some() {
        return Err(
            "usage: multimodal_smoke <llama-server> <model.gguf> <projector.gguf> <image.png>"
                .into(),
        );
    }

    let image = fs::read(image_path)?;
    let projector = CompanionLoadConfig::new(
        CompanionKind::VisionProjector,
        projector,
        PROJECTOR_SHA256,
        PROJECTOR_BYTES,
        COMPATIBILITY_VERSION,
    );
    let config = ModelLoadConfig::new(executable, model, MODEL_SHA256, MODEL_BYTES)
        .with_limits(8_192, 512)
        .with_companions(COMPATIBILITY_VERSION, vec![projector]);
    let runtime = LocalInferenceRuntime::new()?;

    eprintln!("fully verifying primary model and vision projector...");
    runtime.load_model(config)?;
    assert_eq!(
        runtime.status()?,
        InferenceRuntimeStatus::Loaded {
            context_tokens: 8_192,
            output_tokens: 512,
            multimodal: true,
        }
    );
    eprintln!("verified multimodal Metal profile is ready");

    let profile = runtime.active_profile()?;
    let prompt = render_model_prompt(
        profile.binding().adapter(),
        &PromptRenderRequest {
            messages: vec![PromptMessage::User {
                content: vec![
                PromptContent::Text(
                    "Describe the attached image briefly and identify any visible symbol or letter."
                        .to_owned(),
                ),
                PromptContent::Image,
            ],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-15".to_owned(),
            reasoning_strength: ReasoningStrength::Low,
            add_generation_prompt: true,
        },
    )?;
    let request = GenerationRequest::new(
        "model-009-real-smoke",
        prompt,
        512,
        profile.binding().clone(),
    )
    .with_normalized_images(vec![image])?;
    let mut final_content = String::new();
    let mut terminal_events = 0_u8;
    runtime.start_generation(&request, |event| {
        println!("{event:?}");
        match event {
            GenerationEvent::FinalContentDelta { text } => final_content.push_str(&text),
            GenerationEvent::Completed { .. }
            | GenerationEvent::Cancelled
            | GenerationEvent::Failed { .. } => {
                terminal_events = terminal_events.saturating_add(1);
            }
            GenerationEvent::OutputStarted
            | GenerationEvent::ReasoningDelta { .. }
            | GenerationEvent::ContentPreviewDelta { .. }
            | GenerationEvent::ContentPreviewReset
            | GenerationEvent::ToolCall { .. }
            | GenerationEvent::Metrics { .. } => {}
        }
    })?;
    assert_eq!(
        terminal_events, 1,
        "generation must emit one terminal event"
    );
    assert!(
        !final_content.trim().is_empty(),
        "vision generation returned no final content"
    );

    runtime.unload_model()?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    eprintln!("MODEL-009 real multimodal smoke passed; model and projector unloaded");
    Ok(())
}

fn required_path(value: Option<std::ffi::OsString>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}").into())
}
