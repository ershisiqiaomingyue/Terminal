pub mod agent;
pub mod export;
pub mod health;
pub mod store;

use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};
use store::LogStore;
use tauri::{AppHandle, Manager};

static STORE: OnceLock<Mutex<LogStore>> = OnceLock::new();
static EXITED: AtomicBool = AtomicBool::new(false);
struct DiagnosticLogger;
impl log::Log for DiagnosticLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, item: &log::Record<'_>) {
        if self.enabled(item.metadata()) {
            record(
                if item.level() == log::Level::Error {
                    "error"
                } else {
                    "warn"
                },
                "native.library",
                item.target(),
                json!({"message":item.args().to_string()}),
            );
        }
    }
    fn flush(&self) {}
}
static LOGGER: DiagnosticLogger = DiagnosticLogger;

fn fallback_directory() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join(crate::app_identity::identifier())
            .join("logs");
    }
    std::env::temp_dir().join("android-tool-diagnostics")
}

pub fn initialize(directory: Option<PathBuf>) {
    if STORE.get().is_some() {
        return;
    }
    let preferred = directory.unwrap_or_else(fallback_directory);
    let run_id = format!(
        "{}-{}",
        chrono::Utc::now().format("%H%M%S%3f"),
        std::process::id()
    );
    let result = LogStore::new(preferred.clone(), run_id.clone()).or_else(|_| {
        LogStore::new(
            std::env::temp_dir().join("android-tool-diagnostics"),
            run_id,
        )
    });
    match result {
        Ok(store) => {
            let directory = store.directory.clone();
            if STORE.set(Mutex::new(store)).is_err() {
                return;
            }
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
            }
            record(
                "info",
                "application.start",
                "native",
                json!({"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"debug":cfg!(debug_assertions),"logDirectory":directory,"preferredDirectory":preferred}),
            );
            inspect_previous_runs(&directory);
            mark_run(false);
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let message = info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| info.payload().downcast_ref::<&str>().copied())
                    .unwrap_or("native panic");
                let data = json!({"message":message,"location":info.location().map(ToString::to_string),"stack":std::backtrace::Backtrace::force_capture().to_string()});
                // A panic can happen while the logger's mutex is held. Never deadlock the hook.
                if let Some(logger) = STORE.get().and_then(|store| store.try_lock().ok()) {
                    drop(logger);
                    record("error", "native.panic", "native", data);
                } else {
                    let _ = fs::write(
                        directory.join(format!("panic-{}.json", std::process::id())),
                        store::sanitize(&data, 0).to_string(),
                    );
                }
                previous(info);
            }));
        }
        Err(error) => std::eprintln!("诊断日志初始化失败: {error}"),
    }
}

pub fn record(level: &str, event: &str, source: &str, data: Value) {
    if let Err(error) = try_record(level, event, source, data) {
        std::eprintln!("诊断日志写入失败: {error}");
    }
}

pub fn try_record(level: &str, event: &str, source: &str, data: Value) -> Result<(), String> {
    let store = STORE.get().ok_or("诊断日志未初始化")?;
    let mut store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = json!({"schemaVersion":1,"timestamp":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"pid":std::process::id(),"runId":store.run_id,"level":level,"event":event,"source":source,"data":data});
    store
        .write(&value, level == "error" || level == "warn")
        .map_err(|error| error.to_string())
}

pub fn legacy_error(source: &str, message: String) {
    record("warn", "native.error", source, json!({"message":message}));
}

fn marker_path(store: &LogStore) -> PathBuf {
    store.directory.join(format!("run-{}.json", store.run_id))
}

fn mark_run(clean: bool) {
    if let Some(store) = STORE.get().and_then(|store| store.lock().ok()) {
        if let Ok(mut file) = fs::File::create(marker_path(&store)) {
            use std::io::Write;
            let _ = file.write_all(json!({"runId":store.run_id,"pid":std::process::id(),"timestamp":chrono::Utc::now().to_rfc3339(),"cleanExit":clean}).to_string().as_bytes());
            let _ = file.sync_data();
        }
    }
}

fn inspect_previous_runs(directory: &std::path::Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("run-") || !name.ends_with(".json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > 4096 {
            continue;
        }
        let old = metadata
            .modified()
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age.as_secs() > store::RETENTION_DAYS as u64 * 86_400);
        if old {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        let Some(value) = fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        if value["cleanExit"] == false
            && !health::process_alive(value["pid"].as_u64().unwrap_or(0) as u32)
        {
            record("warn", "application.previous_unclean_exit", "native", value);
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn shutdown() {
    if EXITED.swap(true, Ordering::SeqCst) {
        return;
    }
    record(
        "info",
        "application.exit",
        "native",
        json!({"cleanExit":true}),
    );
    mark_run(true);
}

pub fn setup(app: &AppHandle) {
    initialize(app.path().app_log_dir().ok());
    health::start_watchdog(app.clone());
}
