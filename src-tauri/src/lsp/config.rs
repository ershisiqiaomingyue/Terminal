//! Language-server configuration inspection and persistence.

use super::catalog::ServerConfig;
use super::executable::{path_directories, project_executable_dirs, resolve_executable};
use super::limits::MAX_LSP_CONFIG_BYTES;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The same bound `configured_server` reads with, so listing and writing agree.

/// The first-run default, embedded so the settings panel and a probe still work
/// before the per-user file has been created.
pub(super) const BUNDLED_LSP_CONFIG: &str =
    include_str!("../../resources/development/lsp-servers.json");

/// Server keys this app owns. Every other key belongs to the user and survives
/// a write untouched.
const KNOWN_SERVER_FIELDS: [&str; 9] = [
    "id",
    "command",
    "args",
    "languages",
    "filenames",
    "env",
    "disabled",
    "initialization_options",
    "settings",
];

static LSP_CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lsp_config_lock() -> &'static Mutex<()> {
    LSP_CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn lsp_config_path() -> Result<PathBuf, String> {
    Ok(crate::user_storage::root()?
        .join("tools")
        .join("lsp-servers.json"))
}

/// User-facing path: a canonicalized Windows path would show a `\\?\` prefix.
pub(super) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        if let Some(rest) = value.strip_prefix("\\\\?\\") {
            return rest.to_string();
        }
    }
    value
}

/// `Ok(None)` means the file does not exist yet. An unreadable, oversized or
/// invalid file is an error: the panel shows it and a write refuses, so a
/// damaged config is never silently replaced by a default.
pub(super) fn read_lsp_config_value(path: &Path) -> Result<Option<Value>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 LSP 配置 {}：{error}", path.display())),
    };
    if bytes.len() > MAX_LSP_CONFIG_BYTES {
        return Err(format!(
            "LSP 配置 {} 超过 256 KiB 限制，未做修改。",
            path.display()
        ));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "LSP 配置 {} 不是有效 JSON（{error}）；原文件保持不变，请修复后再保存。",
            path.display()
        )
    })?;
    validate_lsp_config(&value, path)?;
    Ok(Some(value))
}

fn validate_lsp_config(value: &Value, path: &Path) -> Result<(), String> {
    if !value.get("servers").is_some_and(Value::is_array) {
        return Err(format!(
            "LSP 配置 {} 缺少 servers 数组；原文件保持不变。",
            path.display()
        ));
    }
    Ok(())
}

/// Structure a *write* has to leave behind: every entry a JSON object with a
/// non-empty `id`, no two entries sharing an id (an edit addresses a server by
/// id, so a duplicate would make a save ambiguous), and every enabled server
/// with a non-empty `command`. A disabled entry is never launched, so it only
/// has to carry a string command when the key is present.
fn validate_lsp_config_for_write(value: &Value, path: &Path) -> Result<(), String> {
    validate_lsp_config(value, path)?;
    let Some(servers) = value.get("servers").and_then(Value::as_array) else {
        return Err(format!(
            "LSP 配置 {} 缺少 servers 数组。",
            display_path(path)
        ));
    };
    let mut seen: Vec<(String, usize)> = Vec::new();
    for (index, entry) in servers.iter().enumerate() {
        let fields = entry.as_object().ok_or_else(|| {
            format!(
                "配置 {} 中第 {} 项不是对象，未做修改。",
                display_path(path),
                index + 1
            )
        })?;
        let id = fields
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                format!(
                    "配置 {} 中第 {} 项缺少非空 id，未做修改。",
                    display_path(path),
                    index + 1
                )
            })?;
        let lowered = id.to_lowercase();
        if let Some((_, first)) = seen.iter().find(|(known, _)| *known == lowered) {
            return Err(format!(
                "配置 {} 中 id「{id}」重复（第 {} 项与第 {} 项），未做修改；请先改名或删除重复项。",
                display_path(path),
                first + 1,
                index + 1
            ));
        }
        seen.push((lowered, index));
        let disabled = fields
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match fields.get("command") {
            Some(command) => {
                let command = command.as_str().map(str::trim).ok_or_else(|| {
                    format!(
                        "配置 {} 中 id 为 {id} 的条目 command 不是字符串，未做修改。",
                        display_path(path)
                    )
                })?;
                if command.is_empty() && !disabled {
                    return Err(format!(
                        "配置 {} 中 id 为 {id} 的条目 command 为空，未做修改；请填写命令或删除该条目。",
                        display_path(path)
                    ));
                }
            }
            None if !disabled => {
                return Err(format!(
                    "配置 {} 中 id 为 {id} 的条目缺少 command，未做修改；请填写命令或删除该条目。",
                    display_path(path)
                ));
            }
            None => {}
        }
    }
    Ok(())
}

