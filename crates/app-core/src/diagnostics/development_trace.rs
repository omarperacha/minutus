//! Debug-build-only raw tool-call tracing for local development.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const TRACE_FILE_NAME: &str = "zeta-tool-trace.jsonl";
const TELEMETRY_FILE_NAME: &str = "zeta-execution-telemetry.jsonl";
const MAX_TRACE_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) struct DevelopmentToolTrace {
    directory: PathBuf,
    sequence: AtomicU64,
    write_lock: Mutex<()>,
}

impl DevelopmentToolTrace {
    pub(crate) fn new(directory: impl Into<PathBuf>) -> Self {
        let trace = Self {
            directory: directory.into(),
            sequence: AtomicU64::new(0),
            write_lock: Mutex::new(()),
        };
        eprintln!(
            "Minutus development tool trace enabled: {}",
            trace.directory.join(TRACE_FILE_NAME).display()
        );
        trace.record(&serde_json::json!({
            "event": "development_tool_trace_started"
        }));
        trace
    }

    pub(crate) fn record(&self, data: &serde_json::Value) {
        let telemetry = is_execution_telemetry(data);
        if telemetry && !execution_telemetry_is_content_free(data) {
            return;
        }
        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        let record = serde_json::json!({
            "schemaVersion": 1,
            "timestampUnixMs": timestamp_unix_ms(),
            "sequence": self.sequence.fetch_add(1, Ordering::Relaxed),
            "data": data,
        });
        let Ok(mut encoded) = serde_json::to_vec(&record) else {
            return;
        };
        encoded.push(b'\n');
        let file_name = if telemetry {
            TELEMETRY_FILE_NAME
        } else {
            TRACE_FILE_NAME
        };
        let _ = self.append(file_name, &encoded);
    }

    fn append(&self, file_name: &str, encoded: &[u8]) -> Result<(), std::io::Error> {
        prepare_private_directory(&self.directory)?;
        let path = self.directory.join(file_name);
        if fs::metadata(&path).map_or(0, |metadata| {
            metadata.len().saturating_add(encoded.len() as u64)
        }) > MAX_TRACE_FILE_BYTES
        {
            let rotated = self.directory.join(format!("{file_name}.previous"));
            let _ = fs::remove_file(&rotated);
            fs::rename(&path, rotated)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(encoded)?;
        file.flush()
    }
}

fn is_execution_telemetry(data: &serde_json::Value) -> bool {
    data.get("event")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|event| event.starts_with("execution_"))
}

fn execution_telemetry_is_content_free(data: &serde_json::Value) -> bool {
    const FORBIDDEN_KEYS: &[&str] = &[
        "arguments",
        "callId",
        "content",
        "error",
        "message",
        "path",
        "prompt",
        "reasoning",
        "recipient",
        "request",
        "result",
        "url",
    ];
    match data {
        serde_json::Value::Object(fields) => fields.iter().all(|(key, value)| {
            !FORBIDDEN_KEYS.contains(&key.as_str()) && execution_telemetry_is_content_free(value)
        }),
        serde_json::Value::Array(values) => values.iter().all(execution_telemetry_is_content_free),
        serde_json::Value::String(value) => value.len() <= 256,
        _ => true,
    }
}

fn prepare_private_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn timestamp_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_debug_trace_records_tool_arguments_in_separate_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::TempDir::new()?;
        let trace = DevelopmentToolTrace::new(temporary.path());
        trace.record(&serde_json::json!({
            "event": "model_tool_call_emitted",
            "tool": "filesystem.stat",
            "arguments": {"path": "private/example", "workspace": "primary"}
        }));
        let contents = fs::read_to_string(temporary.path().join(TRACE_FILE_NAME))?;
        assert!(contents.contains("model_tool_call_emitted"));
        assert!(contents.contains("private/example"));
        assert!(
            contents
                .lines()
                .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
        );
        Ok(())
    }

    #[test]
    fn execution_telemetry_is_separate_bounded_and_content_free()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::TempDir::new()?;
        let trace = DevelopmentToolTrace::new(temporary.path());
        trace.record(&serde_json::json!({
            "event": "execution_round_completed",
            "round": 1,
            "promptEvaluationMs": 12,
            "peakMemoryBytes": null
        }));
        trace.record(&serde_json::json!({
            "event": "execution_round_started",
            "round": 2,
            "imageCount": 1
        }));
        trace.record(&serde_json::json!({
            "event": "execution_round_stream_progress",
            "round": 2,
            "phase": "active",
            "deltaCount": 42,
            "elapsedMs": 30_000
        }));
        trace.record(&serde_json::json!({
            "event": "execution_round_failed",
            "round": 2,
            "failureSubtype": "unexpected_end",
            "firstProviderOutputMs": 185_000,
            "providerCallMs": 430_000
        }));
        trace.record(&serde_json::json!({
            "event": "execution_prompt_preparation_failed",
            "round": 3,
            "stage": "prompt_render",
            "subtype": "duplicate_tool_definition"
        }));
        trace.record(&serde_json::json!({
            "event": "execution_invalid",
            "prompt": "must not be recorded"
        }));
        let contents = fs::read_to_string(temporary.path().join(TELEMETRY_FILE_NAME))?;
        assert!(contents.contains("execution_round_completed"));
        assert!(contents.contains("execution_round_started"));
        assert!(contents.contains("execution_round_stream_progress"));
        assert!(contents.contains("execution_round_failed"));
        assert!(contents.contains("unexpected_end"));
        assert!(contents.contains("execution_prompt_preparation_failed"));
        assert!(contents.contains("duplicate_tool_definition"));
        assert!(!contents.contains("must not be recorded"));
        assert!(!contents.contains("prompt\""));
        let raw = fs::read_to_string(temporary.path().join(TRACE_FILE_NAME))?;
        assert!(!raw.contains("execution_round_completed"));
        Ok(())
    }
}
