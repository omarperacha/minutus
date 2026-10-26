//! Content-free cache decisions from the guarded development provider.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

const FILE_NAME: &str = "zeta-provider-cache.jsonl";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 16 * 1024;

pub(super) fn capture(
    stderr: impl Read + Send + 'static,
    directory: PathBuf,
) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("zeta-ollama-cache-telemetry".to_owned())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if line.len() > MAX_LINE_BYTES {
                    continue;
                }
                if let Some(data) = parse_line(&line) {
                    let _ = append(&directory, &data);
                }
            }
        })
        .ok()
}

fn parse_line(line: &str) -> Option<Value> {
    let slot = number_after(line, "id  ")?;
    let task = number_after(line, "| task ");
    let fields = if line.contains("checking sim =") {
        let shared = number_after(line, "(")?;
        let previous = number_after(line, "/")?;
        json!({"event": "provider_cache_prefix", "sharedTokens": shared,
               "previousPromptTokens": previous})
    } else if line.contains("created context checkpoint") {
        json!({"event": "provider_cache_checkpoint_created",
               "position": number_after(line, "pos_min = ")?,
               "tokenCount": number_after(line, "n_tokens = ")?,
               "sizeMiB": decimal_after(line, "size = ")?})
    } else if line.contains("restored context checkpoint") {
        json!({"event": "provider_cache_checkpoint_restored",
               "position": number_after(line, "pos_min = ")?,
               "cachedTokens": number_after(line, "n_past = ")?,
               "sizeMiB": decimal_after(line, "size = ")?})
    } else if line.contains("erased invalidated context checkpoint")
        || line.contains("erasing old context checkpoint")
    {
        json!({"event": "provider_cache_checkpoint_evicted",
               "position": number_after(line, "pos_min = ")?,
               "sizeMiB": decimal_after(line, "size = ")?})
    } else if line.contains("forcing full prompt re-processing due to lack of cache data") {
        json!({"event": "provider_cache_full_reprocessing"})
    } else if line.contains("prompt eval time =") {
        json!({"event": "provider_cache_prompt_evaluated",
               "evaluatedTokens": number_after(line, "/")?,
               "evaluationMs": decimal_after(line, "prompt eval time =")?})
    } else {
        return None;
    };
    let mut data = fields;
    data["slotId"] = slot.into();
    if let Some(task) = task {
        data["backendTaskId"] = task.into();
    }
    Some(data)
}

fn number_after(line: &str, marker: &str) -> Option<u64> {
    let tail = line.split_once(marker)?.1.trim_start();
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn decimal_after(line: &str, marker: &str) -> Option<f64> {
    let tail = line.split_once(marker)?.1.trim_start();
    let digits: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    digits.parse().ok()
}

fn append(directory: &Path, data: &Value) -> Result<(), std::io::Error> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let record = json!({
        "schemaVersion": 1,
        "timestampUnixMs": SystemTime::now().duration_since(UNIX_EPOCH)
            .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        "data": data
    });
    let mut encoded = serde_json::to_vec(&record)?;
    encoded.push(b'\n');
    let path = directory.join(FILE_NAME);
    if fs::metadata(&path)
        .map_or(0, |metadata| metadata.len())
        .saturating_add(encoded.len() as u64)
        > MAX_FILE_BYTES
    {
        let previous = directory.join(format!("{FILE_NAME}.previous"));
        let _ = fs::remove_file(&previous);
        fs::rename(&path, previous)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Value, std::io::Error> {
        parse_line(line).ok_or_else(|| std::io::Error::other("expected cache event"))
    }

    #[test]
    fn extracts_only_numeric_cache_decisions() -> Result<(), Box<dyn std::error::Error>> {
        let created = parsed(
            "slot create_check: id  0 | task 27 | created context checkpoint 1 of 32 (pos_min = 7985, pos_max = 7985, n_tokens = 7986, size = 180.974 MiB)",
        )?;
        assert_eq!(created["event"], "provider_cache_checkpoint_created");
        assert_eq!(created["position"], 7985);
        assert_eq!(created["backendTaskId"], 27);
        assert_eq!(created["sizeMiB"], 180.974);

        let prefix = parsed(
            "slot get_availabl: id  0 | task -1 |  - checking sim = 0.552 (4690/8498) > 0.100",
        )?;
        assert_eq!(prefix["sharedTokens"], 4690);
        assert_eq!(prefix["previousPromptTokens"], 8498);
        assert!(prefix.get("backendTaskId").is_none());

        let timing = parsed(
            "slot print_timing: id  0 | task 27 | prompt eval time =  136403.76 ms /  8498 tokens (private prompt)",
        )?;
        assert_eq!(timing["evaluatedTokens"], 8498);
        assert_eq!(timing["evaluationMs"], 136_403.76);
        assert!(!timing.to_string().contains("private prompt"));
        assert!(parse_line("prompt body with private content").is_none());
        Ok(())
    }

    #[test]
    fn writes_private_jsonl_without_raw_log_line() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let data = parsed(
            "slot   operator(): id  0 | task 27 | forcing full prompt re-processing due to lack of cache data (private)",
        )?;
        append(directory.path(), &data)?;
        let contents = fs::read_to_string(directory.path().join(FILE_NAME))?;
        assert!(contents.contains("provider_cache_full_reprocessing"));
        assert!(!contents.contains("private"));
        Ok(())
    }

    #[test]
    fn guarded_stream_discards_unrecognized_log_content() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let input = b"private request body\nslot print_timing: id  0 | task 27 | prompt eval time =  120.5 ms /  42 tokens\n";
        let reader = capture(std::io::Cursor::new(input), directory.path().to_owned())
            .ok_or_else(|| std::io::Error::other("cache reader unavailable"))?;
        reader
            .join()
            .map_err(|_| std::io::Error::other("cache reader panicked"))?;
        let contents = fs::read_to_string(directory.path().join(FILE_NAME))?;
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.contains("\"evaluatedTokens\":42"));
        assert!(!contents.contains("private"));
        Ok(())
    }
}
