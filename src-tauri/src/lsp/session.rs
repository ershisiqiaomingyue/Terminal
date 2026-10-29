//! Resident language-server processes and request coordination.

use super::catalog::ServerConfig;
use super::executable::ResolvedExecutable;
use super::limits::*;
use super::protocol::{client_response, read_json, write_json};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

/// The identity a resident process is reused by. Two requests share a process
/// only when the resolved executable, the full argument vector and the workspace
/// all match; `fingerprint` keeps two server entries that agree on those three
/// (but differ in env or settings) from being served by one process.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct SessionKey {
    program: PathBuf,
    args: Vec<String>,
    workspace: PathBuf,
    fingerprint: String,
}

/// Everything needed to (re)start the resident process behind one session.
struct SpawnPlan {
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    workspace: PathBuf,
    server_id: String,
    settings: Value,
    initialization_options: Value,
    root_uri: String,
}

/// One document this session opened. Kept so a restart can replay `didOpen` and
/// so an unchanged file never gets a redundant notification.
#[derive(Clone)]
struct OpenDocument {
    language_id: String,
    version: i64,
    text: String,
    mtime: Option<std::time::SystemTime>,
    size: u64,
}

/// Why one attempt on one generation produced no result. A vanished process is
/// worth one retry; a JSON-RPC error is the server's final answer.
pub(super) enum RequestFailure {
    /// The process is gone or the pipe broke down.
    Gone(String),
    /// A JSON-RPC error response, or a local protocol failure.
    Response(String),
}

impl RequestFailure {
    pub(super) fn message(self) -> String {
        match self {
            Self::Gone(message) | Self::Response(message) => message,
        }
    }
}

#[derive(Default)]
struct DiagnosticsState {
    /// The newest publish for a uri that arrived before anyone waited for it.
    latest: HashMap<String, Value>,
    waiters: HashMap<String, Vec<oneshot::Sender<Value>>>,
}

/// One resident process: its pipes, its in-flight requests, and the task that
/// pumps its stdout.
pub(super) struct Generation {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    /// In-flight JSON-RPC ids. A response is delivered to exactly the request
    /// that asked for it, whatever order the answers arrive in.
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, RequestFailure>>>>,
    diagnostics: Mutex<DiagnosticsState>,
    capabilities: Mutex<Value>,
    dead: AtomicBool,
    exit_note: Mutex<Option<String>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Generation {
    fn note(&self) -> String {
        self.exit_note
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "LSP 服务器已退出".to_string())
    }

    pub(super) fn capabilities(&self) -> Value {
        self.capabilities.lock().unwrap().clone()
    }

    /// Mark the process gone exactly once and fail everything waiting on it, so
    /// no request can hang until the timeout after a crash.
    fn mark_dead(&self, note: &str) {
        if self.dead.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.exit_note.lock().unwrap() = Some(note.to_string());
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, sender) in pending {
            let _ = sender.send(Err(RequestFailure::Gone(note.to_string())));
        }
        // Dropping the senders fails the diagnostics waiters with the same reason.
        let waiters = std::mem::take(&mut self.diagnostics.lock().unwrap().waiters);
        drop(waiters);
    }

    async fn shutdown(&self) {
        self.mark_dead("LSP 会话已回收");
        *self.stdin.lock().await = None;
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill().await;
        }
        if let Some(reader) = self.reader.lock().unwrap().take() {
            reader.abort();
        }
    }

    fn shutdown_blocking(&self) {
        self.mark_dead("LSP 会话已回收");
        if let Ok(mut stdin) = self.stdin.try_lock() {
            *stdin = None;
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
            child.take();
        }
        if let Ok(mut reader) = self.reader.lock() {
            if let Some(reader) = reader.take() {
                reader.abort();
            }
        }
    }
}

