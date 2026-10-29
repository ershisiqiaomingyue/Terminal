use super::*;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

pub(super) struct Runtime {
    child: Mutex<Child>,
    sender: mpsc::Sender<String>,
    active: Arc<AtomicBool>,
    url: String,
    key: String,
    has_client: bool,
    client: reqwest::Client,
    pub(super) providers: Arc<Mutex<Vec<super::providers::HostedProvider>>>,
}
impl Runtime {
    pub(super) fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub(super) fn send(&self, message: serde_json::Value) {
        let _ = self.sender.send(message.to_string());
    }
    pub(super) async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
        id: &str,
    ) -> Result<serde_json::Value, String> {
        if !self.active() {
            return Err("插件已停止，请重新启用插件后重试".into());
        }
        let origin = self.url.split("/_host/").next().ok_or("插件地址无效")?;
        let response = self
            .client
            .post(format!("{origin}/_host/rpc"))
            .header("x-plugin-key", &self.key)
            .json(&serde_json::json!({"method":method,"params":params,"id":id}))
            .send()
            .await
            .map_err(|e| format!("插件调用失败：{e}"))?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("插件返回无效数据：{e}"))?;
        if !status.is_success() {
            return Err(body["error"].as_str().unwrap_or("插件调用失败").into());
        }
        Ok(body)
    }
}
pub(super) fn connections() -> Vec<(String, Arc<Runtime>)> {
    let mut result = processes()
        .lock()
        .map(|items| {
            items
                .iter()
                .filter(|(_, runtime)| runtime.active())
                .map(|(id, runtime)| (id.clone(), runtime.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
static PROCESSES: OnceLock<Mutex<HashMap<String, Arc<Runtime>>>> = OnceLock::new();
fn processes() -> &'static Mutex<HashMap<String, Arc<Runtime>>> {
    PROCESSES.get_or_init(Mutex::default)
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
pub(super) fn connection(id: &str) -> Option<Plugin> {
    let processes = processes().lock().ok()?;
    let runtime = processes
        .get(id)
        .filter(|r| r.active.load(Ordering::Acquire))?;
    Some(Plugin {
        id: id.into(),
        name: String::new(),
        source: String::new(),
        version: String::new(),
        description: String::new(),
        enabled: true,
        required_environment_tools: vec![],
        install_directory: String::new(),
        status: "running".into(),
        message: String::new(),
        frame_url: runtime.has_client.then(|| runtime.url.clone()),
        bridge_key: runtime.has_client.then(|| runtime.key.clone()),
        settings_inline: false,
    })
}
pub(super) fn start(app: &AppHandle, plugin: &Plugin) -> Result<(), String> {
    if connection(&plugin.id).is_some() {
        return Ok(());
    }
    stop(&plugin.id);
    let base = directory(app, &plugin.id)?;
    if plugin.install_directory.is_empty()
        || !plugin.install_directory.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("插件安装目录无效".into());
    }
    let data = base.join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let pipeline = installation::host_pipeline(&plugin.required_environment_tools)?;
    let plugin = installation::upgrade_local(app, plugin)?;
    let path = base.join("versions").join(&plugin.install_directory);
    installation::prepare_runtime(&path, &pipeline, &plugin.name)?;
    let descriptor: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join("descriptor.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let has_client = descriptor["client"].is_string();
    let key = {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}:{}:{}",
                    plugin.id,
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                    std::process::id()
                )
                .as_bytes()
            )
        )
    };
    let log = fs::File::create(base.join("runtime.log")).map_err(|e| e.to_string())?;
    let mut child = pipeline
        .launch_command(&path, &path.join("server.mjs"))?
        .arg(&path)
        .arg(&data)
        .arg(&key)
        .env("DSH_HOME", &data)
        .env("NODE_USE_ENV_PROXY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("无法启动插件：{e}"))?;
    let mut input = child.stdin.take().ok_or("插件输入管道不可用")?;
    let output = child.stdout.take().ok_or("插件输出管道不可用")?;
    let (ready_send, ready_recv) = mpsc::sync_channel(1);
    let app_clone = app.clone();
    let id = plugin.id.clone();
    let base_clone = base.clone();
    let active = Arc::new(AtomicBool::new(true));
    let reader_active = active.clone();
    let providers = Arc::new(Mutex::new(Vec::new()));
    let reader_providers = providers.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                if value["type"] == "ready" {
                    let _ = ready_send
                        .try_send(value["origin"].as_str().unwrap_or_default().to_string());
                } else if value["type"] == "providers" && reader_active.load(Ordering::Acquire) {
                    // An invalid replacement clears stale models instead of retaining a previous account.
                    let next = super::providers::decode(&id, value["providers"].clone())
                        .unwrap_or_default();
                    if let Ok(mut current) = reader_providers.lock() {
                        *current = next;
                    }
                    let _ = app_clone.emit("model-plugins-changed", ());
                }
            }
        }
        if !reader_active.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut errors) = failures().lock() {
            let log = fs::read_to_string(base_clone.join("runtime.log")).unwrap_or_default();
            errors.insert(
                id,
                log.lines()
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        changed(&app_clone);
    });
    let origin = match ready_recv.recv_timeout(Duration::from_secs(30)) {
        Ok(origin) if origin.starts_with("http://127.0.0.1:") => origin,
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            let text = fs::read_to_string(base.join("runtime.log")).unwrap_or_default();
            return Err(format!("插件启动失败：{text}"));
        }
    };
    let (sender, receiver) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in receiver {
            if writeln!(input, "{line}").is_err() || input.flush().is_err() {
                break;
            }
        }
    });
    let runtime = Arc::new(Runtime {
        child: Mutex::new(child),
        sender,
        active,
        url: format!("{origin}/_host/frame?key={key}"),
        key,
        has_client,
        providers,
        client: agent_core::http_client_builder()?
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?,
    });
    processes()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(plugin.id.clone(), runtime);
    let _ = app.emit("model-plugins-changed", ());
    Ok(())
}
pub(super) fn stop(id: &str) {
    let runtime = processes()
        .lock()
        .ok()
        .and_then(|mut processes| processes.remove(id));
    if let Some(runtime) = runtime {
        runtime.active.store(false, Ordering::Release);
        let _ = runtime.sender.send("{\"type\":\"shutdown\"}".into());
        if let Ok(mut child) = runtime.child.lock() {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_secs(1) {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
pub(super) fn broadcast(payload: &serde_json::Value) {
    let Ok(processes) = processes().lock() else {
        return;
    };
    if processes.is_empty() {
        return;
    }
    let line = serde_json::json!({"type":"event", "payload":payload}).to_string();
    for runtime in processes
        .values()
        .filter(|r| r.active.load(Ordering::Acquire))
    {
        let _ = runtime.sender.send(line.clone());
    }
}
pub(super) fn shutdown() {
    let ids = processes()
        .lock()
        .map(|p| p.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for id in ids {
        stop(&id);
    }
}
