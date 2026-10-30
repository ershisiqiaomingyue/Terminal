use agent_core::{
    disconnect_ssh_connection, ensure_ssh_connected, list_ssh_directories, query_ssh_host_key,
    query_ssh_status, test_ssh_connection, SshConnectionConfig, SshDirectoryEntry, SshHostKeyInfo,
    SshRemoteStatus,
};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

use crate::tasks::TaskManager;

const SSH_CONFIG_FILE: &str = "ssh-connections.json";
const SSH_CREDENTIAL_SERVICE: &str = "android-tool-ssh";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredSshConnections {
    connections: Vec<SshConnectionConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionProgress {
    pub connection_id: String,
    pub task_id: Option<String>,
    /// disconnected | connecting | connected | failed
    pub status: String,
    pub progress: u8,
    pub message: String,
    pub updated_at: i64,
    pub remote_status: Option<SshRemoteStatus>,
    pub error: Option<String>,
}

#[derive(Default, Clone)]
pub struct SshRuntimeState {
    connections: Arc<Mutex<BTreeMap<String, SshConnectionProgress>>>,
    selected_connection_id: Arc<Mutex<Option<String>>>,
}

pub fn selected_connection_for_agent(
    app: &AppHandle,
    runtime: &SshRuntimeState,
) -> Result<Option<SshConnectionConfig>, String> {
    let selected_id = runtime
        .selected_connection_id
        .lock()
        .map_err(|_| "全局 SSH 选择状态不可用".to_string())?
        .clone();
    let Some(selected_id) = selected_id else {
        return Ok(None);
    };
    let progress = connection_progress(runtime, &selected_id)?;
    match progress.as_ref().map(|value| value.status.as_str()) {
        Some("connected") => {
            load_connection_for_agent(app, &selected_id).map(|(config, _)| Some(config))
        }
        Some("connecting") => Err("全局 SSH 正在连接，请等待连接完成后再发送。".to_string()),
        Some("failed") | Some("disconnected") => Ok(None),
        _ => Err("全局 SSH 已断开，请在菜单栏连接后再发送。".to_string()),
    }
}

pub fn connected_connection_for_agent(
    app: &AppHandle,
    runtime: &SshRuntimeState,
    connection_id: &str,
) -> Result<(SshConnectionConfig, Option<String>), String> {
    let connection_id = connection_id.trim();
    if connection_id.is_empty() {
        return Err("SSH 执行目标缺少连接 ID".to_string());
    }
    let progress = connection_progress(runtime, connection_id)?;
    match progress.as_ref().map(|value| value.status.as_str()) {
        Some("connected") => load_connection_for_agent(app, connection_id),
        Some("connecting") => Err("指定的 SSH 正在连接，请等待连接完成后再发送。".to_string()),
        Some("failed") | Some("disconnected") => {
            // An explicit saved target is also allowed to recover on the next
            // operation. The transport layer owns the bounded reconnect.
            load_connection_for_agent(app, connection_id)
        }
        // 没有任何登记记录：终端页签走的是 PTY 直起 ssh.exe 的路径
        // （terminal.rs 的 ssh_command），它不会向 SshRuntimeState 写入进度，
        // 所以这里查不到是正常的。这类显式指定的连接同样是"用户明确要连的
        // 目标"，与 failed/disconnected 同等对待：交给传输层按需重连，
        // 而不是直接报"尚未连接"。真正的连不上会在传输层报错。
        None => load_connection_for_agent(app, connection_id),
        _ => Err("指定的 SSH 尚未连接，请先连接后再发送。".to_string()),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn set_connection_progress(
    state: &SshRuntimeState,
    progress: SshConnectionProgress,
) -> Result<(), String> {
    state
        .connections
        .lock()
        .map_err(|_| "SSH 连接状态不可用".to_string())?
        .insert(progress.connection_id.clone(), progress);
    Ok(())
}

pub(crate) fn connection_progress(
    state: &SshRuntimeState,
    connection_id: &str,
) -> Result<Option<SshConnectionProgress>, String> {
    Ok(state
        .connections
        .lock()
        .map_err(|_| "SSH 连接状态不可用".to_string())?
        .get(connection_id)
        .cloned())
}

pub(crate) fn connected_selection_id(runtime: &SshRuntimeState) -> Option<String> {
    let id = runtime.selected_connection_id.lock().ok()?.clone()?;
    connection_progress(runtime, &id).ok().flatten()
        .filter(|progress| progress.status == "connected").map(|_| id)
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(SSH_CONFIG_FILE))
        .map_err(|error| format!("无法定位 SSH 配置目录: {error}"))
}

fn load_connections(app: &AppHandle) -> Result<Vec<SshConnectionConfig>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("无法读取 SSH 配置: {error}"))?;
    serde_json::from_str::<StoredSshConnections>(&content)
        .map(|stored| {
            stored
                .connections
                .into_iter()
                .map(normalize_connection_root)
                .collect()
        })
        .map_err(|error| format!("SSH 配置格式无效: {error}"))
}

