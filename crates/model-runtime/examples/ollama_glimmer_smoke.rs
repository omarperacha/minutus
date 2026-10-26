//! Live registration and raw-generation smoke test for Glimmer over Ollama.

use std::{collections::BTreeMap, env, error::Error, net::SocketAddr, path::PathBuf};

use model_runtime::{
    DownloadCancellation, GenerationEvent, ModelAdapterKind, OllamaChatRequest, OllamaProvider,
    OllamaProviderConfig, PromptContent, PromptMessage, PromptRenderRequest, QwenStreamDecoder,
    ReasoningStrength, render_model_prompt,
};

const TAG: &str = "zeta/muse-glimmer-30b:4cc57c0f5104-v2";
const MODEL_DIGEST: &str = "4cc57c0f51040a226e5a72cc47b7613f7772950e460a665f7083de89f183f60e";
const PROJECTOR_DIGEST: &str = "f48b452316f9b213758e8659444029b961a24a07f99a1abb2a9f88b06f7c00c6";

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let address: SocketAddr = args.next().ok_or("missing loopback address")?.parse()?;
    let model = PathBuf::from(args.next().ok_or("missing model path")?);
    let projector = PathBuf::from(args.next().ok_or("missing projector path")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }

    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;
    provider.ensure_blob(&model, MODEL_DIGEST)?;
    provider.ensure_blob(&projector, PROJECTOR_DIGEST)?;
    let verified = match provider.verify_registered_tag(TAG) {
        Ok(verified) => verified,
        Err(_) => provider.create_from_blobs(
            TAG,
            BTreeMap::from([
                (
                    "Muse-Glimmer-30B-Q4_K_M.gguf".to_owned(),
                    format!("sha256:{MODEL_DIGEST}"),
                ),
                (
                    "mmproj-Muse-Glimmer-30B-Q4_K_M.gguf".to_owned(),
                    format!("sha256:{PROJECTOR_DIGEST}"),
                ),
            ]),
        )?,
    };

    let prompt = render_model_prompt(
        ModelAdapterKind::GlimmerAtem,
        &PromptRenderRequest {
            messages: vec![PromptMessage::User {
                content: vec![PromptContent::Text("Reply with exactly OK.".to_owned())],
            }],
            tools: Vec::new(),
            historical_tool_names: Vec::new(),
            current_date: "2026-08-20".to_owned(),
            reasoning_strength: ReasoningStrength::Low,
            add_generation_prompt: true,
        },
    )?;
    let request = OllamaChatRequest::adapter_prompt(&verified, prompt, Vec::new(), &[])
        .with_context(32_768, 256);
    let mut decoder = QwenStreamDecoder::new("glimmer-smoke")
        .map_err(|error| std::io::Error::other(format!("decoder: {error:?}")))?;
    provider.stream_chat_events(
        &request,
        &DownloadCancellation::new(),
        &mut decoder,
        |event| match event {
            GenerationEvent::ReasoningDelta { text }
            | GenerationEvent::FinalContentDelta { text } => print!("{text}"),
            GenerationEvent::Completed { reason } => println!("\ncompleted: {reason:?}"),
            GenerationEvent::Failed { failure } => eprintln!("failed: {failure:?}"),
            _ => {}
        },
    )?;
    Ok(())
}