/// Removes the pending entry for one request on every exit path, including a
/// dropped future (outer timeout or cancellation).
struct PendingGuard {
    generation: Arc<Generation>,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.generation.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

struct InFlightGuard(Arc<LspSession>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One resident server process set, keyed by `SessionKey` in `sessions()`.
pub(crate) struct LspSession {
    plan: SpawnPlan,
    generation: RwLock<Option<Arc<Generation>>>,
    /// Serializes process starts so two concurrent first requests share one
    /// process instead of racing into two.
    start_lock: tokio::sync::Mutex<()>,
    /// Every document the current process has been told about.
    opened: tokio::sync::Mutex<BTreeMap<String, OpenDocument>>,
    next_id: AtomicU64,
    last_used: Mutex<Instant>,
    /// Milliseconds; `0` means "idle immediately" and is only used by tests.
    idle_millis: AtomicU64,
    consecutive_restarts: AtomicUsize,
    in_flight: AtomicUsize,
}

impl LspSession {
    fn new(plan: SpawnPlan) -> Self {
        Self {
            plan,
            generation: RwLock::new(None),
            start_lock: tokio::sync::Mutex::new(()),
            opened: tokio::sync::Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            last_used: Mutex::new(Instant::now()),
            idle_millis: AtomicU64::new(configured_idle_timeout().as_millis() as u64),
            consecutive_restarts: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
        }
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// A session with a request in flight is never idle: the reaper must not kill
    /// the process a 45-second request is talking to.
    fn is_idle(&self) -> bool {
        if self.in_flight.load(Ordering::Relaxed) > 0 {
            return false;
        }
        self.last_used.lock().unwrap().elapsed().as_millis() as u64
            >= self.idle_millis.load(Ordering::SeqCst)
    }

    async fn live_generation(&self) -> Option<Arc<Generation>> {
        let generation = self.generation.read().await.as_ref().cloned()?;
        if generation.dead.load(Ordering::SeqCst) {
            None
        } else {
            Some(generation)
        }
    }

    async fn exit_note(&self) -> String {
        let note = self
            .generation
            .read()
            .await
            .as_ref()
            .map(|generation| generation.note());
        note.unwrap_or_else(|| "LSP 服务器已退出".to_string())
    }

    /// Stop the resident process of this session, if one is running.
    async fn shutdown(&self) {
        let generation = self.generation.read().await.as_ref().cloned();
        if let Some(generation) = generation {
            generation.shutdown().await;
        }
    }

    /// Start the process if none is live. A dead process is replaced at most
    /// `MAX_CONSECUTIVE_RESTARTS` times in a row without a successful response.
    pub(super) async fn ensure_started(self: &Arc<Self>) -> Result<Arc<Generation>, String> {
        self.touch();
        if let Some(generation) = self.live_generation().await {
            return Ok(generation);
        }
        let _guard = self.start_lock.lock().await;
        if let Some(generation) = self.live_generation().await {
            return Ok(generation);
        }
        let replacing = self.generation.read().await.is_some();
        if replacing && !self.take_restart_budget() {
            return Err(format!(
                "{}；自动重启已达上限（{MAX_CONSECUTIVE_RESTARTS} 次连续崩溃），请检查语言服务器配置。",
                self.exit_note().await
            ));
        }
        let generation = self.spawn_generation().await?;
        *self.generation.write().await = Some(generation.clone());
        Ok(generation)
    }

    /// One automatic restart after a crash. A concurrent caller that already
    /// restarted the session wins; this is then a no-op.
    pub(super) async fn restart(self: &Arc<Self>, note: &str) -> Result<(), String> {
        let _guard = self.start_lock.lock().await;
        if self.live_generation().await.is_some() {
            return Ok(());
        }
        if !self.take_restart_budget() {
            return Err(format!(
                "{note}；自动重启已达上限（{MAX_CONSECUTIVE_RESTARTS} 次连续崩溃），请检查语言服务器配置。"
            ));
        }
        let generation = self.spawn_generation().await?;
        *self.generation.write().await = Some(generation);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_expire_idle(&self) {
        self.idle_millis.store(0, Ordering::Relaxed);
    }

    fn take_restart_budget(&self) -> bool {
        let mut current = self.consecutive_restarts.load(Ordering::SeqCst);
        loop {
            if current >= MAX_CONSECUTIVE_RESTARTS {
                return false;
            }
            match self.consecutive_restarts.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    async fn spawn_generation(self: &Arc<Self>) -> Result<Arc<Generation>, String> {
        let plan = &self.plan;
        let mut command = Command::new(&plan.program);
        command.current_dir(&plan.workspace).envs(&plan.env);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child: Child = command
            .args(&plan.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                format!(
                    "LSP {} 启动失败（{}）：{error}",
                    plan.server_id,
                    plan.program.display()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "LSP stdin 不可用".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "LSP stdout 不可用".to_string())?;
        let generation = Arc::new(Generation {
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
            pending: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(DiagnosticsState::default()),
            capabilities: Mutex::new(Value::Null),
            dead: AtomicBool::new(false),
            exit_note: Mutex::new(None),
            reader: Mutex::new(None),
        });
        // The reader starts before `initialize` is written, so a server that asks
        // for workspace/configuration while initializing still gets an answer.
        let reader = tokio::spawn(read_loop(
            generation.clone(),
            stdout,
            plan.settings.clone(),
            plan.root_uri.clone(),
        ));
        *generation.reader.lock().unwrap() = Some(reader);
        // `initialize` runs exactly once per process; its capabilities are kept
        // for later requests (they decide pull versus push diagnostics).
        let initialized = self
            .send_on(
                &generation,
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": plan.root_uri,
                    "workspaceFolders": [{"uri": plan.root_uri, "name": "workspace"}],
                    "initializationOptions": plan.initialization_options,
                    "capabilities": {
                        "workspace": {"configuration": true, "workspaceFolders": true, "symbol": {}},
                        "general": {"positionEncodings": ["utf-16"]},
                        "textDocument": {
                            "publishDiagnostics": {},
                            "diagnostic": {},
                            "definition": {},
                            "references": {},
                            "hover": {},
                            "completion": {},
                            "rename": {},
                            "formatting": {},
                            "documentSymbol": {}
                        }
                    }
                }),
            )
            .await
            .map_err(RequestFailure::message)?;
        *generation.capabilities.lock().unwrap() = initialized
            .get("capabilities")
            .cloned()
            .unwrap_or(Value::Null);
        self.notify_on(&generation, "initialized", json!({}))
            .await
            .map_err(RequestFailure::message)?;
        self.notify_on(
            &generation,
            "workspace/didChangeConfiguration",
            json!({"settings": plan.settings}),
        )
        .await
        .map_err(RequestFailure::message)?;
        // After a restart the fresh process gets the same document view without
        // the caller re-opening anything.
        let documents: Vec<(String, OpenDocument)> = self
            .opened
            .lock()
            .await
            .iter()
            .map(|(uri, document)| (uri.clone(), document.clone()))
            .collect();
        for (uri, document) in documents {
            self.notify_on(
                &generation,
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri,
                    "languageId": document.language_id,
                    "version": document.version,
                    "text": document.text
                }}),
            )
            .await
            .map_err(RequestFailure::message)?;
        }
        Ok(generation)
    }

    async fn send_on(
        &self,
        generation: &Arc<Generation>,
        method: &str,
        params: Value,
    ) -> Result<Value, RequestFailure> {
        if generation.dead.load(Ordering::SeqCst) {
            return Err(RequestFailure::Gone(generation.note()));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        generation.pending.lock().unwrap().insert(id, sender);
        let _pending = PendingGuard {
            generation: generation.clone(),
            id,
        };
        {
            let mut stdin = generation.stdin.lock().await;
            let Some(stdin) = stdin.as_mut() else {
                return Err(RequestFailure::Gone(generation.note()));
            };
            write_json(
                stdin,
                &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            )
            .await
            .map_err(|error| RequestFailure::Gone(format!("LSP 服务器已退出：{error}")))?;
        }
        match receiver.await {
            Ok(result) => result,
            Err(_) => Err(RequestFailure::Gone(generation.note())),
        }
    }

    async fn notify_on(
        &self,
        generation: &Arc<Generation>,
        method: &str,
        params: Value,
    ) -> Result<(), RequestFailure> {
        if generation.dead.load(Ordering::SeqCst) {
            return Err(RequestFailure::Gone(generation.note()));
        }
        let mut stdin = generation.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(RequestFailure::Gone(generation.note()));
        };
        write_json(
            stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .await
        .map_err(|error| RequestFailure::Gone(format!("LSP 服务器已退出：{error}")))
    }

    /// Open the file once, then keep it in step with the disk. Only a real change
    /// (size/mtime, confirmed by comparing the text) becomes a `didChange`.
    pub(super) async fn sync_document(
        self: &Arc<Self>,
        uri: &str,
        language_id: &str,
        path: &Path,
    ) -> Result<(), String> {
        let generation = self.ensure_started().await?;
        let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
        let mtime = metadata.modified().ok();
        let size = metadata.len();
        let action = {
            let mut opened = self.opened.lock().await;
            let unchanged = opened
                .get(uri)
                .is_some_and(|document| document.size == size && document.mtime == mtime);
            if unchanged {
                None
            } else {
                let content = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|error| format!("无法读取 LSP 文件 {}：{error}", path.display()))?;
                match opened.get(uri).map(|document| document.version) {
                    // Same text, new mtime: refresh the fingerprint but send no
                    // didChange, so the server's view and ours stay in step.
                    Some(_version) if opened.get(uri).is_some_and(|d| d.text == content) => {
                        if let Some(document) = opened.get_mut(uri) {
                            document.mtime = mtime;
                            document.size = size;
                        }
                        None
                    }
                    Some(version) => {
                        let version = version + 1;
                        opened.insert(
                            uri.to_string(),
                            OpenDocument {
                                language_id: language_id.to_string(),
                                version,
                                text: content.clone(),
                                mtime,
                                size,
                            },
                        );
                        Some((
                            "textDocument/didChange",
                            json!({
                                "textDocument": {"uri": uri, "version": version},
                                "contentChanges": [{"text": content}]
                            }),
                        ))
                    }
                    None => {
                        opened.insert(
                            uri.to_string(),
                            OpenDocument {
                                language_id: language_id.to_string(),
                                version: 1,
                                text: content.clone(),
                                mtime,
                                size,
                            },
                        );
                        Some((
                            "textDocument/didOpen",
                            json!({"textDocument": {
                                "uri": uri,
                                "languageId": language_id,
                                "version": 1,
                                "text": content
                            }}),
                        ))
                    }
                }
            }
        };
        let Some((method, params)) = action else {
            return Ok(());
        };
        match self.notify_on(&generation, method, params).await {
            Ok(()) => Ok(()),
            // The record already holds the new text, so the restart replays it;
            // there is nothing left to send.
            Err(RequestFailure::Gone(note)) => self.restart(&note).await,
            Err(failure) => Err(failure.message()),
        }
    }

    /// Send one request and wait for *its* id. A vanished process costs exactly
    /// one automatic restart.
    pub(super) async fn request(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let _in_flight = InFlightGuard(self.clone());
        self.touch();
        let mut retried = false;
        loop {
            let generation = self.ensure_started().await?;
            match self.send_on(&generation, method, params.clone()).await {
                Ok(value) => {
                    self.touch();
                    self.consecutive_restarts.store(0, Ordering::SeqCst);
                    return Ok(value);
                }
                Err(RequestFailure::Response(message)) => return Err(message),
                Err(RequestFailure::Gone(note)) => {
                    if retried {
                        return Err(format!("{note}；已自动重启一次仍失败"));
                    }
                    retried = true;
                    self.restart(&note).await?;
                }
            }
        }
    }

    /// Wait for the next push diagnostics for one uri, including one that already
    /// arrived for this document after its `didOpen`.
    pub(super) async fn wait_for_diagnostics(
        &self,
        generation: &Arc<Generation>,
        uri: &str,
    ) -> Result<Value, RequestFailure> {
        let receiver = {
            let mut state = generation.diagnostics.lock().unwrap();
            if let Some(value) = state.latest.remove(uri) {
                return Ok(value);
            }
            let (sender, receiver) = oneshot::channel();
            state
                .waiters
                .entry(uri.to_string())
                .or_default()
                .push(sender);
            receiver
        };
        match receiver.await {
            Ok(value) => Ok(value),
            Err(_) => Err(RequestFailure::Gone(generation.note())),
        }
    }
}

fn configured_idle_timeout() -> Duration {
    std::env::var("ANDROID_TOOL_LSP_IDLE_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(LSP_IDLE_TIMEOUT)
}

fn sessions() -> &'static Mutex<HashMap<SessionKey, Arc<LspSession>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<SessionKey, Arc<LspSession>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One background reaper for the whole process; it only ever touches sessions
/// that are idle *and* have no request in flight.
fn ensure_reaper() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            sleep(LSP_REAP_INTERVAL).await;
            reap_idle_sessions().await;
        }
    });
}