/// Serialize and atomically replace the config file.
fn write_lsp_config_value(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("无法序列化 LSP 配置：{error}"))?;
    crate::user_storage::atomic_write(path, &bytes)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerView {
    pub(super) id: String,
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) languages: BTreeMap<String, String>,
    pub(super) filenames: BTreeMap<String, String>,
    pub(super) disabled: bool,
    /// Keys of this entry that this app does not own, listed so the user can
    /// see what a write keeps.
    pub(super) extra_fields: Vec<String>,
    /// `ready`, `missing`, `unlaunchable`, `disabled` or `invalid`.
    pub(super) status: &'static str,
    /// How the executable was found: `绝对路径`, `工作区 node_modules/.bin` or `PATH`.
    pub(super) source: Option<&'static str>,
    pub(super) resolved_path: Option<String>,
    pub(super) message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspConfigView {
    pub(super) config_path: String,
    /// The list came from the embedded default because the user file does not
    /// exist yet; the first save creates it.
    pub(super) using_bundled_default: bool,
    /// Set when the user file exists but cannot be read or parsed. `servers` is
    /// then empty and every write keeps refusing, so the panel can show the
    /// problem next to an explicit「重置为默认」without ever overwriting on its own.
    pub(super) config_error: Option<String>,
    pub(super) servers: Vec<LspServerView>,
}

/// One editable server. `command`, `args` and `languages` replace the stored
/// values; every other key of that entry is left exactly as it was.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerEdit {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub languages: BTreeMap<String, String>,
}

