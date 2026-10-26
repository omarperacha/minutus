//! Bounded, counterbalanced fixture conversations through real agent execution.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use app_core::AppCore;
use ipc_types::{ChatRequest, ConversationId, RequestId, ValidateIpc};
use serde_json::{Value, json};
use tauri::Listener;

use super::{EvalResult, recording::Recording};
use crate::app_state::{CHAT_GENERATION_EVENT, ChatGenerationControl};

pub(super) struct Suite {
    cases: Vec<Value>,
    fixture_directory: PathBuf,
    repeats: usize,
    timeout_seconds: u64,
    warmup: bool,
}

impl Suite {
    pub(super) fn read(path: &Path) -> EvalResult<Self> {
        let value: Value = serde_json::from_slice(&fs::read(path)?)?;
        if value["schema_version"] != 1
            || (value["synthetic_only"] != true && value["fixture_only"] != true)
        {
            return Err("expected version 1 explicitly scoped evaluation suite".into());
        }
        let repeats = value["repeats"].as_u64().ok_or("repeats missing")?;
        let timeout_seconds = value["timeout_seconds"].as_u64().ok_or("timeout missing")?;
        let warmup = match value.get("warmup") {
            None => true,
            Some(value) => value.as_bool().ok_or("warmup must be boolean")?,
        };
        let cases = value["cases"].as_array().ok_or("cases missing")?.clone();
        if !(1..=40).contains(&repeats)
            || !(1..=3_600).contains(&timeout_seconds)
            || cases.is_empty()
            || cases.len() > 16
        {
            return Err("evaluation bounds exceeded".into());
        }
        for case in &cases {
            if value["fixture_only"] == true && case["workspace_fixture"].as_str().is_none() {
                return Err("local fixture cases require an isolated workspace fixture".into());
            }
            if case["id"].as_str().is_none() || case["prompt"].as_str().is_none() {
                return Err("case needs id and prompt".into());
            }
            for field in ["workspace_fixture", "image", "followup"] {
                if case.get(field).is_some_and(|value| !value.is_string()) {
                    return Err(format!("{field} must be text").into());
                }
            }
            if case.get("followups").is_some_and(|value| {
                value.as_array().is_none_or(|items| {
                    items.len() > 8 || items.iter().any(|item| !item.is_string())
                })
            }) {
                return Err("followups must contain at most eight text prompts".into());
            }
        }
        Ok(Self {
            cases,
            fixture_directory: path.parent().ok_or("suite has no parent")?.to_owned(),
            repeats: usize::try_from(repeats)?,
            timeout_seconds,
            warmup,
        })
    }
}

pub(super) fn timestamp() -> EvalResult<String> {
    Ok(time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?)
}

