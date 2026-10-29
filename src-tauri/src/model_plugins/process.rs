use super::*;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use tauri::Emitter;

#[derive(Clone, Default, Deserialize)]
pub(super) struct Snapshot {
    #[serde(default)]
    pub accounts: Vec<AccountView>,
    #[serde(default)]
    pub providers: Vec<RuntimeProvider>,
}
#[derive(Clone, Deserialize)]
pub(super) struct RuntimeProvider {
    #[serde(flatten)]
    pub settings: AgentProviderSettings,
    pub token: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[serde(default)]
    provider_id: String,
    #[serde(default)]
    reasoning_models: Vec<ReasoningModelView>,
    id: String,
    name: String,
    state: String,
    account: String,
    model_count: usize,
    credits: Option<f64>,
    unlimited: bool,
    message: String,
    credit_error: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningModelView {
    id: String,
    name: String,
    icon_brand: String,
    supports_reasoning: bool,
    efforts: Vec<String>,
    source: String,
    validation: String,
    probed_at: Option<u64>,
    can_probe: bool,
    probing: bool,
    message: String,
}
type Reply = Result<Value, String>;
type Pending = Arc<Mutex<HashMap<u64, mpsc::SyncSender<Reply>>>>;
struct PluginProcess {
    child: Mutex<Child>,
    input: Mutex<ChildStdin>,
    active: Arc<AtomicBool>,
    pending: Pending,
}
/// Monotonic JSON-RPC request id, unique for the app's lifetime so a reply can
/// never be matched against a request from a previous plugin process.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// One process slot per plugin spec, so a new plugin needs no new static. An id
/// with no spec row gets its own fallback slot instead of index 0, which would
/// otherwise alias that unknown id onto workbuddy's live process.
fn slots() -> &'static [Mutex<Option<Arc<PluginProcess>>>] {
    static SLOTS: OnceLock<Vec<Mutex<Option<Arc<PluginProcess>>>>> = OnceLock::new();
    SLOTS.get_or_init(|| super::spec::ids().map(|_| Mutex::default()).collect())
}
fn slot(id: &str) -> &'static Mutex<Option<Arc<PluginProcess>>> {
    static FALLBACK: OnceLock<Mutex<Option<Arc<PluginProcess>>>> = OnceLock::new();
    match super::spec::ids().position(|candidate| candidate == id) {
        Some(index) => &slots()[index],
        None => FALLBACK.get_or_init(Mutex::default),
    }
}
fn notify(app: &AppHandle) {
    let _ = app.emit("model-plugins-changed", ());
}
fn clear_pending(pending: &Pending) {
    if let Ok(mut requests) = pending.lock() {
        for (_, reply) in requests.drain() {
            let _ = reply.send(Err("插件进程已停止，请刷新重试".into()));
        }
    }
}
pub(super) fn is_running(id: &str) -> bool {
    let Ok(slot) = slot(id).lock() else {
        return false;
    };
    slot.as_ref()
        .is_some_and(|process| process.active.load(Ordering::Acquire))
}

