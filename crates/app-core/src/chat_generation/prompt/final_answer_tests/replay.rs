//! Explicit opt-in replay of saved, text-only results; never executes task tools.

use super::*;
use model_runtime::{
    DownloadCancellation, ExactTokenizer, GenerationCompletion, GenerationEvent, OllamaProvider,
    OllamaProviderConfig, OllamaSidecar, OllamaSidecarConfig, QwenStreamDecoder,
};
use std::{fs, path::Path};

fn saved_requests() -> TestResult<(PromptRenderRequest, PromptRenderRequest)> {
    let snapshot = fs::read(std::env::var_os("ZETA_ANSWER_REPLAY").ok_or("snapshot missing")?)?;
    let snapshot: serde_json::Value = serde_json::from_slice(&snapshot)?;
    let conversation: Conversation = serde_json::from_value(snapshot["conversation"].clone())?;
    let plan = &snapshot["plan"];
    assert_eq!(plan["contractVersion"], "semantic_execution_v6");
    assert_eq!(plan["outcome"], "answer");
    let strings = |value: &serde_json::Value| -> TestResult<Vec<String>> {
        value
            .as_array()
            .ok_or("array missing")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "string missing".into())
            })
            .collect()
    };
    let steps = plan["steps"]
        .as_array()
        .ok_or("steps missing")?
        .iter()
        .enumerate()
        .map(|(index, step)| -> TestResult<ToolPlanStepProgress> {
            let id = step["id"].as_str().ok_or("step ID missing")?;
            Ok(ToolPlanStepProgress::new(
                index + 1,
                ToolPlanStepStatus::ForecastComplete,
                Vec::new(),
            )
            .with_contract(
                id.to_owned(),
                step["objective"]
                    .as_str()
                    .ok_or("objective missing")?
                    .to_owned(),
                Vec::new(),
                step["doneWhen"]
                    .as_str()
                    .ok_or("doneWhen missing")?
                    .to_owned(),
                strings(&snapshot["stepCalls"][id])?,
            )
            .with_dependencies(strings(&step["dependsOn"])?))
        })
        .collect::<TestResult<Vec<_>>>()?;
    let progress = ToolPlanProgress::new(
        1,
        0,
        steps,
        None,
        ToolPlanOutcome::Answer,
        false,
        false,
        2,
        false,
        0,
    )
    .with_outcome_contract(
        plan["target"].as_str().ok_or("target missing")?.to_owned(),
        strings(&plan["constraints"])?,
        strings(&plan["successCriteria"])?,
    )
    .with_retained_call_ids(strings(&snapshot["retainedCallIds"])?);
    // Reconstruct the observed old selection, not an asserted byte-for-byte
    // historical prompt: the original transient ledger/capsule was not saved.
    // Both arms share today's production builder, policy, options and decoder.
    let old_ids = strings(&snapshot["retainedCallIds"])?.into_iter().collect();
    let mut old_conversation = conversation.clone();
    old_conversation.messages =
        focused_phase_messages(&conversation, &old_ids, 0..conversation.messages.len());
    let old = request(&old_conversation, context(progress.clone()))?;
    let fixed = request(&conversation, context(progress))?;
    assert_eq!(old.tools, fixed.tools);
    assert_eq!(old.messages.first(), fixed.messages.first());
    Ok((old, fixed))
}

#[test]
#[ignore = "requires ZETA_ANSWER_REPLAY snapshot and an idle pinned local Qwen store"]
fn live_saved_answer_handoff() -> TestResult {
    let (old, fixed) = saved_requests()?;
    let models = std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("model store missing")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sidecar = OllamaSidecar::new();
    let endpoint = sidecar.start(
        &OllamaSidecarConfig::new(root.join("src-tauri/ollama-runtime/ollama"), &models)
            .with_guardian_executable(root.join("target/debug/zeta-desktop")),
    )?;
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(endpoint.address().into())?)?;
    let catalogue = model_runtime::bundled_catalogue()?;
    let profile = catalogue
        .models()
        .iter()
        .find(|model| model.backend() == model_runtime::InferenceBackend::Ollama)
        .ok_or("Qwen missing")?;
    let model = provider.verify_model(profile)?;
    let tokenizer =
        ExactTokenizer::new(root.join("src-tauri/binaries/zeta-tokenizer-aarch64-apple-darwin"));
    let weights = Path::new(&models)
        .join("blobs")
        .join(format!("sha256-{}", profile.artifact().sha256()));
    // TempDir creates a private directory. Only this explicitly requested local
    // probe writes prompts/raw output; normal application diagnostics never do.
    let output = tempfile::Builder::new()
        .prefix("zeta-answer-replay-")
        .tempdir()?
        .keep();
    eprintln!("Replay artifacts: {}", output.display());
    for (label, request) in [("old", old), ("fixed", fixed)] {
        let native = QwenAdapter.render_with_context(&model, &request, &[], 32768, 6144)?;
        let rendered = provider.render_chat_prompt(&native)?;
        let tokens = tokenizer.count(&weights, &rendered)?;
        replay_answer(&provider, &native, label, tokens, &output)?;
    }
    sidecar.stop()?;
    Ok(())
}

fn replay_answer(
    provider: &OllamaProvider,
    native: &model_runtime::OllamaChatRequest,
    label: &str,
    tokens: u64,
    output: &Path,
) -> TestResult {
    // The same exact-token admission controller and reserves remain in
    // force. No truncation, extra context or output budget is granted.
    ContextAdmissionController::new(ContextAdmissionPolicy::new(32768, 1024)?).admit_tool_round(
        RoundAllowance {
            remaining: ExecutionBudgets {
                rounds: 1,
                tool_calls: 0,
                elapsed_ms: u64::MAX,
                tool_ms: 0,
                context_tokens: 32768,
                final_response_tokens: 6144,
                tool_output_bytes: 0,
            },
            tools_enabled: true,
        },
        ContextTokenEstimate {
            conversation_history: tokens,
            ..ContextTokenEstimate::default()
        },
    )?;
    fs::write(
        output.join(format!("{label}-request.json")),
        serde_json::to_vec_pretty(native)?,
    )?;
    let mut raw = Vec::new();
    let mut decoder = QwenStreamDecoder::new(label)?;
    let mut events = Vec::new();
    let started = std::time::Instant::now();
    provider.stream_chat_bytes(native, &DownloadCancellation::new(), |bytes| {
        raw.extend_from_slice(bytes);
        events.extend(decoder.push(bytes));
        true
    })?;
    events.extend(decoder.finish(GenerationCompletion::Stop));
    fs::write(output.join(format!("{label}-raw.ndjson")), &raw)?;
    let answer = events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::FinalContentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let raw_content = raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .filter_map(|frame| frame["message"]["content"].as_str())
        .collect::<String>();
    fs::write(output.join(format!("{label}-answer.txt")), &answer)?;
    assert_eq!(
        raw_content, answer,
        "provider content changed during decoding"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        GenerationEvent::Completed {
            reason: GenerationCompletion::Stop
        }
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        GenerationEvent::Failed { .. } | GenerationEvent::ToolCall { .. }
    )));
    eprintln!(
        "{label}: input_tokens={tokens}, elapsed_ms={}, answer_bytes={}, raw_content_matches=true",
        started.elapsed().as_millis(),
        answer.len()
    );
    Ok(())
}