pub(super) fn run(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    control: &Arc<ChatGenerationControl>,
    executable: &Path,
    directory: &Path,
    suite: &Suite,
) -> EvalResult<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("measurements.jsonl"))?;
    // Latency studies retain block zero by default. Correctness-only probes can
    // explicitly omit warmup; this never changes production inference settings.
    for block in usize::from(!suite.warmup)..=suite.repeats {
        for offset in 0..suite.cases.len() {
            let case = &suite.cases[(offset + block) % suite.cases.len()];
            let conversation = conversation_for_case(
                core,
                case,
                &suite.fixture_directory,
                &directory.join(format!("workspace-{block}-{offset}")),
            )?;
            let asset_ids = match case["image"].as_str() {
                Some(path) => vec![
                    core.import_image(
                        &fs::read(suite.fixture_directory.join(path))?,
                        &timestamp()?,
                    )?
                    .id,
                ],
                None => Vec::new(),
            };
            let mut prompts = vec![("initial", case["prompt"].as_str().ok_or("prompt missing")?)];
            if let Some(followup) = case["followup"].as_str() {
                prompts.push(("continuation", followup));
            }
            if let Some(followups) = case["followups"].as_array() {
                for followup in followups {
                    prompts.push((
                        "continuation",
                        followup.as_str().ok_or("followup must be text")?,
                    ));
                }
            }
            for (phase, prompt) in prompts {
                let started_at = timestamp()?;
                ChatRequest {
                    project_id: conversation.project_id.clone(),
                    conversation_id: Some(conversation.id.clone()),
                    text: prompt.to_owned(),
                    asset_ids: if phase == "initial" {
                        asset_ids.clone()
                    } else {
                        Vec::new()
                    },
                    model_selection: None,
                    context_tokens: None,
                    effort: None,
                }
                .validate()
                .map_err(|error| format!("invalid fixture: {error:?}"))?;
                let conversation = core.append_user_message(
                    &conversation.id,
                    prompt.to_owned(),
                    if phase == "initial" {
                        asset_ids.clone()
                    } else {
                        Vec::new()
                    },
                    &timestamp()?,
                )?;
                let row = trial(
                    app,
                    core,
                    control,
                    executable,
                    conversation,
                    suite.timeout_seconds,
                )?;
                let completed = row["terminal"]["type"] == "final";
                let row = json!({"schema_version": 1, "case": case["id"], "block": block,
                    "warmup": block == 0, "phase": phase, "started_at": started_at,
                    "measurement": row});
                writeln!(output, "{row}")?;
                output.flush()?;
                eprintln!("Qwen evaluation: block {block}, {} / {phase}", case["id"]);
                if !completed {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn conversation_for_case(
    core: &AppCore,
    case: &Value,
    fixtures: &Path,
    destination: &Path,
) -> EvalResult<ipc_types::Conversation> {
    if let Some(relative) = case["workspace_fixture"].as_str() {
        let project = core.create_project("Isolated trajectory fixture", &timestamp()?)?;
        super::workspace::install(core, &project.id, fixtures, relative, destination)?;
        Ok(core.create_project_conversation(&project.id, &timestamp()?)?)
    } else {
        Ok(core.create_conversation(&timestamp()?)?)
    }
}

fn trial(
    app: &tauri::AppHandle,
    core: &Arc<AppCore>,
    control: &Arc<ChatGenerationControl>,
    executable: &Path,
    conversation: ipc_types::Conversation,
    timeout_seconds: u64,
) -> EvalResult<Value> {
    let request_id = RequestId(uuid::Uuid::now_v7().to_string());
    let conversation_id: ConversationId = conversation.id.clone();
    let cancellation = control
        .begin(request_id.clone(), core.begin_chat_activity()?)
        .map_err(|error| format!("evaluation admission failed: {error:?}"))?;
    let recording = Arc::new(Mutex::new(Recording::new()));
    let listener_recording = Arc::clone(&recording);
    let listener = app.listen(CHAT_GENERATION_EVENT, move |event| {
        if let Ok(mut recording) = listener_recording.lock() {
            recording.observe(event.payload());
        }
    });
    let (finished, receiver) = mpsc::channel();
    let watchdog_core = Arc::clone(core);
    let watchdog_control = Arc::clone(control);
    let watchdog_id = request_id.clone();
    let watchdog = std::thread::spawn(move || {
        if receiver
            .recv_timeout(Duration::from_secs(timeout_seconds))
            .is_err()
        {
            let _ = watchdog_control.cancel(&watchdog_id);
            let _ = watchdog_core.stop_generation(&watchdog_id.0);
        }
    });
    super::super::generation::run_chat_generation(
        app.clone(),
        Arc::clone(core),
        Arc::clone(control),
        executable.to_owned(),
        request_id.clone(),
        conversation,
        cancellation,
        super::super::generation::ChatGenerationSettings::default(),
    );
    let _ = finished.send(());
    let _ = watchdog.join();
    app.unlisten(listener);
    let mut result = recording
        .lock()
        .map_err(|_| "recording unavailable")?
        .finish();
    result["request_id"] = json!(request_id.0);
    result["conversation_id"] = json!(conversation_id.0);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suite_requires_explicit_input_scope_and_bounded_execution() -> EvalResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("suite.json");
        let mut suite = json!({"schema_version":1,"synthetic_only":true,"repeats":20,
            "timeout_seconds":300,"cases":[{"id":"text","prompt":"OK"}]});
        fs::write(&path, serde_json::to_vec(&suite)?)?;
        assert!(Suite::read(&path)?.warmup);
        suite["warmup"] = json!(false);
        suite["timeout_seconds"] = json!(1800);
        fs::write(&path, serde_json::to_vec(&suite)?)?;
        let parsed = Suite::read(&path)?;
        assert!(!parsed.warmup);
        assert_eq!(parsed.timeout_seconds, 1800);
        for (key, invalid) in [
            ("synthetic_only", json!(false)),
            ("repeats", json!(41)),
            ("timeout_seconds", json!(3601)),
            ("warmup", json!("false")),
        ] {
            let previous = suite[key].clone();
            suite[key] = invalid;
            fs::write(&path, serde_json::to_vec(&suite)?)?;
            assert!(Suite::read(&path).is_err());
            suite[key] = previous;
        }
        suite["synthetic_only"] = json!(false);
        suite["fixture_only"] = json!(true);
        fs::write(&path, serde_json::to_vec(&suite)?)?;
        assert!(Suite::read(&path).is_err());
        suite["cases"][0]["workspace_fixture"] = json!("source");
        fs::write(&path, serde_json::to_vec(&suite)?)?;
        assert!(Suite::read(&path).is_ok());
        suite["cases"][0]["followups"] = json!(["first reply", 42]);
        fs::write(&path, serde_json::to_vec(&suite)?)?;
        assert!(Suite::read(&path).is_err());
        Ok(())
    }
}
