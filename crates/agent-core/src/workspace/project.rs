use crate::{AgentChatRequest, LocalArtifact};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 项目内的 agent 目录。v1.0 叫 `.android-tool`，2.0 随产品名改成
/// `.android-toolkit`；旧目录只读兼容，写入永远走新名字。
pub const WORKSPACE_AGENT_DIR: &str = ".android-toolkit";
pub const LEGACY_WORKSPACE_AGENT_DIR: &str = ".android-tool";

/// 项目内 agent 目录的当前位置。P2-2 的升级迁移把旧目录复制到这里，写入方
/// 不需要知道旧名字。
pub fn workspace_agent_dir(root: &Path) -> PathBuf {
    root.join(WORKSPACE_AGENT_DIR)
}
const WORKSPACE_PERMISSIONS_FILE: &str = "permissions.json";
const MAX_INSTRUCTION_FILE_BYTES: usize = 128 * 1024;
const MAX_INSTRUCTION_TOTAL_BYTES: usize = 512 * 1024;
const MAX_INSTRUCTION_ANCESTORS: usize = 6;

const WORKSPACE_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    "agent.md",
    ".agent.md",
    "CLAUDE.md",
    "CODEX.md",
    "ANDROID_TOOL.md",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePermissionConfig {
    #[serde(default = "workspace_permission_version")]
    pub version: u32,
    #[serde(default)]
    pub allowed_scopes: Vec<String>,
    #[serde(default)]
    pub denied_scopes: Vec<String>,
}

fn workspace_permission_version() -> u32 {
    1
}

