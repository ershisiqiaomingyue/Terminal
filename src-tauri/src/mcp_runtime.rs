use agent_core::{
    mcp_clear_session, mcp_list_tools, ConnectionStatus, McpHealth, McpServerConfig, McpTool,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

use agent_core::AgentConnectionView;

const MCP_CONFIG_FILE: &str = "mcp-servers.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredMcpProfiles {
    profiles: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInput {
    pub id: String,
    pub transport: agent_core::McpTransport,
    pub enabled: bool,
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: u64,
}

impl From<McpServerInput> for McpServerConfig {
    fn from(value: McpServerInput) -> Self {
        Self {
            id: value.id,
            transport: value.transport,
            enabled: value.enabled,
            timeout_secs: value.timeout_secs,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerView {
    pub id: String,
    pub enabled: bool,
    pub status: McpHealth,
    pub tools: Vec<McpTool>,
    pub error: Option<String>,
    #[serde(rename = "lastCheckedAt")]
    pub last_checked_at: Option<i64>,
}

struct McpServerEntry {
    attempt: u64,
    config: McpServerConfig,
    view: McpServerView,
}

#[derive(Clone)]
pub struct ConnectedMcpTool {
    pub server_id: String,
    pub config: McpServerConfig,
    pub tool: McpTool,
}

#[derive(Default)]
pub struct McpRuntimeState {
    servers: Mutex<BTreeMap<String, McpServerEntry>>,
    next_attempt: AtomicU64,
}

fn view_for(entry: &McpServerEntry) -> McpServerView {
    entry.view.clone()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(MCP_CONFIG_FILE))
        .map_err(|error| format!("无法定位 MCP 配置目录: {error}"))
}

fn load_profiles(app: &AppHandle) -> Result<Vec<McpServerConfig>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("无法读取 MCP 配置: {error}"))?;
    let stored = serde_json::from_str::<StoredMcpProfiles>(&content)
        .map_err(|error| format!("MCP 配置格式无效: {error}"))?;
    for profile in &stored.profiles {
        profile
            .validate()
            .map_err(|error| format!("MCP 配置无效: {error}"))?;
    }
    Ok(stored.profiles)
}

fn save_profiles(app: &AppHandle, profiles: &[McpServerConfig]) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "MCP 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 MCP 配置目录: {error}"))?;
    let content = serde_json::to_vec_pretty(&StoredMcpProfiles {
        profiles: profiles.to_vec(),
    })
    .map_err(|error| format!("无法序列化 MCP 配置: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, content).map_err(|error| format!("无法写入 MCP 配置: {error}"))?;
    fs::rename(temporary, path).map_err(|error| format!("无法提交 MCP 配置: {error}"))
}

fn save_profile(app: &AppHandle, config: &McpServerConfig) -> Result<(), String> {
    let mut profiles = load_profiles(app)?;
    profiles.retain(|profile| profile.id != config.id);
    profiles.push(config.clone());
    profiles.sort_by(|left, right| left.id.cmp(&right.id));
    save_profiles(app, &profiles)
}

fn hydrate_profiles(app: &AppHandle, state: &McpRuntimeState) -> Result<(), String> {
    let profiles = load_profiles(app)?;
    let mut servers = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?;
    for config in profiles {
        let id = config.id.clone();
        servers.entry(id.clone()).or_insert_with(|| McpServerEntry {
            attempt: 0,
            config: config.clone(),
            view: McpServerView {
                id,
                enabled: config.enabled,
                status: McpHealth::Disconnected,
                tools: Vec::new(),
                error: None,
                last_checked_at: None,
            },
        });
    }
    Ok(())
}

pub fn connected_mcp_tools(state: &McpRuntimeState) -> Result<Vec<ConnectedMcpTool>, String> {
    let servers = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?;
    Ok(servers
        .values()
        .filter(|entry| {
            matches!(entry.view.status, McpHealth::Connected { .. }) && entry.config.enabled
        })
        .flat_map(|entry| {
            entry.view.tools.iter().cloned().map({
                let server_id = entry.config.id.clone();
                let config = entry.config.clone();
                move |tool| ConnectedMcpTool {
                    server_id: server_id.clone(),
                    config: config.clone(),
                    tool,
                }
            })
        })
        .collect())
}

#[tauri::command]
pub async fn connect_mcp_server(
    app: AppHandle,
    state: State<'_, McpRuntimeState>,
    input: McpServerInput,
) -> Result<McpServerView, String> {
    let config: McpServerConfig = input.into();
    config.validate().map_err(|error| error.to_string())?;
    save_profile(&app, &config)?;
    let id = config.id.clone();
    let attempt = state.next_attempt.fetch_add(1, Ordering::Relaxed);
    state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?
        .insert(
            id.clone(),
            McpServerEntry {
                attempt,
                config: config.clone(),
                view: McpServerView {
                    id: id.clone(),
                    enabled: config.enabled,
                    status: McpHealth::Checking,
                    tools: Vec::new(),
                    error: None,
                    last_checked_at: Some(now_ms()),
                },
            },
        );
    let tools = match mcp_list_tools(&config).await {
        Ok(tools) => tools,
        Err(error) => {
            let view = McpServerView {
                id: id.clone(),
                enabled: config.enabled,
                status: McpHealth::Error {
                    message: error.clone(),
                },
                tools: Vec::new(),
                error: Some(error),
                last_checked_at: Some(now_ms()),
            };
            let mut servers = state
                .servers
                .lock()
                .map_err(|_| "MCP 状态不可用".to_string())?;
            if servers
                .get(&id)
                .is_some_and(|entry| entry.attempt == attempt)
            {
                servers.insert(
                    id,
                    McpServerEntry {
                        attempt,
                        config,
                        view: view.clone(),
                    },
                );
            } else if let Some(current) = servers.get(&id) {
                return Ok(current.view.clone());
            } else {
                return Ok(McpServerView {
                    id: id.clone(),
                    enabled: false,
                    status: McpHealth::Disconnected,
                    tools: Vec::new(),
                    error: None,
                    last_checked_at: None,
                });
            }
            return Ok(view);
        }
    };
    let view = McpServerView {
        id: id.clone(),
        enabled: config.enabled,
        status: McpHealth::Connected {
            tool_count: tools.len(),
        },
        tools,
        error: None,
        last_checked_at: Some(now_ms()),
    };
    let mut servers = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?;
    if servers
        .get(&id)
        .is_some_and(|entry| entry.attempt == attempt)
    {
        servers.insert(
            id,
            McpServerEntry {
                attempt,
                config,
                view: view.clone(),
            },
        );
    } else if let Some(current) = servers.get(&id) {
        return Ok(current.view.clone());
    } else {
        return Ok(McpServerView {
            id: id.clone(),
            enabled: false,
            status: McpHealth::Disconnected,
            tools: Vec::new(),
            error: None,
            last_checked_at: None,
        });
    }
    Ok(view)
}

#[tauri::command]
pub fn list_mcp_servers(
    app: AppHandle,
    state: State<'_, McpRuntimeState>,
) -> Result<Vec<McpServerView>, String> {
    hydrate_profiles(&app, &state)?;
    Ok(state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?
        .values()
        .map(view_for)
        .collect())
}

#[tauri::command]
pub fn disconnect_mcp_server(
    app: AppHandle,
    state: State<'_, McpRuntimeState>,
    server_id: String,
) -> Result<(), String> {
    let runtime_config = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?
        .remove(&server_id)
        .map(|entry| entry.config);
    let config = runtime_config.or_else(|| {
        load_profiles(&app)
            .ok()
            .and_then(|profiles| profiles.into_iter().find(|profile| profile.id == server_id))
    });
    let Some(config) = config else {
        return Err(format!("未找到 MCP 配置: {server_id}"));
    };
    mcp_clear_session(&config);
    Ok(())
}

#[tauri::command]
pub fn remove_mcp_server(
    app: AppHandle,
    state: State<'_, McpRuntimeState>,
    server_id: String,
) -> Result<(), String> {
    let config = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?
        .remove(&server_id)
        .map(|entry| entry.config);
    let mut profiles = load_profiles(&app)?;
    let before = profiles.len();
    profiles.retain(|profile| profile.id != server_id);
    if config.is_none() && before == profiles.len() {
        return Err(format!("未找到 MCP 配置: {server_id}"));
    }
    save_profiles(&app, &profiles)?;
    if let Some(config) = config {
        mcp_clear_session(&config);
    }
    Ok(())
}

#[tauri::command]
pub fn get_mcp_connection_status(
    app: AppHandle,
    state: State<'_, McpRuntimeState>,
) -> Result<AgentConnectionView, String> {
    hydrate_profiles(&app, &state)?;
    let servers = state
        .servers
        .lock()
        .map_err(|_| "MCP 状态不可用".to_string())?;
    let connected_count = servers
        .values()
        .filter(|entry| matches!(entry.view.status, McpHealth::Connected { .. }))
        .count();
    let status = if connected_count > 0 {
        ConnectionStatus::connected(format!("{} 个 MCP server 已连接", connected_count))
    } else if servers
        .values()
        .any(|entry| matches!(entry.view.status, McpHealth::Checking))
    {
        ConnectionStatus::Checking
    } else if servers.is_empty() {
        ConnectionStatus::NotConfigured
    } else {
        ConnectionStatus::error("没有可用的 MCP server")
    };
    Ok(AgentConnectionView {
        id: "mcp".to_string(),
        name: "MCP".to_string(),
        status,
    })
}

pub fn restore_mcp_profiles(app: &AppHandle, state: &McpRuntimeState) -> Result<(), String> {
    hydrate_profiles(app, state)
}