fn normalize_connection_root(mut config: SshConnectionConfig) -> SshConnectionConfig {
    config.default_root = config
        .default_root
        .take()
        .map(|root| root.trim().replace('\\', "/"))
        .filter(|root| !root.is_empty());
    config
}

fn save_connections(app: &AppHandle, connections: &[SshConnectionConfig]) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "SSH 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 SSH 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(&StoredSshConnections {
        connections: connections.to_vec(),
    })
    .map_err(|error| format!("无法序列化 SSH 配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 SSH 配置: {error}"))
}

fn credential(id: &str) -> Result<Entry, String> {
    Entry::new(SSH_CREDENTIAL_SERVICE, id).map_err(|error| format!("无法访问 SSH 凭据库: {error}"))
}

fn saved_password(id: &str) -> Result<Option<String>, String> {
    match credential(id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取 SSH 已保存的密码: {error}")),
    }
}

fn save_password(id: &str, password: &str) -> Result<(), String> {
    let entry = credential(id)?;
    entry
        .set_password(password)
        .map_err(|error| format!("无法保存 SSH 凭据: {error}"))?;
    let persisted = entry
        .get_password()
        .map_err(|error| format!("无法校验 SSH 已保存的密码: {error}"))?;
    if persisted != password {
        return Err("SSH 凭据保存后校验不一致".to_string());
    }
    Ok(())
}

fn delete_credential(id: &str) -> Result<(), String> {
    match credential(id)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除 SSH 凭据: {error}")),
    }
}

pub fn load_connection_for_agent(
    app: &AppHandle,
    id: &str,
) -> Result<(SshConnectionConfig, Option<String>), String> {
    let config = load_connections(app)?
        .into_iter()
        .find(|connection| connection.id == id)
        .ok_or_else(|| "SSH 连接不存在，请先在设置中保存连接".to_string())?;
    let password = saved_password(id)?;
    Ok((config, password))
}

#[tauri::command]
pub fn list_ssh_connections(app: AppHandle) -> Result<Vec<SshConnectionConfig>, String> {
    load_connections(&app)
}

#[tauri::command]
pub fn save_ssh_connection(
    app: AppHandle,
    mut config: SshConnectionConfig,
    password: Option<String>,
) -> Result<SshConnectionConfig, String> {
    config = normalize_connection_root(config);
    config.id = config.id.trim().to_string();
    config.name = config.name.trim().to_string();
    config.host = config.host.trim().to_string();
    config.username = config.username.trim().to_string();
    config.port = if config.port == 0 { 22 } else { config.port };
    if config.id.is_empty()
        || config.name.is_empty()
        || config.host.is_empty()
        || config.username.is_empty()
    {
        return Err("SSH 名称、主机和用户名不能为空".to_string());
    }
    let previous = load_connections(&app)?
        .into_iter()
        .find(|connection| connection.id == config.id);
    if let Some(previous) = &previous {
        disconnect_ssh_connection(previous);
    }
    if let Some(value) = password.filter(|value| !value.trim().is_empty()) {
        save_password(&config.id, &value)?;
    }
    let mut connections = load_connections(&app)?;
    connections.retain(|connection| connection.id != config.id);
    connections.push(config.clone());
    save_connections(&app, &connections)?;
    Ok(config)
}