/// Kill and forget every session that has been idle past its timeout.
pub(crate) async fn reap_idle_sessions() {
    let expired: Vec<(SessionKey, Arc<LspSession>)> = {
        let registry = sessions().lock().unwrap();
        registry
            .iter()
            .filter(|(_, session)| session.is_idle())
            .map(|(key, session)| (key.clone(), session.clone()))
            .collect()
    };
    for (key, session) in expired {
        // A request may have started since the registry was scanned.
        if !session.is_idle() {
            continue;
        }
        session.shutdown().await;
        sessions().lock().unwrap().remove(&key);
    }
}

/// Terminate every resident server. Called on application exit so no language
/// server outlives the host.
pub fn shutdown_sessions_blocking() {
    let drained: Vec<Arc<LspSession>> = sessions()
        .lock()
        .unwrap()
        .drain()
        .map(|(_, session)| session)
        .collect();
    for session in drained {
        if let Ok(mut generation) = session.generation.try_write() {
            if let Some(generation) = generation.take() {
                generation.shutdown_blocking();
            }
        }
    }
}

#[cfg(test)]
pub(crate) async fn shutdown_sessions() {
    let drained: Vec<Arc<LspSession>> = sessions()
        .lock()
        .unwrap()
        .drain()
        .map(|(_, session)| session)
        .collect();
    for session in drained {
        session.shutdown().await;
    }
}

