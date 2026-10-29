use super::process::{self, Runtime};
use agent_core::{ToolDefinition, ToolHandler, ToolRegistry};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

#[derive(Clone)]
struct Binding {
    runtime: Arc<Runtime>,
    scope: String,
    definitions: Vec<(String, ToolDefinition)>,
    prompt: String,
}
type TurnKey = (String, String);
static BINDINGS: OnceLock<Mutex<HashMap<TurnKey, Vec<Binding>>>> = OnceLock::new();
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);
fn bindings() -> &'static Mutex<HashMap<TurnKey, Vec<Binding>>> {
    BINDINGS.get_or_init(Mutex::default)
}
fn call_id() -> String {
    format!("plugin-call-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed))
}

pub(crate) struct PluginTurn {
    key: TurnKey,
}
struct PendingScope {
    runtime: Arc<Runtime>,
    scope: String,
    retained: bool,
}
impl Drop for PendingScope {
    fn drop(&mut self) {
        if !self.retained {
            self.runtime
                .send(json!({"type":"release", "scope":self.scope}));
        }
    }
}
impl Drop for PluginTurn {
    fn drop(&mut self) {
        if let Some(items) = bindings()
            .lock()
            .ok()
            .and_then(|mut cache| cache.remove(&self.key))
        {
            for item in items {
                item.runtime
                    .send(json!({"type":"release", "scope":item.scope}));
            }
        }
    }
}
impl PluginTurn {
    pub(crate) fn prompt(&self, windows_default: bool) -> String {
        let Ok(cache) = bindings().lock() else {
            return String::new();
        };
        cache
            .get(&self.key)
            .into_iter()
            .flatten()
            .filter(|item| item.runtime.active())
            .map(|item| {
                let mut prompt = item.prompt.clone();
                // One regex pass prevents replacements inside names that were just
                // prefixed, and preserves plugin-owned guidance without adapters.
                let names = item
                    .definitions
                    .iter()
                    .map(|(name, definition)| {
                        (
                            name.clone(),
                            if windows_default {
                                definition.name.clone()
                            } else {
                                format!("windows__{}", definition.name)
                            },
                        )
                    })
                    .collect::<HashMap<_, _>>();
                if !names.is_empty() {
                    let pattern = format!(
                        r"\b({})\b",
                        names
                            .keys()
                            .map(|name| regex::escape(name))
                            .collect::<Vec<_>>()
                            .join("|")
                    );
                    if let Ok(re) = regex::Regex::new(&pattern) {
                        prompt = re
                            .replace_all(&prompt, |capture: &regex::Captures<'_>| {
                                names[&capture[0]].clone()
                            })
                            .into_owned();
                    }
                }
                format!("\n[Windows 本机插件工具；通过当前对话的权限流程调用]\n{prompt}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(crate) async fn prepare_turn(
    session: &str,
    turn: &str,
    roots: &[String],
    mode: &str,
) -> PluginTurn {
    let key = (session.into(), turn.into());
    let guard = PluginTurn { key: key.clone() };
    let Some(cwd) = agent_core::selected_working_directory(roots) else {
        return guard;
    };
    // The app's ask/risk modes permit approved workspace writes. They do not
    // provide an OS confinement service; plugins requiring that service keep
    // their own shell layer hidden until the user selects full access.
    let policy = if mode == "full" {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let scope = format!("{}:{}:{}", session, turn, call_id());
    if let Ok(mut cache) = bindings().lock() {
        cache.insert(key.clone(), Vec::new());
    }
    for (id, runtime) in process::connections() {
        let mut pending = PendingScope {
            runtime: runtime.clone(),
            scope: scope.clone(),
            retained: false,
        };
        let request_id = call_id();
        let request = runtime.rpc(
            "prepare",
            json!({"scope":scope,"sessionId":session,"cwd":cwd,"mode":policy}),
            &request_id,
        );
        let snapshot = match tokio::time::timeout(Duration::from_secs(5), request).await {
            Ok(Ok(snapshot)) => snapshot,
            error => {
                let code = if matches!(error, Err(_)) {
                    "plugin_prepare_timeout"
                } else {
                    "plugin_prepare_failed"
                };
                crate::diagnostics::record(
                    "warn",
                    "plugin.tools.prepare_failed",
                    "native.plugin_host",
                    json!({"plugin_id":id,"session_id":session,"turn_id":turn,"code":code}),
                );
                continue;
            }
        };
        let definitions = snapshot["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let name = row["name"].as_str()?;
                let schema = row.get("parameters")?.clone();
                Some((
                    name.into(),
                    ToolDefinition::new(
                        agent_core::plugin_tool_name(&id, name),
                        format!(
                            "[Windows 插件] {}",
                            row["description"].as_str().unwrap_or(name)
                        ),
                        schema,
                    )
                    .requiring_permission(),
                ))
            })
            .collect();
        if let Ok(mut cache) = bindings().lock() {
            cache.entry(key.clone()).or_default().push(Binding {
                runtime,
                scope: scope.clone(),
                definitions,
                prompt: snapshot["prompt"].as_str().unwrap_or_default().into(),
            });
            pending.retained = true;
        }
    }
    guard
}

pub(crate) fn register_tools(
    registry: &mut ToolRegistry,
    session: &str,
    turn: &str,
    cancellation: Arc<AtomicBool>,
) -> Result<(), String> {
    let items = bindings()
        .lock()
        .map_err(|e| e.to_string())?
        .get(&(session.into(), turn.into()))
        .cloned()
        .unwrap_or_default();
    for binding in items.into_iter().filter(|item| item.runtime.active()) {
        for (name, definition) in &binding.definitions {
            registry
                .register_handler(PluginTool {
                    runtime: binding.runtime.clone(),
                    scope: binding.scope.clone(),
                    name: name.clone(),
                    definition: definition.clone(),
                    cancellation: cancellation.clone(),
                })
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
struct CancelCall {
    runtime: Arc<Runtime>,
    id: String,
}
impl Drop for CancelCall {
    fn drop(&mut self) {
        self.runtime.send(json!({"type":"cancel","id":self.id}));
    }
}
struct PluginTool {
    runtime: Arc<Runtime>,
    scope: String,
    name: String,
    definition: ToolDefinition,
    cancellation: Arc<AtomicBool>,
}
#[async_trait]
impl ToolHandler for PluginTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".into());
        }
        // Host-only bookkeeping must never reach the plugin: its argument schema
        // is strict and rejects unknown keys ("invalid arguments: unknown key
        // "__agent_call_id""), which failed every plugin call.
        let mut input = input;
        agent_core::strip_host_tool_bookkeeping(&mut input);
        if input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(agent_core::is_forbidden_destructive_command_line)
        {
            return Err(agent_core::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.into());
        }
        let cancel = CancelCall {
            runtime: self.runtime.clone(),
            id: call_id(),
        };
        let result = tokio::select! {
            result = self.runtime.rpc("call", json!({"scope":self.scope,"name":self.name,"arguments":input}), &cancel.id) => result?,
            _ = async { while !self.cancellation.load(Ordering::Relaxed) { tokio::time::sleep(Duration::from_millis(25)).await; } } => return Err("请求已取消".into()),
        };
        // The renderer provides a rendered message and its structured value. Forward the
        // value once to avoid doubling file/search output and token consumption.
        Ok(agent_core::plugin_tool_output(result))
    }
}