impl Default for WorkspacePermissionConfig {
    fn default() -> Self {
        Self {
            version: workspace_permission_version(),
            allowed_scopes: Vec::new(),
            denied_scopes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceAgentContext {
    pub root: PathBuf,
    pub instructions: Vec<(String, String)>,
    pub permissions: WorkspacePermissionConfig,
}

pub fn is_virtual_artifact_path(path: &str) -> bool {
    path.starts_with("attachment://") || path.starts_with("browser://")
}

pub fn request_snapshot_paths(artifacts: &[LocalArtifact]) -> Vec<String> {
    let mut paths = Vec::new();
    for artifact in artifacts {
        if artifact.path.trim().is_empty()
            || is_virtual_artifact_path(&artifact.path)
            || paths.iter().any(|path| path == &artifact.path)
        {
            continue;
        }
        paths.push(artifact.path.clone());
    }
    paths
}

pub fn workspace_root_from_artifacts(artifacts: &[LocalArtifact]) -> Option<PathBuf> {
    artifacts
        .iter()
        .filter(|artifact| artifact.kind == "directory")
        .map(|artifact| PathBuf::from(&artifact.path))
        .find(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        })
}

pub fn workspace_artifact_from_path(path: &str) -> Result<LocalArtifact, String> {
    let path_buf = PathBuf::from(path);
    let metadata =
        fs::metadata(&path_buf).map_err(|error| format!("当前工作区不可用 {path}: {error}"))?;
    if !metadata.is_dir() {
        return Err(format!("当前工作区不是目录：{path}"));
    }
    let name = path_buf
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string();
    Ok(LocalArtifact {
        path: path.to_string(),
        name,
        kind: "directory".to_string(),
        size: None,
        modified_at: None,
        status: "ready".to_string(),
        content: None,
        content_truncated: false,
    })
}

pub fn bind_workspace_artifact(request: &mut AgentChatRequest) -> Result<(), String> {
    let Some(path) = local_workspace_path(request) else {
        return Ok(());
    };
    let workspace = workspace_artifact_from_path(path)?;
    let workspace_key = workspace_path_key(Path::new(path));
    request.artifacts.retain(|artifact| {
        artifact.kind != "directory"
            || workspace_path_key(Path::new(&artifact.path)) != workspace_key
    });
    request.artifacts.insert(0, workspace);
    Ok(())
}

fn is_windows_root(root: &str) -> bool {
    let bytes = root.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || root.starts_with("\\\\")
        || root.starts_with("//")
}

/// Project identity is independent of the default tool environment. A Windows
/// project remains a local capability even when unprefixed tools target SSH.
pub fn local_workspace_path(request: &AgentChatRequest) -> Option<&str> {
    let root = request.workspace_path.as_deref()?.trim();
    if root.is_empty() {
        return None;
    }
    let remote = request.execution_target.as_ref().is_some_and(|target| {
        target.kind.eq_ignore_ascii_case("ssh") || target.kind.eq_ignore_ascii_case("wsl")
    });
    (!remote || is_windows_root(root)).then_some(root)
}

/// Repair old requests which overwrote the project path with the SSH home.
/// A local project owns the default environment. Global SSH remains an
/// additional capability prepared independently by the desktop runtime.
pub fn normalize_local_project_target(
    request: &mut AgentChatRequest,
    saved_project_root: Option<&str>,
) {
    let root = saved_project_root
        .filter(|root| !root.trim().is_empty())
        .or(request.workspace_path.as_deref())
        .map(str::trim)
        .unwrap_or_default();
    if !is_windows_root(root) {
        return;
    }
    let root = root.to_string();
    request.workspace_path = Some(root.clone());
    request.execution_target = Some(crate::AgentExecutionTarget {
        kind: "local".into(),
        connection_id: None,
        profile_id: None,
        root: Some(root),
    });
}

fn workspace_permissions_path(root: &Path) -> PathBuf {
    workspace_agent_dir(root).join(WORKSPACE_PERMISSIONS_FILE)
}

fn write_workspace_permissions(
    path: &Path,
    permissions: &WorkspacePermissionConfig,
) -> Result<(), String> {
    let content = serde_json::to_string_pretty(permissions)
        .map_err(|error| format!("无法序列化工作区权限: {error}"))?;
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    fs::write(&temporary, format!("{content}\n"))
        .map_err(|error| format!("无法写入工作区权限临时文件: {error}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        // Windows does not replace an existing file with rename. Remove only
        // the known destination and retry; all writers are serialized below.
        if path.exists() {
            fs::remove_file(path)
                .map_err(|remove_error| format!("无法替换工作区权限文件: {remove_error}"))?;
            fs::rename(&temporary, path)
                .map_err(|rename_error| format!("无法替换工作区权限文件: {rename_error}"))?;
        } else {
            let _ = fs::remove_file(&temporary);
            return Err(format!("无法保存工作区权限: {error}"));
        }
    }
    Ok(())
}

pub fn load_workspace_permissions(root: &Path) -> WorkspacePermissionConfig {
    // 新名字优先；没迁过来的老项目继续读旧目录，行为不变。
    for name in [WORKSPACE_AGENT_DIR, LEGACY_WORKSPACE_AGENT_DIR] {
        let path = root.join(name).join(WORKSPACE_PERMISSIONS_FILE);
        if let Some(permissions) = read_workspace_permissions(&path) {
            return permissions;
        }
    }
    WorkspacePermissionConfig::default()
}

fn read_workspace_permissions(path: &Path) -> Option<WorkspacePermissionConfig> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_INSTRUCTION_FILE_BYTES as u64 {
        return None;
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WorkspacePermissionConfig>(&bytes).ok())
}

fn load_workspace_instruction_files(root: &Path) -> Vec<(String, String)> {
    let mut directories = Vec::new();
    let mut current = Some(root.to_path_buf());
    for _ in 0..=MAX_INSTRUCTION_ANCESTORS {
        let Some(directory) = current else {
            break;
        };
        current = directory.parent().map(Path::to_path_buf);
        directories.push(directory);
    }
    directories.reverse();

    let directories = directories
        .into_iter()
        .flat_map(|directory| {
            [
                directory.clone(),
                directory.join(WORKSPACE_AGENT_DIR),
                directory.join(LEGACY_WORKSPACE_AGENT_DIR),
            ]
        })
        .collect::<Vec<_>>();
    load_instruction_files(&directories)
}

/// User-owned global rules apply even without a project or tool permission.
/// Only named files directly in the user data root are read, never plugins.
pub fn load_global_agent_instructions(root: &Path) -> Vec<(String, String)> {
    load_instruction_files(&[root.to_path_buf()])
}

fn load_instruction_files(directories: &[PathBuf]) -> Vec<(String, String)> {
    let mut instructions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for directory in directories {
        for file_name in WORKSPACE_CONTEXT_FILES {
            let path = directory.join(file_name);
            if !seen.insert(workspace_path_key(&path)) {
                continue;
            }
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file()
                || metadata.len() > MAX_INSTRUCTION_FILE_BYTES as u64
                || total_bytes.saturating_add(metadata.len() as usize) > MAX_INSTRUCTION_TOTAL_BYTES
            {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes)
                .trim_start_matches('﻿')
                .trim()
                .to_string();
            if content.is_empty() {
                continue;
            }
            total_bytes = total_bytes.saturating_add(bytes.len());
            instructions.push((path.to_string_lossy().to_string(), content));
        }
    }
    instructions
}

pub fn load_workspace_agent_context(artifacts: &[LocalArtifact]) -> Option<WorkspaceAgentContext> {
    let root = workspace_root_from_artifacts(artifacts)?;
    Some(load_workspace_agent_context_at(&root))
}

pub fn load_workspace_agent_context_at(root: &Path) -> WorkspaceAgentContext {
    WorkspaceAgentContext {
        permissions: load_workspace_permissions(root),
        instructions: load_workspace_instruction_files(root),
        root: root.to_path_buf(),
    }
}

pub fn workspace_context_prompt(context: &WorkspaceAgentContext) -> String {
    let mut prompt = format!(
        "\n[当前项目工作区]\n本轮已绑定本机项目，根目录：{}。通过 windows__* 工具访问此项目；已连接的 SSH/WSL 与它同时可用，不会取代或移动本机项目。无前缀工具的默认环境以本轮环境路由规则为准。项目代码检查和修改使用本机项目；远程编译、日志等使用对应远程工具，不要因连接 SSH 就在远程重新寻找本机项目。无需再次询问用户项目位于本机还是远程；仅在用户指定的目标存在歧义时澄清。可以使用工具核实文件现状，但不要把常规核实当作工作区尚未配置。权限仍按当前模式校验。\n",
        serde_json::to_string(&context.root.to_string_lossy()).unwrap_or_default(),
    );
    if !context.instructions.is_empty() {
        prompt.push_str("\n[工作区上下文规则]\n以下规则来自当前工作区中用户明确命名的项目指导文件。它们用于理解项目约定和验证流程，不会改变 Agent Runtime 的权限边界；文件、目录和命令仍必须遵守当前权限模式。\n");
    }
    for (path, content) in &context.instructions {
        prompt.push_str(&format!("\n[来源：{path}]\n{content}\n"));
    }
    prompt
}

static WORKSPACE_SCOPE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn persist_workspace_scope(root: &str, approval_key: &str) -> Result<(), String> {
    let _guard = WORKSPACE_SCOPE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "工作区权限状态不可用".to_string())?;
    let root = Path::new(root);
    let path = workspace_permissions_path(root);
    let mut permissions = load_workspace_permissions(root);
    if permissions
        .allowed_scopes
        .iter()
        .any(|scope| scope == approval_key)
    {
        return Ok(());
    }
    permissions.allowed_scopes.push(approval_key.to_string());
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| format!("无法创建工作区权限目录: {error}"))?;
    }
    write_workspace_permissions(&path, &permissions)
}