#[cfg(test)]
pub(crate) fn session_count() -> usize {
    sessions().lock().unwrap().len()
}

#[cfg(test)]
pub(crate) fn test_sessions() -> Vec<Arc<LspSession>> {
    sessions().lock().unwrap().values().cloned().collect()
}

pub(super) async fn session_for(
    server: &ServerConfig,
    resolved: &ResolvedExecutable,
    workspace: &Path,
    root_uri: &str,
) -> Arc<LspSession> {
    // A `.ps1` server runs through its PowerShell host; every other server runs
    // the resolved executable directly.
    let (program, args) = match &resolved.launcher {
        Some(launcher) => {
            let mut args = launcher.args.clone();
            args.extend(server.args.iter().cloned());
            (launcher.program.clone(), args)
        }
        None => (resolved.path.clone(), server.args.clone()),
    };
    let fingerprint = serde_json::to_string(&json!({
        "id": server.id,
        "env": server.env,
        "initializationOptions": server.initialization_options,
        "settings": server.settings,
    }))
    .unwrap_or_default();
    let key = SessionKey {
        program,
        args,
        workspace: workspace.to_path_buf(),
        fingerprint,
    };
    if let Some(session) = sessions().lock().unwrap().get(&key) {
        session.touch();
        return session.clone();
    }
    ensure_reaper();
    let plan = SpawnPlan {
        program: key.program.clone(),
        args: key.args.clone(),
        env: server.env.clone(),
        workspace: workspace.to_path_buf(),
        server_id: server.id.clone(),
        settings: server.settings.clone(),
        initialization_options: server.initialization_options.clone(),
        root_uri: root_uri.to_string(),
    };
    let session = Arc::new(LspSession::new(plan));
    sessions().lock().unwrap().insert(key, session.clone());
    // The process starts lazily on the first request, under the session's start
    // lock, so concurrent calls on one workspace share a single process.
    session
}

