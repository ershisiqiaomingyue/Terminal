//! Executable discovery and Windows launch wrappers.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Where a resolved server executable came from, in resolution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExecutableSource {
    /// An absolute path written in `tools/lsp-servers.json`.
    Absolute,
    /// A project-local `node_modules/.bin`, inside the selected workspace.
    Workspace,
    /// An entry of the `PATH` environment variable.
    Path,
}

impl ExecutableSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Absolute => "绝对路径",
            Self::Workspace => "工作区 node_modules/.bin",
            Self::Path => "PATH",
        }
    }
}

/// The PowerShell host used to start a `.ps1` server, plus its prefix
/// arguments (ending with the script itself).
#[derive(Debug, Clone)]
pub(crate) struct Launcher {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedExecutable {
    pub(crate) path: PathBuf,
    pub(crate) source: ExecutableSource,
    /// `Some` only for a `.ps1` script: `CreateProcess` cannot start a
    /// PowerShell script directly, so the host process runs it instead.
    pub(crate) launcher: Option<Launcher>,
}

/// Windows executable suffixes in shell resolution order. `.ps1` is last
/// because PowerShell scripts are not part of the default `PATHEXT` and need a
/// host process, so any real executable wins over one.
const WINDOWS_EXECUTABLE_SUFFIXES: [&str; 5] = ["", ".exe", ".cmd", ".bat", ".ps1"];

fn executable_names(command: &str) -> Vec<String> {
    if cfg!(windows) {
        WINDOWS_EXECUTABLE_SUFFIXES
            .iter()
            .map(|suffix| format!("{command}{suffix}"))
            .collect()
    } else {
        vec![command.to_string()]
    }
}

pub(super) fn path_directories() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

fn resolve_in_directory(directory: &Path, command: &str) -> Option<PathBuf> {
    executable_names(command)
        .into_iter()
        .map(|name| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Project-local binary directories, closest to the opened file first. Only
/// directories inside the user-selected workspace are considered, so a nested
/// package resolves its own `node_modules/.bin` without reading outside the
/// workspace the user opened. Without a file the workspace root is the start.
pub(crate) fn project_executable_dirs(file: Option<&Path>, workspace: &Path) -> Vec<PathBuf> {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let start = match file {
        Some(file) => {
            let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
            if !file.starts_with(&workspace) {
                return Vec::new();
            }
            file.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| workspace.clone())
        }
        None => workspace.clone(),
    };
    let mut directories = Vec::new();
    let mut current = Some(start.as_path());
    while let Some(directory) = current {
        if !directory.starts_with(&workspace) {
            break;
        }
        directories.push(directory.join("node_modules").join(".bin"));
        if directory == workspace {
            break;
        }
        current = directory.parent();
    }
    if directories.is_empty() {
        directories.push(workspace.join("node_modules").join(".bin"));
    }
    directories
}

/// Why a server command could not be turned into something runnable.
/// `unlaunchable` means the executable *was* found but no host on this machine
/// can start it (only reachable for a `.ps1` with no PowerShell), so the panel
/// reports「找到但无法启动」instead of the misleading「未安装」.
#[derive(Debug)]
pub(crate) struct ResolveFailure {
    message: String,
    pub(super) unlaunchable: bool,
}

impl ResolveFailure {
    fn missing(message: String) -> Self {
        Self {
            message,
            unlaunchable: false,
        }
    }

    pub(super) fn unlaunchable(message: String) -> Self {
        Self {
            message,
            unlaunchable: true,
        }
    }

    /// Probe status, kept in step with `LspServerView::status`.
    pub(super) fn status(&self) -> &'static str {
        if self.unlaunchable {
            "unlaunchable"
        } else {
            "missing"
        }
    }

    pub(super) fn into_message(self) -> String {
        self.message
    }
}

impl std::fmt::Display for ResolveFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Lets `?` in a `Result<_, String>` function and `.contains(...)` in tests keep
/// working on the plain message.
impl std::ops::Deref for ResolveFailure {
    type Target = str;