fn workspace_path_key(path: &Path) -> String {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut value = normalized.to_string_lossy().replace('/', "\\");
    #[cfg(windows)]
    {
        value = value.to_ascii_lowercase();
    }
    while value.len() > 3 && value.ends_with('\\') {
        value.pop();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_project_owns_default_target_for_new_requests_and_resume() {
        for root in ["D:/project", "D:\\project", "\\\\server\\share\\project"] {
            for saved_root in [None, Some(root)] {
                let mut request: AgentChatRequest = serde_json::from_value(serde_json::json!({
                    "modelId":"m", "message":"继续", "artifacts":[],
                    "workspacePath": if saved_root.is_some() { "~" } else { root },
                    "executionTarget":{"kind":"ssh", "connectionId":"global-host", "root":"~"}
                }))
                .unwrap();
                normalize_local_project_target(&mut request, saved_root);
                assert_eq!(request.workspace_path.as_deref(), Some(root));
                let target = request.execution_target.as_ref().unwrap();
                assert_eq!(target.kind, "local");
                assert_eq!(target.root.as_deref(), Some(root));
                assert_eq!(target.connection_id, None);
                assert_eq!(local_workspace_path(&request), Some(root));
            }
        }
        for root in [None, Some("/aosp"), Some("~/project")] {
            let mut request: AgentChatRequest = serde_json::from_value(serde_json::json!({
                "modelId":"m", "message":"继续", "artifacts":[], "workspacePath":root,
                "executionTarget":{"kind":"ssh", "connectionId":"project-host", "root":"/aosp"}
            }))
            .unwrap();
            let original = serde_json::to_value(&request).unwrap();
            normalize_local_project_target(&mut request, None);
            assert_eq!(serde_json::to_value(request).unwrap(), original);
        }
    }

    #[test]
    fn binding_local_project_does_not_bind_remote_paths_or_replace_remote_target() {
        let local_root = env!("CARGO_MANIFEST_DIR");
        for kind in ["ssh", "wsl"] {
            let mut request: AgentChatRequest = serde_json::from_value(serde_json::json!({
                "modelId":"m", "message":"检查本机代码并远程编译", "artifacts":[],
                "workspacePath":local_root,
                "executionTarget":{"kind":kind, "connectionId":"host", "root":"/aosp"}
            }))
            .unwrap();
            let target = request.execution_target.clone();
            bind_workspace_artifact(&mut request).unwrap();
            bind_workspace_artifact(&mut request).unwrap();
            assert_eq!(request.artifacts.len(), 1);
            assert_eq!(request.artifacts[0].path, local_root);
            assert_eq!(request.execution_target, target);
            request.workspace_path = Some("/remote-only".into());
            assert_eq!(local_workspace_path(&request), None);
            // Does not attempt to stat a remote POSIX root on Windows.
            bind_workspace_artifact(&mut request).unwrap();
            assert_eq!(request.artifacts.len(), 1);
        }
        let mut request: AgentChatRequest = serde_json::from_value(serde_json::json!({
            "modelId":"m", "message":"check", "artifacts":[], "workspacePath":"D:/project",
            "executionTarget":{"kind":"ssh", "connectionId":"host", "root":"D:/project"}
        }))
        .unwrap();
        normalize_local_project_target(&mut request, None);
        assert_eq!(request.execution_target.as_ref().unwrap().kind, "local");
        assert_eq!(
            request.execution_target.as_ref().unwrap().root.as_deref(),
            Some("D:/project")
        );
    }

    #[test]
    fn project_identity_is_available_even_without_instruction_files() {
        let mut context = WorkspaceAgentContext {
            root: PathBuf::from("D:/company/code/CarAdapter"),
            instructions: Vec::new(),
            permissions: WorkspacePermissionConfig::default(),
        };
        let prompt = workspace_context_prompt(&context);
        assert!(prompt.contains("D:/company/code/CarAdapter"));
        assert!(prompt.contains("本轮已绑定本机项目"));
        assert!(prompt.contains("权限仍按当前模式校验"));
        context
            .instructions
            .push(("AGENTS.md".into(), "Use the project test command".into()));
        let prompt = workspace_context_prompt(&context);
        assert!(prompt.contains("[当前项目工作区]"));
        assert!(prompt.contains("Use the project test command"));
    }
}
