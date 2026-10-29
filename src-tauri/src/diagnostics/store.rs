use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_FILES: usize = 10;
pub const RETENTION_DAYS: i64 = 14;

pub fn redact(text: &str) -> String {
    static RULES: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let rules = RULES.get_or_init(|| vec![
        regex::Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+").unwrap(),
        regex::Regex::new(r#"(?i)((?:api[_-]?key|access[_-]?token|refresh[_-]?token|password|passwd|secret|authorization)\s*["']?\s*[:=]\s*["']?)[^\s,"'};]+"#).unwrap(),
        regex::Regex::new(r"\bsk-[A-Za-z0-9_-]{12,}\b").unwrap(),
    ]);
    let mut result: String = text.chars().take(8_000).collect();
    for (index, rule) in rules.iter().enumerate() {
        result = rule
            .replace_all(
                &result,
                if index == 2 {
                    "[REDACTED]"
                } else {
                    "${1}[REDACTED]"
                },
            )
            .into_owned();
    }
    result
}

pub fn sanitize(value: &Value, depth: usize) -> Value {
    if depth > 6 {
        return Value::String("[depth limit]".into());
    }
    match value {
        Value::String(text) => Value::String(redact(text)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(48)
                .map(|item| sanitize(item, depth + 1))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .take(48)
                .map(|(key, value)| {
                    let normalized = key.to_lowercase().replace(['_', '-'], "");
                    let sensitive = [
                        "password",
                        "passwd",
                        "secret",
                        "token",
                        "apikey",
                        "authorization",
                        "cookie",
                        "headers",
                        "prompt",
                        "messages",
                        "command",
                        "output",
                        "input",
                    ]
                    .iter()
                    .any(|field| normalized.contains(field));
                    // Usage counts and truncation flags are diagnostic data;
                    // credentials and response bodies stay redacted regardless
                    // of their JSON type. Only these exact metric names pass.
                    let metric = ((value.is_u64() || value.is_null())
                        && matches!(normalized.as_str(),
                            "inputtokens" | "outputtokens" | "cachedinputtokens"
                            | "cachecreationinputtokens" | "maxtokens" | "contexttokens"
                            | "reservedoutputtokens" | "availabletokens"
                            | "generationtokens" | "generationdurationms"))
                        || (normalized == "outputtruncated" && value.is_boolean());
                    (
                        key.chars().take(80).collect(),
                        if sensitive && !metric {
                            Value::String("[REDACTED]".into())
                        } else {
                            sanitize(value, depth + 1)
                        },
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn owned_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("application-") && name.ends_with(".jsonl"))
}

pub fn log_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<_> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file()) && owned_log(&entry.path())
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    Ok(files)
}

pub struct LogStore {
    pub directory: PathBuf,
    pub run_id: String,
    pub last_error: Option<String>,
    pub current: Option<PathBuf>,
    file: Option<File>,
    bytes: u64,
    day: String,
    part: u32,
    max_bytes: u64,
    max_files: usize,
}

impl LogStore {
    pub fn new(directory: PathBuf, run_id: String) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            run_id,
            last_error: None,
            current: None,
            file: None,
            bytes: 0,
            day: String::new(),
            part: 0,
            max_bytes: MAX_FILE_BYTES,
            max_files: MAX_FILES,
        })
    }

    pub fn write(&mut self, record: &Value, durable: bool) -> io::Result<()> {
        let result = self.append(record, durable);
        self.last_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    fn append(&mut self, record: &Value, durable: bool) -> io::Result<()> {
        let mut bounded = sanitize(record, 0);
        if serde_json::to_vec(&bounded)?.len() > 32 * 1024 {
            bounded["data"] =
                serde_json::json!({"truncated":true,"reason":"diagnostic record exceeded 32 KB"});
        }
        let mut line = serde_json::to_vec(&bounded)?;
        line.push(b'\n');
        let day = chrono::Utc::now().format("%Y%m%d").to_string();
        if self.file.is_none() || self.day != day || self.bytes + line.len() as u64 > self.max_bytes
        {
            self.file = None;
            self.day = day;
            loop {
                self.part += 1;
                let path = self.directory.join(format!(
                    "application-{}-{}-{:04}.jsonl",
                    self.day, self.run_id, self.part
                ));
                match OpenOptions::new().create_new(true).write(true).open(&path) {
                    Ok(file) => {
                        self.file = Some(file);
                        self.current = Some(path);
                        self.bytes = 0;
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            self.prune()?;
        }
        let file = self.file.as_mut().unwrap();
        file.write_all(&line)?;
        self.bytes += line.len() as u64;
        // Errors, panics and native process failures are flushed to disk immediately.
        if durable {
            file.sync_data()?;
        }
        Ok(())
    }

    pub fn prune(&self) -> io::Result<()> {
        let files = log_files(&self.directory)?;
        let excess = files.len().saturating_sub(self.max_files);
        // Another running instance can share this directory. Preserve its latest file.
        let mut latest_by_pid = std::collections::HashMap::new();
        for path in &files {
            if let Some(pid) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.rsplit('-').nth(1))
                .and_then(|pid| pid.parse::<u32>().ok())
            {
                latest_by_pid.insert(pid, path);
            }
        }
        let protected: Vec<_> = latest_by_pid
            .into_iter()
            .filter(|(pid, _)| *pid != std::process::id() && super::health::process_alive(*pid))
            .map(|(_, path)| path)
            .collect();
        let mut removed = 0;
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(RETENTION_DAYS as u64 * 86_400);
        for path in &files {
            if self.current.as_ref() == Some(path) {
                continue;
            }
            if protected.contains(&path) {
                continue;
            }
            let old = fs::metadata(path)?
                .modified()
                .is_ok_and(|modified| modified < cutoff);
            if removed < excess || old {
                fs::remove_file(path)?;
                removed += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_is_bounded_and_never_removes_unrelated_logs() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-log-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut store = LogStore::new(root.clone(), "test".into()).unwrap();
        store.max_bytes = 100;
        store.max_files = 3;
        fs::write(root.join("agent.log"), "keep").unwrap();
        for index in 0..12 {
            store
                .write(
                    &serde_json::json!({"index":index,"message":"one diagnostic record"}),
                    true,
                )
                .unwrap();
        }
        let files = log_files(&root).unwrap();
        assert_eq!(files.len(), 3);
        for path in files {
            for line in fs::read_to_string(path).unwrap().lines() {
                assert!(serde_json::from_str::<Value>(line).is_ok());
            }
        }
        assert_eq!(fs::read_to_string(root.join("agent.log")).unwrap(), "keep");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn credentials_and_content_are_removed_without_losing_context() {
        let record = sanitize(
            &serde_json::json!({"threadId":"thread-12","turnId":"turn-5","command":"private shell command","apiKey":"credential","message":"password=supersecret Bearer aaa.bbb.ccc sk-1234567890123456"}),
            0,
        );
        let text = record.to_string();
        for secret in [
            "supersecret",
            "aaa.bbb.ccc",
            "sk-1234567890123456",
            "private shell command",
            "credential",
        ] {
            assert!(!text.contains(secret));
        }
        assert!(text.contains("thread-12"));
        assert!(text.contains("turn-5"));
    }
    #[test]
    fn numeric_usage_and_truncation_survive_but_credential_types_never_bypass_redaction() {
        let record = sanitize(&serde_json::json!({
            "input_tokens": 1200, "output_tokens": 45, "output_truncated": true,
            "cached_input_tokens": null, "access_token": 123456,
            "api_key": false, "output": 789, "max_tokens": "private",
        }), 0);
        assert_eq!(record["input_tokens"], 1200);
        assert_eq!(record["output_tokens"], 45);
        assert_eq!(record["output_truncated"], true);
        assert!(record["cached_input_tokens"].is_null());
        for key in ["access_token", "api_key", "output", "max_tokens"] {
            assert_eq!(record[key], "[REDACTED]");
        }
    }
    #[test]
    fn expired_logs_are_removed_and_disk_failures_remain_visible() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-log-error-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut store = LogStore::new(root.clone(), "test".into()).unwrap();
        let expired = root.join("application-20000101-old.jsonl");
        fs::write(&expired, "old").unwrap();
        fs::File::options()
            .write(true)
            .open(&expired)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(15 * 86_400),
            )
            .unwrap();
        store.prune().unwrap();
        assert!(!expired.exists());
        let blocked = root.join("blocked");
        fs::write(&blocked, "do not overwrite").unwrap();
        store.directory = blocked.clone();
        assert!(store
            .write(&serde_json::json!({"event":"failure"}), true)
            .is_err());
        assert!(store.last_error.is_some());
        assert_eq!(fs::read_to_string(blocked).unwrap(), "do not overwrite");
        store.directory = root.clone();
        store
            .write(&serde_json::json!({"event":"recovered"}), true)
            .unwrap();
        assert!(store.last_error.is_none());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