    fn deref(&self) -> &str {
        &self.message
    }
}

impl From<ResolveFailure> for String {
    fn from(failure: ResolveFailure) -> String {
        failure.message
    }
}

pub(super) fn executable_missing(command: &str, absolute: bool) -> ResolveFailure {
    let cause = if absolute {
        format!("LSP 服务器路径 {command} 不存在")
    } else {
        format!("LSP 服务器 {command} 未安装或不在 PATH")
    };
    ResolveFailure::missing(format!("{cause}。查找顺序为 绝对路径 → 工作区 node_modules/.bin → PATH。请配置可执行文件路径；当前仍可使用源码检索和项目构建检查。"))
}

/// Resolve one server command. Order: an absolute configured path, then the
/// project-local `node_modules/.bin` directories, then `PATH`.
pub(crate) fn resolve_executable(
    command: &str,
    project_dirs: &[PathBuf],
    search_dirs: &[PathBuf],
) -> Result<ResolvedExecutable, ResolveFailure> {
    let configured = PathBuf::from(command);
    if configured.is_absolute() {
        if configured.is_file() {
            return finalize(configured, ExecutableSource::Absolute);
        }
        // Never fall through to PATH for an absolute path: silently running a
        // different server than the one the user configured hides the mistake.
        return Err(executable_missing(command, true));
    }
    for directory in project_dirs {
        if let Some(path) = resolve_in_directory(directory, command) {
            return finalize(path, ExecutableSource::Workspace);
        }
    }
    for directory in search_dirs {
        if let Some(path) = resolve_in_directory(directory, command) {
            return finalize(path, ExecutableSource::Path);
        }
    }
    Err(executable_missing(command, false))
}

fn finalize(path: PathBuf, source: ExecutableSource) -> Result<ResolvedExecutable, ResolveFailure> {
    let launcher = script_launcher(&path).map_err(ResolveFailure::unlaunchable)?;
    Ok(ResolvedExecutable {
        path,
        source,
        launcher,
    })
}

/// A `.ps1` server cannot be started by `CreateProcess`; it needs a PowerShell
/// host. `-ExecutionPolicy Bypass` is required because an unsigned user-chosen
/// script is normally blocked by the default policy, and `-NonInteractive`
/// keeps a stray prompt from hanging the request until the LSP timeout.
#[cfg(windows)]
fn script_launcher(executable: &Path) -> Result<Option<Launcher>, String> {
    let is_script = executable
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("ps1"));
    if !is_script {
        return Ok(None);
    }
    let program = powershell_host()?;
    let args = vec![
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        executable.to_string_lossy().into_owned(),
    ];
    Ok(Some(Launcher { program, args }))
}

#[cfg(not(windows))]
fn script_launcher(_executable: &Path) -> Result<Option<Launcher>, String> {
    Ok(None)
}

/// Prefer PowerShell 7 (`pwsh`) and fall back to the Windows PowerShell that
/// ships with the OS.
#[cfg(windows)]
fn powershell_host() -> Result<PathBuf, String> {
    let search_dirs = path_directories();
    for name in ["pwsh", "powershell"] {
        if let Ok(found) = resolve_executable(name, &[], &search_dirs) {
            return Ok(found.path);
        }
    }
    if let Some(root) = std::env::var_os("SystemRoot") {
        let candidate = PathBuf::from(root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("系统中未找到 PowerShell 主机，无法启动 .ps1 语言服务器；请改用 .cmd 包装脚本或可执行文件。".to_string())
}

/// Resolve one server command for a workspace file: the absolute configured
/// path, then the project-local `node_modules/.bin`, then `PATH`. The result
/// carries the PowerShell launcher when the winner is a `.ps1` script.
pub(crate) fn find_server_executable(
    command: &str,
    file: &Path,
    workspace: &Path,
) -> Result<ResolvedExecutable, ResolveFailure> {
    resolve_executable(
        command,
        &project_executable_dirs(Some(file), workspace),
        &path_directories(),
    )
}

/// Global lookup for test fixtures, which have no workspace. Returns the
/// discovered path only.
#[cfg(test)]
pub(super) fn find_executable(command: &str) -> Result<PathBuf, String> {
    resolve_executable(command, &[], &path_directories())
        .map(|resolved| resolved.path)
        .map_err(ResolveFailure::into_message)
}