/// Pump one generation's stdout: answer server-to-client requests, deliver each
/// response to its own waiting request, and park push diagnostics.
async fn read_loop(
    generation: Arc<Generation>,
    mut stdout: ChildStdout,
    settings: Value,
    root_uri: String,
) {
    loop {
        match read_json(&mut stdout).await {
            Ok(value) => {
                if let Some(response) = client_response(&value, &settings, &root_uri) {
                    let mut stdin = generation.stdin.lock().await;
                    let Some(stdin) = stdin.as_mut() else {
                        break;
                    };
                    if write_json(stdin, &response).await.is_err() {
                        break;
                    }
                    continue;
                }
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let sender = generation.pending.lock().unwrap().remove(&id);
                    if let Some(sender) = sender {
                        let reply = match value.get("error") {
                            Some(error) => {
                                Err(RequestFailure::Response(format!("LSP 请求被拒绝：{error}")))
                            }
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = sender.send(reply);
                    }
                    continue;
                }
                if value.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                {
                    if let Some(uri) = value["params"]["uri"].as_str() {
                        let diagnostics = value["params"]["diagnostics"].clone();
                        publish_diagnostics(&generation, uri, diagnostics);
                    }
                }
            }
            Err(error) => {
                generation.mark_dead(&format!("LSP 服务器已退出：{error}"));
                return;
            }
        }
    }
    generation.mark_dead("LSP 服务器已退出：响应管道已关闭");
}

fn publish_diagnostics(generation: &Generation, uri: &str, diagnostics: Value) {
    let waiters = {
        let mut state = generation.diagnostics.lock().unwrap();
        let waiters = state.waiters.remove(uri).unwrap_or_default();
        if waiters.is_empty() {
            // Nobody is waiting yet; keep it for the next wait on this document.
            state.latest.insert(uri.to_string(), diagnostics.clone());
        }
        waiters
    };
    for waiter in waiters {
        let _ = waiter.send(diagnostics.clone());
    }
}