pub(super) fn start(app: &AppHandle, config: &PluginConfig) -> Result<(), String> {
    let spec = super::spec::require(&config.id)?;
    let id = spec.id;
    if is_running(id) {
        return Ok(());
    }
    stop(app, id);
    let directory = install_directory(app, config)?;
    // The install layout belongs to the runtime, so ask the pipeline instead of
    // assuming npm's node_modules tree.
    let language = crate::plugin_runtime::Language::primary(required_environment_tools(id))?;
    if !crate::plugin_runtime::is_installed(language, &directory, spec.package) {
        return Err("插件文件缺失，请重新安装".into());
    }
    let pipeline = installer::pipeline(app, id)?;
    let bridge = directory.join("android-tool-bridge.mjs");
    fs::write(
        &bridge,
        spec.bridge,
    )
    .map_err(|e| e.to_string())?;
    let data = root(app, id)?.join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    // The entry point runs on the runtime the plugin declared; the pipeline
    // owns the interpreter, its arguments and the working directory.
    let mut child = pipeline
        .launch_command(&directory, &bridge)?
        .arg(&directory)
        .arg(&data)
        .arg(serde_json::to_string(config).map_err(|e| e.to_string())?)
        .env("DSH_HOME", &data)
        .env("NODE_USE_ENV_PROXY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动模型插件：{e}"))?;
    let input = child.stdin.take().ok_or("插件输入管道不可用")?;
    let output = child.stdout.take().ok_or("插件输出管道不可用")?;
    if let Some(error_output) = child.stderr.take() {
        // Drain diagnostics without publishing potentially sensitive third-party output.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(error_output), &mut std::io::sink());
        });
    }
    let active = Arc::new(AtomicBool::new(true));
    let pending = Pending::default();
    let process = Arc::new(PluginProcess {
        child: Mutex::new(child),
        input: Mutex::new(input),
        active: active.clone(),
        pending: pending.clone(),
    });
    *slot(id).lock().map_err(|e| e.to_string())? = Some(process);
    let app_reader = app.clone();
    let (ready, waiter) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut ready = Some(ready);
        for line in BufReader::new(output).lines() {
            if !active.load(Ordering::Acquire) {
                break;
            }
            let Ok(line) = line else {
                break;
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["type"] == "ready" {
                if let Some(sender) = ready.take() {
                    let _ = sender.send(());
                }
            } else if message["type"] == "snapshot" {
                if let Ok(snapshot) = serde_json::from_value::<Snapshot>(message["data"].clone()) {
                    // Endpoint and model changes invalidate selectors; credit updates don't.
                    if let Ok(mut current) = live(id).lock() {
                        let signature = |value: &Snapshot| {
                            serde_json::to_string(
                                &value
                                    .providers
                                    .iter()
                                    .map(|p| &p.settings)
                                    .collect::<Vec<_>>(),
                            )
                            .unwrap_or_default()
                        };
                        let changed = signature(&current) != signature(&snapshot);
                        let accounts_changed = serde_json::to_string(&current.accounts).ok()
                            != serde_json::to_string(&snapshot.accounts).ok();
                        if active.load(Ordering::Acquire) {
                            *current = snapshot;
                            if changed {
                                notify(&app_reader);
                            }
                            if accounts_changed {
                                let _ = app_reader.emit("model-plugin-state-changed", ());
                            }
                        }
                    }
                }
            } else if let Some(id) = message["id"].as_u64() {
                if let Ok(mut requests) = pending.lock() {
                    if let Some(sender) = requests.remove(&id) {
                        let reply = if let Some(error) = message["error"].as_str() {
                            Err(error.to_owned())
                        } else {
                            Ok(message["data"].clone())
                        };
                        let _ = sender.send(reply);
                    }
                }
            }
        }
        if active.swap(false, Ordering::AcqRel) {
            if let Ok(mut current) = live(id).lock() {
                *current = Snapshot::default();
            }
            clear_pending(&pending);
            notify(&app_reader);
        }
    });
    if waiter.recv_timeout(Duration::from_secs(180)).is_err() {
        stop(app, id);
        return Err("插件启动失败或模型目录同步超时，请检查网络及账号登录状态".into());
    }
    notify(app);
    Ok(())
}

pub(super) fn request(id: &str, mut message: Value) -> Reply {
    let process = slot(id)
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("插件尚未启动")?;
    if !process.active.load(Ordering::Acquire) {
        return Err("插件已停止，请刷新重试".into());
    }
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    message["id"] = Value::from(id);
    let (sender, receiver) = mpsc::sync_channel(1);
    process
        .pending
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id, sender);
    let write = (|| {
        let mut input = process.input.lock().map_err(|e| e.to_string())?;
        writeln!(input, "{}", message)
            .and_then(|_| input.flush())
            .map_err(|e| e.to_string())
    })();
    let response = match write {
        Ok(()) => receiver
            .recv_timeout(Duration::from_secs(180))
            .map_err(|_| "插件请求超时，请稍后刷新".to_string())
            .and_then(|value| value),
        Err(error) => Err(error),
    };
    if let Ok(mut pending) = process.pending.lock() {
        pending.remove(&id);
    }
    response
}

pub(super) fn stop(app: &AppHandle, id: &str) {
    let process = slot(id).lock().ok().and_then(|mut slot| slot.take());
    if let Some(process) = process {
        process.active.store(false, Ordering::Release);
        if let Ok(mut child) = process.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        clear_pending(&process.pending);
    }
    if let Ok(mut current) = live(id).lock() {
        *current = Snapshot::default();
    }
    notify(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every plugin in the table owns a distinct slot.
    #[test]
    fn spec_rows_get_distinct_slots() {
        let ids: Vec<_> = super::super::spec::ids().collect();
        for (index, id) in ids.iter().enumerate() {
            for other in ids.iter().skip(index + 1) {
                assert!(!std::ptr::eq(slot(id), slot(other)), "{id} aliases {other}");
            }
        }
    }

    /// Before this, an id with no spec row fell back to index 0 via
    /// `position(..).unwrap_or_default()`, i.e. straight onto workbuddy's slot:
    /// a stray id could then report or stop workbuddy's live process.
    #[test]
    fn unknown_ids_do_not_land_on_a_plugin_slot() {
        let unknown = slot("no-such-plugin");
        for id in super::super::spec::ids() {
            assert!(!std::ptr::eq(unknown, slot(id)), "unknown id aliases {id}");
        }
        // The fallback is stable, so nothing is rebuilt (or leaked) per lookup.
        assert!(std::ptr::eq(unknown, slot("no-such-plugin")));
    }

    /// Request ids must be monotonic, or a late reply from a previous plugin
    /// process could be matched against a request of the current one.
    #[test]
    fn request_ids_are_unique() {
        let first = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let second = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        assert!(second > first);
    }
}