#[tauri::command]
pub fn delete_ssh_connection(app: AppHandle, id: String) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("SSH 连接 ID 不能为空".to_string());
    }
    let mut connections = load_connections(&app)?;
    let deleted = connections
        .iter()
        .find(|connection| connection.id == id)
        .cloned();
    let previous = connections.len();
    connections.retain(|connection| connection.id != id);
    if connections.len() == previous {
        return Err("SSH 连接不存在".to_string());
    }
    save_connections(&app, &connections)?;
    if let Some(config) = deleted {
        disconnect_ssh_connection(&config);
    }
    delete_credential(id)
}

#[tauri::command]
pub fn clear_ssh_credential(app: AppHandle, id: String) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("SSH 连接 ID 不能为空".to_string());
    }
    let _ = load_connection_for_agent(&app, id)?;
    delete_credential(id)
}

#[tauri::command]
pub async fn test_ssh_connection_command(app: AppHandle, id: String) -> Result<(), String> {
    let (config, password) = load_connection_for_agent(&app, id.trim())?;
    tauri::async_runtime::spawn_blocking(move || test_ssh_connection(&config, password.as_deref()))
        .await
        .map_err(|error| format!("SSH 测试任务失败: {error}"))?
}

#[tauri::command]
pub async fn test_ssh_connection_config(
    config: SshConnectionConfig,
    password: Option<String>,
) -> Result<(), String> {
    let password = match password.filter(|value| !value.trim().is_empty()) {
        Some(password) => Some(password),
        None => saved_password(&config.id)?,
    };
    tauri::async_runtime::spawn_blocking(move || test_ssh_connection(&config, password.as_deref()))
        .await
        .map_err(|error| format!("SSH 测试任务失败: {error}"))?
}

/// Starts a durable SSH connection probe. The actual network work runs off
/// the command thread; callers observe phase progress through this state and
/// the shared task stream.
#[tauri::command]
pub fn start_ssh_connection(
    app: AppHandle,
    runtime: State<'_, SshRuntimeState>,
    tasks: State<'_, TaskManager>,
    id: String,
    path: Option<String>,
) -> Result<SshConnectionProgress, String> {
    begin_ssh_connection(&app, &runtime, tasks.inner(), id, path, true)
}

/// 只建立连接、不改写全局选中项的前端入口（文件预览等按需读取远程文件时使用）。
#[tauri::command]
pub fn connect_ssh_host_command(
    app: AppHandle,
    runtime: State<'_, SshRuntimeState>,
    tasks: State<'_, TaskManager>,
    id: String,
    path: Option<String>,
) -> Result<SshConnectionProgress, String> {
    connect_ssh_host(&app, &runtime, tasks.inner(), &id, path)
}

/// 为对话绑定的附加主机建立连接，但**不改写**全局选中的执行环境。
///
/// 绑定多台主机时若复用 `start_ssh_connection`，会把菜单栏的全局执行环境改成
/// 最后连接的那台；这里只建立传输通道，全局选择仍由用户显式操作决定。
pub(crate) fn connect_ssh_host(
    app: &AppHandle,
    runtime: &SshRuntimeState,
    tasks: &TaskManager,
    id: &str,
    path: Option<String>,
) -> Result<SshConnectionProgress, String> {
    begin_ssh_connection(app, runtime, tasks, id.to_string(), path, false)
}