/// One entry as the panel shows it. A structurally invalid entry is reported as
/// `invalid` instead of failing the whole listing, so the user can see and fix
/// the broken one.
fn lsp_server_view(
    value: &Value,
    index: usize,
    project_dirs: &[PathBuf],
    search_dirs: &[PathBuf],
) -> LspServerView {
    let extra_fields = value
        .as_object()
        .map(|fields| {
            fields
                .keys()
                .filter(|key| !KNOWN_SERVER_FIELDS.contains(&key.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let server = match serde_json::from_value::<ServerConfig>(value.clone()) {
        Ok(server) => server,
        Err(error) => {
            return LspServerView {
                id: value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("第 {} 项", index + 1)),
                command: value
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args: Vec::new(),
                languages: BTreeMap::new(),
                filenames: BTreeMap::new(),
                disabled: false,
                extra_fields,
                status: "invalid",
                source: None,
                resolved_path: None,
                message: format!("该条目结构无效，LSP 工具会跳过它：{error}"),
            };
        }
    };
    let (status, source, resolved_path, message) = if server.disabled {
        (
            "disabled",
            None,
            None,
            "已在配置中禁用，LSP 工具不会使用它。".to_string(),
        )
    } else {
        match resolve_executable(&server.command, project_dirs, search_dirs) {
            Ok(resolved) => {
                let mut message = format!("已通过{}解析。", resolved.source.label());
                if resolved.launcher.is_some() {
                    message.push_str("将通过 PowerShell 主机启动该 .ps1 脚本。");
                }
                (
                    "ready",
                    Some(resolved.source.label()),
                    Some(display_path(&resolved.path)),
                    message,
                )
            }
            Err(failure) => (failure.status(), None, None, failure.into_message()),
        }
    };
    LspServerView {
        id: server.id,
        command: server.command,
        args: server.args,
        languages: server.languages,
        filenames: server.filenames,
        disabled: server.disabled,
        extra_fields,
        status,
        source,
        resolved_path,
        message,
    }
}

/// `node_modules/.bin` candidates for an explicitly typed workspace path. A bad
/// value is an error because the user chose it on purpose.
fn workspace_project_dirs(workspace: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let Some(workspace) = workspace.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let workspace = PathBuf::from(workspace);
    if !workspace.is_dir() {
        return Err(format!(
            "工作区目录不存在或不是目录：{}",
            workspace.display()
        ));
    }
    Ok(project_executable_dirs(None, &workspace))
}

pub(super) fn lsp_config_view_at(
    path: &Path,
    workspace: Option<&str>,
) -> Result<LspConfigView, String> {
    // A workspace that does not exist is an argument problem: fail loudly instead
    // of probing without the project-local binaries the caller asked for.
    let project_dirs = workspace_project_dirs(workspace)?;
    // A damaged user file is reported *inside* the view rather than as a command
    // failure: the panel has to name the file and offer「重置为默认」, and it must
    // still show an empty list instead of pretending the default is the config.
    let (value, using_bundled_default, config_error) = match read_lsp_config_value(path) {
        Ok(Some(value)) => (value, false, None),
        Ok(None) => (
            serde_json::from_str::<Value>(BUNDLED_LSP_CONFIG)
                .map_err(|error| format!("内置 LSP 默认配置无效：{error}"))?,
            true,
            None,
        ),
        Err(error) => (Value::Null, false, Some(error)),
    };
    let search_dirs = path_directories();
    let servers = value
        .get("servers")
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .enumerate()
                .map(|(index, server)| lsp_server_view(server, index, &project_dirs, &search_dirs))
                .collect()
        })
        .unwrap_or_default();
    Ok(LspConfigView {
        config_path: display_path(path),
        using_bundled_default,
        config_error,
        servers,
    })
}

/// List every configured server with its probe result. Reading never writes.
#[tauri::command]
pub fn list_lsp_servers(workspace: Option<String>) -> Result<LspConfigView, String> {
    lsp_config_view_at(&lsp_config_path()?, workspace.as_deref())
}

fn validated_languages(
    languages: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if languages.is_empty() {
        return Err("至少需要保留一条「扩展名 → 语言 ID」映射。".to_string());
    }
    let mut validated = BTreeMap::new();
    for (extension, language) in languages {
        let extension = extension.trim();
        if extension.is_empty() || extension.contains(['.', '/', '\\', ':']) {
            return Err(format!(
                "扩展名「{extension}」无效；请只填写扩展名本身，例如 rs。"
            ));
        }
        let language = language.trim();
        if language.is_empty() {
            return Err(format!("扩展名 {extension} 的语言 ID 不能为空。"));
        }
        validated.insert(extension.to_string(), language.to_string());
    }
    Ok(validated)
}

/// Write one server back into the raw document: only `command`, `args` and
/// `languages` of that single entry change. Unknown keys, unknown top-level
/// keys and every other entry stay exactly as the user left them.
pub(super) fn apply_lsp_server_edit(path: &Path, edit: &LspServerEdit) -> Result<(), String> {
    let command = edit.command.trim();
    if command.is_empty() {
        return Err("语言服务器命令不能为空。".to_string());
    }
    if edit.args.iter().any(|arg| arg.trim().is_empty()) {
        return Err("启动参数不能包含空项；请删除空行后重试。".to_string());
    }
    let languages = validated_languages(&edit.languages)?;
    let id = edit.id.trim();
    if id.is_empty() {
        return Err("服务器 id 不能为空。".to_string());
    }
    let mut value = match read_lsp_config_value(path)? {
        Some(value) => value,
        // A missing file is seeded from the bundled default, including the
        // servers this edit does not touch.
        None => serde_json::from_str::<Value>(BUNDLED_LSP_CONFIG)
            .map_err(|error| format!("内置 LSP 默认配置无效：{error}"))?,
    };
    let entry = value
        .get_mut("servers")
        .and_then(Value::as_array_mut)
        .and_then(|servers| {
            servers
                .iter_mut()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| {
            format!(
                "配置 {} 中没有 id 为 {id} 的服务器，未做修改。",
                display_path(path)
            )
        })?;
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| format!("配置中 id 为 {id} 的条目不是对象，未做修改。"))?;
    entry.insert("command".to_string(), Value::String(command.to_string()));
    entry.insert(
        "args".to_string(),
        Value::Array(edit.args.iter().cloned().map(Value::String).collect()),
    );
    entry.insert(
        "languages".to_string(),
        serde_json::to_value(&languages).map_err(|error| format!("无法序列化语言映射：{error}"))?,
    );
    // Validate before the rename: a document with duplicate ids or an empty
    // command must never reach the disk.
    validate_lsp_config_for_write(&value, path)?;
    write_lsp_config_value(path, &value)
}

/// Save one server and return the refreshed probe result for the whole file.
#[tauri::command]
pub fn save_lsp_server(
    server: LspServerEdit,
    workspace: Option<String>,
) -> Result<LspConfigView, String> {
    let _guard = lsp_config_lock()
        .lock()
        .map_err(|_| "LSP 配置状态不可用。".to_string())?;
    let path = lsp_config_path()?;
    apply_lsp_server_edit(&path, &server)?;
    lsp_config_view_at(&path, workspace.as_deref())
}

/// Replace the file with the embedded default. Reached only from an explicit
/// user action: a damaged file is never overwritten by an automatic path, so
/// the panel asks first and the button carries the file path in its message.
pub(super) fn reset_lsp_config_at(
    path: &Path,
    workspace: Option<&str>,
) -> Result<LspConfigView, String> {
    let value: Value = serde_json::from_str(BUNDLED_LSP_CONFIG)
        .map_err(|error| format!("内置 LSP 默认配置无效：{error}"))?;
    validate_lsp_config_for_write(&value, path)?;
    write_lsp_config_value(path, &value)?;
    lsp_config_view_at(path, workspace)
}

/// Overwrite the user config with the bundled default after an explicit
/// confirmation.
#[tauri::command]
pub fn reset_lsp_servers(workspace: Option<String>) -> Result<LspConfigView, String> {
    let _guard = lsp_config_lock()
        .lock()
        .map_err(|_| "LSP 配置状态不可用。".to_string())?;
    let path = lsp_config_path()?;
    reset_lsp_config_at(&path, workspace.as_deref())
}
