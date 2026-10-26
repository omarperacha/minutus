//! Manual end-to-end smoke test for a locally available Glimmer GGUF.

use std::{env, error::Error, fs, path::PathBuf, sync::mpsc, thread, time::Duration};

use model_runtime::{
    GenerationEvent, GenerationRequest, InferenceError, InferenceRuntimeStatus,
    LocalInferenceRuntime, ModelLoadConfig, PromptContent, PromptMessage, PromptRenderRequest,
    ReasoningStrength, render_model_prompt,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let executable = required_path(arguments.next(), "llama-server path")?;
    let model = required_path(arguments.next(), "GGUF path")?;
    let digest = arguments
        .next()
        .ok_or("missing expected SHA-256")?
        .into_string()
        .map_err(|_| "SHA-256 must be UTF-8")?;
    if arguments.next().is_some() {
        return Err("usage: inference_smoke <llama-server> <model.gguf> <sha256>".into());
    }

    let model_bytes = fs::metadata(&model)?.len();
    let runtime = LocalInferenceRuntime::new()?;
    let config =
        ModelLoadConfig::new(executable, model, digest, model_bytes).with_limits(4_096, 512);

    eprintln!("loading and verifying {model_bytes} bytes...");
    runtime.load_model(config)?;
    assert_eq!(
        runtime.status()?,
        InferenceRuntimeStatus::Loaded {
            context_tokens: 4_096,
            output_tokens: 512,
            multimodal: false,
        }
    );

    let profile = runtime.active_profile()?;
    let prompt = render_model_prompt(
        profile.binding().adapter(),
        &PromptRenderRequest {
            messages: vec![PromptMessage::User {
                content: vec![PromptContent::Text(
                    "Reply with the single token GLIMMER_OK and nothing else.".to_owned(),
                )],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-15".to_owned(),
            reasoning_strength: ReasoningStrength::Low,
            add_generation_prompt: true,
        },
    )?;
    let request = GenerationRequest::new("model-007-smoke", prompt, 512, profile.binding().clone());
    let mut terminal_events = 0_u8;
    let mut visible_content = String::new();
    runtime.start_generation(&request, |event| {
        println!("{event:?}");
        match event {
            GenerationEvent::FinalContentDelta { text } => visible_content.push_str(&text),
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
        visible_content.contains("GLIMMER_OK"),
        "unexpected final content: {visible_content:?}"
    );

    smoke_cancellation(&runtime)?;
    runtime.unload_model()?;
    assert_eq!(runtime.status()?, InferenceRuntimeStatus::Unloaded);
    eprintln!("MODEL-007 smoke test passed; model unloaded");
    Ok(())
}

fn smoke_cancellation(runtime: &LocalInferenceRuntime) -> Result<(), Box<dyn Error>> {
    let profile = runtime.active_profile()?;
    let prompt = render_model_prompt(
        profile.binding().adapter(),
        &PromptRenderRequest {
            messages: vec![PromptMessage::User {
                content: vec![PromptContent::Text(
                    "Write a detailed numbered history of computing, continuing until stopped."
                        .to_owned(),
                )],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-15".to_owned(),
            reasoning_strength: ReasoningStrength::High,
            add_generation_prompt: true,
        },
    )?;
    let request =
        GenerationRequest::new("model-007-cancel", prompt, 512, profile.binding().clone());
    let (sender, receiver) = mpsc::channel();

    thread::scope(|scope| -> Result<(), Box<dyn Error>> {
        let generation = scope.spawn(|| {
            runtime.start_generation(&request, |event| {
                let _ = sender.send(event);
            })
        });
        let first = receiver.recv_timeout(Duration::from_mins(2))?;
        assert!(
            !matches!(
                first,
                GenerationEvent::Completed { .. }
                    | GenerationEvent::Cancelled
                    | GenerationEvent::Failed { .. }
            ),
            "generation terminated before cancellation: {first:?}"
        );
        assert!(runtime.stop_generation("model-007-cancel")?);
        let result = generation
            .join()
            .map_err(|_| "cancellation generation thread panicked")?;
        assert!(matches!(result, Err(InferenceError::Cancelled)));

        let terminal = receiver
            .try_iter()
            .filter(|event| {
                matches!(
                    event,
                    GenerationEvent::Completed { .. }
                        | GenerationEvent::Cancelled
                        | GenerationEvent::Failed { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal.len(),
            1,
            "cancellation must emit one terminal event"
        );
        assert!(matches!(terminal.first(), Some(GenerationEvent::Cancelled)));
        eprintln!("active cancellation passed");
        Ok(())
    })
}

fn required_path(value: Option<std::ffi::OsString>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| name.to_owned().into())
}