fn begin_ssh_connection(
    app: &AppHandle,
    runtime: &SshRuntimeState,
    tasks: &TaskManager,
    id: String,
    path: Option<String>,
    select: bool,
) -> Result<SshConnectionProgress, String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("SSH 连接 ID 不能为空".to_string());
    }
    if select {
        *runtime
            .selected_connection_id
            .lock()
            .map_err(|_| "全局 SSH 选择状态不可用".to_string())? = Some(id.clone());
    }
    if let Some(existing) = connection_progress(runtime, &id)? {
        if existing.status == "connecting" {
            return Ok(existing);
        }
    }
    let (config, password) = load_connection_for_agent(app, &id)?;
    let root = path
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config.default_root.clone())
        .unwrap_or_else(|| "~".to_string());
    let task_state = tasks.clone();
    let (task_id, _) = crate::tasks::create_task(
        app,
        &task_state,
        "ssh-connect",
        &id,
        "正在读取 SSH 连接配置",
    )?;
    let initial = SshConnectionProgress {
        connection_id: id.clone(),
        task_id: Some(task_id.clone()),
        status: "connecting".to_string(),
        progress: 8,
        message: "正在读取 SSH 连接配置".to_string(),
        updated_at: now_ms(),
        remote_status: None,
        error: None,
    };
    set_connection_progress(&runtime, initial.clone())?;

    let runtime_state = runtime.clone();
    // 后台任务需要拥有句柄，这里克隆一份；后续同步调用仍可借用 `&app`。
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let update = |progress: u8, message: &str| {
            let _ = crate::tasks::update_task(&app, &task_id, progress, message);
            let _ = set_connection_progress(
                &runtime_state,
                SshConnectionProgress {
                    connection_id: id.clone(),
                    task_id: Some(task_id.clone()),
                    status: "connecting".to_string(),
                    progress,
                    message: message.to_string(),
                    updated_at: now_ms(),
                    remote_status: None,
                    error: None,
                },
            );
        };
        update(22, "正在建立 SSH 认证连接");
        let config_for_test = config.clone();
        let password_for_test = password.clone();
        // 门控语义是「确保可用」而非「校验凭据」：池里已有探活会话就直接放行
        // （v3 §21.4 S0），只有池冷时才付一次全新握手 + 命令验证。
        let verified = tauri::async_runtime::spawn_blocking(move || {
            ensure_ssh_connected(&config_for_test, password_for_test.as_deref())
        })
        .await
        .map_err(|error| format!("SSH 认证任务失败: {error}"))
        .and_then(|result| result);
        if let Err(error) = verified {
            let message = format!("SSH 连接失败：{error}");
            let _ = crate::tasks::finish_task(
                &app,
                &task_state,
                &task_id,
                None,
                Some(&message),
            );
            let _ = set_connection_progress(
                &runtime_state,
                SshConnectionProgress {
                    connection_id: id,
                    task_id: Some(task_id),
                    status: "failed".to_string(),
                    progress: 0,
                    message: message.clone(),
                    updated_at: now_ms(),
                    remote_status: None,
                    error: Some(message),
                },
            );
            return;
        }
        update(68, "认证已通过，正在读取远程状态");
        let config_for_status = config.clone();
        let password_for_status = password.clone();
        let remote_status = tauri::async_runtime::spawn_blocking(move || {
            query_ssh_status(&config_for_status, password_for_status.as_deref(), &root)
        })
        .await
        .map_err(|error| format!("SSH 状态任务失败: {error}"))
        .and_then(|result| result);
        let (message, remote_status, warning) = match remote_status {
            Ok(status) => (
                format!("SSH 已连接 · {} ms", status.latency_ms),
                Some(status),
                None,
            ),
            Err(error) => (
                format!("SSH 已连接，但远程状态读取失败：{error}"),
                None,
                Some(error),
            ),
        };
        let _ = crate::tasks::update_task(&app, &task_id, 96, &message);
        let _ = crate::tasks::finish_task(&app, &task_state, &task_id, None, None);
        let _ = set_connection_progress(
            &runtime_state,
            SshConnectionProgress {
                connection_id: id,
                task_id: Some(task_id),
                status: "connected".to_string(),
                progress: 100,
                message,
                updated_at: now_ms(),
                remote_status,
                error: warning,
            },
        );
        let agent_state = app.state::<agent_core::AgentRuntimeState>();
        for session_id in agent_state.queued_session_ids() {
            crate::agent::dispatch_queued_agent_request(&app, &session_id);
        }
    });
    Ok(initial)
}

#[tauri::command]
pub fn get_ssh_connection_progress(
    app: AppHandle,
    runtime: State<'_, SshRuntimeState>,
    id: String,
) -> Result<SshConnectionProgress, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("SSH 连接 ID 不能为空".to_string());
    }
    if let Some(progress) = connection_progress(&runtime, id)? {
        return Ok(progress);
    }
    let _ = load_connection_for_agent(&app, id)?;
    Ok(SshConnectionProgress {
        connection_id: id.to_string(),
        task_id: None,
        status: "disconnected".to_string(),
        progress: 0,
        message: "尚未连接".to_string(),
        updated_at: now_ms(),
        remote_status: None,
        error: None,
    })
}

#[tauri::command]
pub fn disconnect_ssh_connection_command(
    app: AppHandle,
    runtime: State<'_, SshRuntimeState>,
    id: String,
) -> Result<SshConnectionProgress, String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("SSH 连接 ID 不能为空".to_string());
    }
    let (config, _) = load_connection_for_agent(&app, &id)?;
    disconnect_ssh_connection(&config);
    let was_selected = runtime
        .selected_connection_id
        .lock()
        .map_err(|_| "全局 SSH 选择状态不可用".to_string())?
        .as_deref()
        == Some(id.as_str());
    if was_selected {
        *runtime
            .selected_connection_id
            .lock()
            .map_err(|_| "全局 SSH 选择状态不可用".to_string())? = None;
    }
    let progress = SshConnectionProgress {
        connection_id: id,
        task_id: None,
        status: "disconnected".to_string(),
        progress: 0,
        message: "已断开 SSH 连接".to_string(),
        updated_at: now_ms(),
        remote_status: None,
        error: None,
    };
    set_connection_progress(runtime.inner(), progress.clone())?;
    Ok(progress)
}

#[tauri::command]
pub async fn get_ssh_host_key(app: AppHandle, id: String) -> Result<SshHostKeyInfo, String> {
    let (config, _) = load_connection_for_agent(&app, id.trim())?;
    tauri::async_runtime::spawn_blocking(move || query_ssh_host_key(&config))
        .await
        .map_err(|error| format!("SSH 主机密钥读取任务失败: {error}"))?
}

#[tauri::command]
pub async fn get_ssh_status(
    app: AppHandle,
    runtime: State<'_, SshRuntimeState>,
    id: String,
    path: Option<String>,
) -> Result<SshRemoteStatus, String> {
    let id = id.trim().to_string();
    let (config, password) = load_connection_for_agent(&app, &id)?;
    let root = path
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config.default_root.clone())
        .unwrap_or_else(|| "~".to_string());
    let status = tauri::async_runtime::spawn_blocking(move || {
        query_ssh_status(&config, password.as_deref(), &root)
    })
    .await
    .map_err(|error| format!("SSH 状态读取任务失败: {error}"))??;
    let task_id = connection_progress(runtime.inner(), &id)?.and_then(|value| value.task_id);
    set_connection_progress(
        runtime.inner(),
        SshConnectionProgress {
            connection_id: id,
            task_id,
            status: "connected".to_string(),
            progress: 100,
            message: format!("SSH 已连接 · {} ms", status.latency_ms),
            updated_at: now_ms(),
            remote_status: Some(status.clone()),
            error: None,
        },
    )?;
    Ok(status)
}

#[tauri::command]
pub async fn list_ssh_directories_command(
    app: AppHandle,
    id: String,
    path: String,
) -> Result<Vec<SshDirectoryEntry>, String> {
    let (config, password) = load_connection_for_agent(&app, id.trim())?;
    tauri::async_runtime::spawn_blocking(move || {
        list_ssh_directories(&config, password.as_deref(), &path)
    })
    .await
    .map_err(|error| format!("远程目录读取任务失败: {error}"))?
}
