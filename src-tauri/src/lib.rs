// Persist the existing native error paths without changing their console behavior.
macro_rules! eprintln {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        crate::diagnostics::legacy_error(module_path!(), message.clone());
        std::eprintln!("{message}");
    }};
}
mod diagnostics;
mod external_links;
mod agent;
mod agent_errors;
mod agent_events;
mod agent_executor;
mod app_identity;
mod appdata_migration;
mod attachment_cache;
mod background_process;
mod chat;
mod code_viewer;
mod code_transfers;
mod diagram_export;
mod collaboration;
mod file_changes;
mod history;
mod host_status;
mod logs;
mod lsp;
mod mcp_runtime;
mod model_plugins;
mod plugin_environments;
mod plugin_host;
mod plugin_manager;
mod plugin_runtime;
mod model_capability_cache;
mod provider_store;
mod project_preferences;
mod runtime_env;
mod skill_runtime;
mod ssh;
mod tasks;
mod terminal;
mod terminal_output;
mod tool_windows;
mod themes;
mod upgrade;
mod user_storage;
mod wsl;
mod webview_health;

use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const MAX_LOCAL_FILE_COPY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_WORKSPACE_GIT_DIFF_BYTES: usize = 512 * 1024;

fn canonical_local_attachment_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("文件路径为空".to_string());
    }
    let resolved = fs::canonicalize(trimmed).map_err(|error| format!("无法定位文件：{error}"))?;
    fs::metadata(&resolved).map_err(|error| format!("无法读取文件信息：{error}"))?;
    Ok(resolved)
}

#[tauri::command]
async fn open_local_attachment(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let target = canonical_local_attachment_path(&path)?;
        #[cfg(target_os = "windows")]
        background_process::command("cmd.exe")
            .args(["/D", "/C", "start", "", &target.to_string_lossy()])
            .spawn()
            .map_err(|error| format!("无法使用默认应用打开文件：{error}"))?;
        #[cfg(target_os = "macos")]
        background_process::command("open")
            .arg(&target)
            .spawn()
            .map_err(|error| format!("无法使用默认应用打开文件：{error}"))?;
        #[cfg(all(unix, not(target_os = "macos")))]
        background_process::command("xdg-open")
            .arg(&target)
            .spawn()
            .map_err(|error| format!("无法使用默认应用打开文件：{error}"))?;
        Ok(())
    })
    .await
    .map_err(|error| format!("启动默认应用失败：{error}"))?
}

#[tauri::command]
async fn read_local_attachment_for_copy(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let target = canonical_local_attachment_path(&path)?;
        let metadata =
            fs::metadata(&target).map_err(|error| format!("无法读取文件信息：{error}"))?;
        if !metadata.is_file() {
            return Err("只能复制文件内容，目录请使用打开操作".to_string());
        }
        if metadata.len() > MAX_LOCAL_FILE_COPY_BYTES {
            return Err(format!(
                "文件超过 {} MB，未复制内容",
                MAX_LOCAL_FILE_COPY_BYTES / 1024 / 1024
            ));
        }
        fs::read_to_string(&target).map_err(|error| format!("文件不是可复制的文本：{error}"))
    })
    .await
    .map_err(|error| format!("读取文件失败：{error}"))?
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceEnvironment {
    path: String,
    workspace_name: String,
    git: GitEnvironment,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitEnvironment {
    available: bool,
    repository: bool,
    branch: Option<String>,
    dirty: bool,
    changed_files: usize,
    added_lines: u64,
    removed_lines: u64,
    github_cli_available: bool,
    remote: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitWorkspaceFileChange {
    path: String,
    status: String,
    added_lines: u64,
    removed_lines: u64,
}

fn workspace_command(program: &str, args: &[&str], cwd: &PathBuf) -> Option<std::process::Output> {
    background_process::command(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
}

fn parse_git_numstat(output: &str) -> (u64, u64) {
    output.lines().fold((0, 0), |(added, removed), line| {
        let mut fields = line.split('\t');
        let next_added = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let next_removed = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        (
            added.saturating_add(next_added),
            removed.saturating_add(next_removed),
        )
    })
}

fn parse_git_numstat_by_path(output: &[u8]) -> std::collections::BTreeMap<String, (u64, u64)> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|record| std::str::from_utf8(record).ok())
        .filter_map(|record| {
            let mut fields = record.splitn(3, '\t');
            let added = fields.next()?.parse::<u64>().ok().unwrap_or(0);
            let removed = fields.next()?.parse::<u64>().ok().unwrap_or(0);
            let path = fields.next()?.to_string();
            Some((path, (added, removed)))
        })
        .collect()
}

fn git_status_label(code: &str) -> &'static str {
    if code == "??" {
        "created"
    } else if code.contains('D') {
        "deleted"
    } else if code.contains('R') {
        "renamed"
    } else {
        "modified"
    }
}

fn workspace_text_line_count(path: &PathBuf) -> u64 {
    const MAX_GIT_UNTRACKED_LINE_SCAN_BYTES: u64 = 4 * 1024 * 1024;
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    if !metadata.is_file() || metadata.len() > MAX_GIT_UNTRACKED_LINE_SCAN_BYTES {
        return 0;
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.lines().count() as u64)
        .unwrap_or(0)
}

#[tauri::command]
async fn get_workspace_git_changes(path: String) -> Result<Vec<GitWorkspaceFileChange>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = PathBuf::from(path.trim());
        if !workspace_path.is_dir() {
            return Err("工作区路径不是目录".to_string());
        }
        let git_available = workspace_command("git", &["--version"], &workspace_path)
            .is_some_and(|output| output.status.success());
        if !git_available {
            return Err("未安装 Git".to_string());
        }
        let Some(status) = workspace_command(
            "git",
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ],
            &workspace_path,
        )
        .filter(|output| output.status.success()) else {
            return Err("当前目录不是 Git 仓库".to_string());
        };
        let statistics = workspace_command(
            "git",
            &["diff", "--numstat", "-z", "HEAD", "--no-renames"],
            &workspace_path,
        )
        .filter(|output| output.status.success())
        .map(|output| parse_git_numstat_by_path(&output.stdout))
        .unwrap_or_default();
        let mut changes = Vec::new();
        for record in status.stdout.split(|byte| *byte == 0) {
            if record.len() < 4 {
                continue;
            }
            let code = std::str::from_utf8(&record[..2]).unwrap_or_default();
            let path = String::from_utf8_lossy(&record[3..]).to_string();
            if path.trim().is_empty() {
                continue;
            }
            let status = git_status_label(code).to_string();
            let (mut added_lines, removed_lines) = statistics.get(&path).copied().unwrap_or((0, 0));
            if status == "created" && added_lines == 0 && removed_lines == 0 {
                added_lines = workspace_text_line_count(&workspace_path.join(&path));
            }
            changes.push(GitWorkspaceFileChange {
                path,
                status,
                added_lines,
                removed_lines,
            });
        }
        changes.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(changes)
    })
    .await
    .map_err(|error| format!("读取 Git 文件改动失败：{error}"))?
}

fn truncate_workspace_git_diff(diff: String) -> String {
    if diff.len() <= MAX_WORKSPACE_GIT_DIFF_BYTES {
        return diff;
    }
    let mut end = MAX_WORKSPACE_GIT_DIFF_BYTES;
    while !diff.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!(
        "{}\n\n… Diff 已截断，仅显示前 {} KB。",
        &diff[..end],
        MAX_WORKSPACE_GIT_DIFF_BYTES / 1024
    )
}

#[tauri::command]
async fn get_workspace_git_file_diff(path: String, file_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = PathBuf::from(path.trim());
        if !workspace_path.is_dir() {
            return Err("工作区路径不是目录".to_string());
        }
        let requested_path = PathBuf::from(file_path.trim());
        if requested_path.as_os_str().is_empty()
            || requested_path.is_absolute()
            || requested_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err("Git 文件路径无效".to_string());
        }
        let target_path = workspace_path.join(&requested_path);
        if target_path.exists() {
            let workspace_root = fs::canonicalize(&workspace_path)
                .map_err(|error| format!("无法定位工作区：{error}"))?;
            let target = fs::canonicalize(&target_path)
                .map_err(|error| format!("无法定位 Git 文件：{error}"))?;
            if !target.starts_with(&workspace_root) {
                return Err("Git 文件不在当前工作区内".to_string());
            }
        }
        let file_arg = requested_path.to_string_lossy().replace('\\', "/");
        let has_head =
            workspace_command("git", &["rev-parse", "--verify", "HEAD"], &workspace_path)
                .is_some_and(|output| output.status.success());
        let primary_args = if has_head {
            vec![
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "HEAD",
                "--",
                &file_arg,
            ]
        } else {
            vec![
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "--",
                &file_arg,
            ]
        };
        let output = workspace_command("git", &primary_args, &workspace_path)
            .ok_or_else(|| "无法执行 Git Diff".to_string())?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if message.is_empty() {
                "无法读取 Git Diff".to_string()
            } else {
                message
            });
        }
        let mut diff = String::from_utf8_lossy(&output.stdout).to_string();
        if diff.trim().is_empty() && target_path.is_file() {
            let output = workspace_command(
                "git",
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    "/dev/null",
                    &file_arg,
                ],
                &workspace_path,
            )
            .ok_or_else(|| "无法读取新文件 Diff".to_string())?;
            // git diff --no-index finds a difference with exit code 1.
            if output.status.success() || output.status.code() == Some(1) {
                diff = String::from_utf8_lossy(&output.stdout).to_string();
            }
        }
        Ok(truncate_workspace_git_diff(diff))
    })
    .await
    .map_err(|error| format!("读取 Git 文件 Diff 失败：{error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceHistoryEntry {
    id: String,
    title: String,
    detail: String,
    when_label: String,
}

#[tauri::command]
async fn list_workspace_git_history(path: String) -> Result<Vec<WorkspaceHistoryEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = PathBuf::from(path.trim());
        if !workspace_path.is_dir() {
            return Err("工作区路径不是目录".to_string());
        }
        let Some(output) = workspace_command(
            "git",
            &["log", "-n", "20", "--pretty=format:%h%x09%s%x09%ci"],
            &workspace_path,
        )
        .filter(|item| item.status.success()) else {
            return Err("当前目录没有 Git 历史".to_string());
        };
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let id = parts.next()?.trim().to_string();
                if id.is_empty() {
                    return None;
                }
                let title = parts.next().unwrap_or("").trim().to_string();
                let when_label = parts.next().unwrap_or("").trim().to_string();
                Some(WorkspaceHistoryEntry {
                    id: id.clone(),
                    title,
                    detail: id,
                    when_label,
                })
            })
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_workspace_environment(path: String) -> Result<WorkspaceEnvironment, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = PathBuf::from(path.trim());
        if workspace_path.as_os_str().is_empty() {
            return Err("工作区路径为空".to_string());
        }
        if !fs::metadata(&workspace_path)
            .map_err(|error| format!("无法读取工作区：{error}"))?
            .is_dir()
        {
            return Err("工作区路径不是目录".to_string());
        }
        let workspace_name = workspace_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| workspace_path.to_str().unwrap_or("工作区"))
            .to_string();
        let unavailable = || GitEnvironment {
            available: false,
            repository: false,
            branch: None,
            dirty: false,
            changed_files: 0,
            added_lines: 0,
            removed_lines: 0,
            github_cli_available: false,
            remote: None,
        };
        let git_available = workspace_command("git", &["--version"], &workspace_path)
            .is_some_and(|output| output.status.success());
        if !git_available {
            return Ok(WorkspaceEnvironment {
                path: workspace_path.to_string_lossy().to_string(),
                workspace_name,
                git: unavailable(),
            });
        }
        let github_cli_available = workspace_command("gh", &["--version"], &workspace_path)
            .is_some_and(|output| output.status.success());
        let Some(status) = workspace_command(
            "git",
            &["status", "--porcelain=v1", "--branch"],
            &workspace_path,
        )
        .filter(|output| output.status.success()) else {
            return Ok(WorkspaceEnvironment {
                path: workspace_path.to_string_lossy().to_string(),
                workspace_name,
                git: GitEnvironment {
                    available: true,
                    repository: false,
                    branch: None,
                    dirty: false,
                    changed_files: 0,
                    added_lines: 0,
                    removed_lines: 0,
                    github_cli_available,
                    remote: None,
                },
            });
        };
        let raw_status = String::from_utf8_lossy(&status.stdout);
        let mut lines = raw_status.lines();
        let branch = lines
            .next()
            .and_then(|line| line.strip_prefix("## "))
            .map(|value| {
                let value = value.split("...").next().unwrap_or(value).trim();
                if value == "HEAD (no branch)" {
                    "分离 HEAD".to_string()
                } else {
                    value.to_string()
                }
            });
        let changed_files = lines.filter(|line| !line.trim().is_empty()).count();
        let (added_lines, removed_lines) = workspace_command(
            "git",
            &["diff", "--numstat", "HEAD", "--no-renames"],
            &workspace_path,
        )
        .filter(|output| output.status.success())
        .map(|output| parse_git_numstat(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or((0, 0));
        let remote = workspace_command("git", &["remote", "get-url", "origin"], &workspace_path)
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(WorkspaceEnvironment {
            path: workspace_path.to_string_lossy().to_string(),
            workspace_name,
            git: GitEnvironment {
                available: true,
                repository: true,
                branch,
                dirty: changed_files > 0,
                changed_files,
                added_lines,
                removed_lines,
                github_cli_available,
                remote,
            },
        })
    })
    .await
    .map_err(|error| format!("读取工作区环境失败：{error}"))?
}

#[tauri::command]
async fn finish_boot(app_handle: AppHandle, startup_ready: bool) -> Result<(), String> {
    let main = app_handle
        .get_webview_window("main")
        .ok_or_else(|| "找不到主窗口".to_string())?;

    // Keep the same ICO-derived icon as other windows; do not replace it
    // with a 512px bitmap that Windows has to shrink for the taskbar.
    main.show().map_err(|error| error.to_string())?;
    let _ = main.set_focus();
    if let Some(boot) = app_handle.get_webview_window("boot") {
        let _ = boot.close();
    }
    // Startup has committed session repair before asking to reveal the UI.
    // An error page may be shown, but must not dispatch recovered work.
    static RECOVERY_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if startup_ready && !RECOVERY_STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        tauri::async_runtime::spawn_blocking(move || {
            history::schedule_storage_maintenance(app_handle.clone());
            if let Err(error) = agent::background_shell::recover_background_shells(&app_handle) {
                eprintln!("无法恢复后台编译通知: {error}");
            }
            if let Err(error) = agent::recover_durable_agent_queue(&app_handle) {
                eprintln!("无法恢复 Agent 待发送队列: {error}");
            }
            if let Err(error) = collaboration::recover_and_dispatch_collaboration_jobs(&app_handle) {
                eprintln!("无法恢复协作子 Agent: {error}");
            }
        });
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // identifier 决定 `%APPDATA%` / `%LOCALAPPDATA%` 下用户能直接看到的目录名，
    // 这里先把它交给 `app_identity`，再把旧目录搬过来，最后才让日志、存储这些
    // 模块去算路径——顺序反了就会在新目录还没就绪时写进旧目录。
    let context = tauri::generate_context!();
    let identifier = context.config().identifier.clone();
    app_identity::set(&identifier);
    if let Err(error) = appdata_migration::migrate(&identifier) {
        eprintln!("用户数据目录迁移未完成：{error}");
    }
    diagnostics::initialize(None);
    tauri::Builder::default()
        .manage(agent_core::AgentRuntimeState::default())
        .manage(agent::background_shell::BackgroundShellState::default())
        .manage(tasks::TaskManager::default())
        .manage(terminal::TerminalState::default())
        .manage(tool_windows::ToolWindowState::default())
        .manage(agent::CompletionNoticeState::default())
        .manage(ssh::SshRuntimeState::default())
        .manage(code_transfers::FileTransferState::default())
        .manage(mcp_runtime::McpRuntimeState::default())
        .setup(|app| {
            // WebView2 cannot safely share one user-data directory between a
            // packaged instance and a debug instance. Keep the native app
            // data (history, providers and tasks) shared, but isolate the
            // browser cache/profile by build mode so both instances can run.
            let webview_root = app
                .path()
                .app_local_data_dir()?
                .join(if cfg!(debug_assertions) {
                    "webview-dev"
                } else {
                    "webview-release"
                });
            fs::create_dir_all(&webview_root)?;
            // 无边框窗口一旦在 WebView 内部跳转到外部站点就没有返回入口，
            // 这里只放行应用自身的页面，其它地址改由系统浏览器打开。
            let navigation_app = app.handle().clone();
            let new_window_app = app.handle().clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("/".into()))
                .title("Terminal")
                .inner_size(1440.0, 960.0)
                .min_inner_size(1100.0, 760.0)
                .resizable(true)
                .decorations(false)
                .transparent(false)
                .shadow(true)
                .visible(false)
                .center()
                .data_directory(webview_root.join("main"))
                .on_navigation(move |url| {
                    let internal = external_links::is_internal_url(url);
                    if !internal {
                        external_links::open_in_browser(&navigation_app, url);
                    }
                    internal
                })
                // window.open 一律不新建 WebView 窗口，交给系统浏览器。
                .on_new_window(move |url, _features| {
                    external_links::open_in_browser(&new_window_app, &url);
                    tauri::webview::NewWindowResponse::Deny
                })
                .build()
                .map_err(|error| error.to_string())?;
            diagnostics::setup(app.handle());
            user_storage::initialize(app.handle()).map_err(std::io::Error::other)?;
            if let Some(window) = app.get_webview_window("main") { webview_health::install(&window); }
            if let Err(error) = runtime_env::install_child_environment() {
                eprintln!("无法配置内置工具环境: {error}");
            }
            if let Err(error) = runtime_env::restore_environment_network_settings() {
                eprintln!("无法恢复网络代理配置: {error}");
            }
            model_capability_cache::ensure_capabilities_in_background(app.handle(), &[]);
            model_plugins::initialize(app.handle());
            plugin_host::initialize(app.handle());
            let mcp_state = app.state::<mcp_runtime::McpRuntimeState>();
            if let Err(error) = mcp_runtime::restore_mcp_profiles(app.handle(), &mcp_state) {
                eprintln!("无法恢复 MCP 配置: {error}");
            }
            WebviewWindowBuilder::new(app, "boot", WebviewUrl::App("/?boot=1".into()))
                .title("Terminal")
                .inner_size(260.0, 220.0)
                .min_inner_size(260.0, 220.0)
                .max_inner_size(260.0, 220.0)
                .resizable(false)
                .decorations(false)
                .transparent(true)
                // 和会话完成提示窗同一个坑：窗口透明不等于 WebView 透明，WebView2 那层
                // 默认底色是白的，卡片外的一圈留白会变成白边。alpha 必须正好是 0。
                .background_color(tauri::window::Color(0, 0, 0, 0))
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .center()
                .data_directory(webview_root.join("boot"))
                .on_navigation(crate::external_links::is_internal_url)
                .build()
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            diagnostics::export::get_diagnostic_log_info,
            diagnostics::export::open_diagnostic_log_directory,
            diagnostics::export::export_diagnostic_logs,
            diagnostics::health::report_diagnostic_events,
            diagnostics::health::report_frontend_heartbeat,
            webview_health::report_frontend_error,
            model_plugins::list_model_plugins,
            model_plugins::install_model_plugin,
            model_plugins::set_model_plugin_enabled,
            model_plugins::configure_model_plugin,
            model_plugins::refresh_model_plugin,
            model_plugins::probe_model_plugin,
            model_plugins::uninstall_model_plugin,
            plugin_host::list_host_plugins,
            plugin_host::discover_local_plugins,
            plugin_host::inspect_host_plugin,
            plugin_host::install_host_plugin,
            plugin_host::set_host_plugin_enabled,
            plugin_host::uninstall_host_plugin,
            plugin_host::report_plugin_error,
            plugin_manager::list_plugins,
            plugin_manager::check_plugin_updates,
            plugin_manager::install_plugin,
            plugin_manager::set_plugin_enabled,
            plugin_manager::refresh_plugin,
            plugin_manager::uninstall_plugin,
            diagram_export::download_mermaid_svg,
            finish_boot,
            upgrade::upgrade_status,
            upgrade::run_upgrade,
            upgrade::acknowledge_upgrade,
            agent::background_shell::list_background_shells,
            agent::background_shell::cancel_background_shell,
            tool_windows::create_tool_window,
            tool_windows::get_tool_window,
            tool_windows::ready_tool_window,
            tool_windows::abort_tool_window,
            tool_windows::focus_main_window,
            agent::get_completion_notice,
            agent::show_completion_notice,
            agent::dismiss_completion_notice,
            agent::open_completion_notice,
            get_workspace_environment,
            get_workspace_git_changes,
            get_workspace_git_file_diff,
            list_workspace_git_history,
            host_status::get_host_status,
            open_local_attachment,
            read_local_attachment_for_copy,
            runtime_env::list_environment_tools,
            runtime_env::check_environment_tool,
            runtime_env::save_environment_tool_path,
            runtime_env::download_environment_tool,
            runtime_env::get_environment_network_settings,
            runtime_env::save_environment_network_settings,
            tasks::list_tasks,
            tasks::cancel_task,
            code_viewer::list_code_viewer_directory,
            code_viewer::preview_code_viewer_mutation,
            code_viewer::execute_code_viewer_mutation,
            code_viewer::preview_ssh_code_viewer_mutation,
            code_viewer::execute_ssh_code_viewer_mutation,
            code_viewer::list_ssh_code_viewer_directory,
            code_viewer::read_ssh_code_viewer_file,
            code_viewer::write_ssh_code_viewer_file,
            code_transfers::get_code_transfer_settings,
            code_transfers::save_code_transfer_settings,
            code_transfers::inspect_code_download,
            code_transfers::inspect_code_upload,
            code_transfers::start_code_download,
            code_transfers::open_code_download,
            code_transfers::inspect_code_download_files,
            code_transfers::start_code_upload,
            code_transfers::list_code_transfers,
            code_transfers::cancel_code_transfer,
            code_viewer::ssh_code_viewer_file_version,
            code_viewer::code_viewer_file_version,
            code_viewer::ssh_code_viewer_file_metadata,
            code_viewer::code_viewer_file_metadata,
            code_viewer::code_viewer_git_directory,
            code_viewer::code_viewer_git_file,
            code_viewer::ssh_code_viewer_git_directory,
            code_viewer::ssh_code_viewer_git_file,
            code_viewer::ssh_workspace_git,
            code_viewer::query_ssh_cwd,
            code_viewer::read_code_viewer_file,
            themes::fetch_theme_text,
            themes::fetch_theme_package,
            themes::storage::initialize_theme_preferences,
            themes::storage::get_theme_preferences,
            themes::storage::save_theme_preferences,
            code_viewer::write_code_viewer_file,
            ssh::list_ssh_connections,
            ssh::save_ssh_connection,
            ssh::delete_ssh_connection,
            ssh::clear_ssh_credential,
            ssh::test_ssh_connection_command,
            ssh::test_ssh_connection_config,
            ssh::start_ssh_connection,
            ssh::connect_ssh_host_command,
            ssh::get_ssh_connection_progress,
            ssh::disconnect_ssh_connection_command,
            ssh::get_ssh_host_key,
            ssh::get_ssh_status,
            ssh::list_ssh_directories_command,
            wsl::list_wsl_distributions,
            wsl::list_wsl_profiles,
            wsl::save_wsl_profile,
            wsl::delete_wsl_profile,
            wsl::test_wsl_profile,
            wsl::get_wsl_status,
            terminal::count_active_ssh_terminals,
    terminal::open_terminal_session,
            terminal::read_terminal_output,
            terminal::write_terminal_input,
            terminal::write_ai_terminal_input,
            terminal::resize_terminal_session,
            terminal::interrupt_terminal_session,
            terminal::close_terminal_session,
            provider_store::list_agent_providers,
            provider_store::get_agent_provider_api_key,
            provider_store::save_agent_provider,
            provider_store::delete_agent_provider,
            chat::send_chat_message,
            agent::prepare_workspace_agent_context,
            agent::get_local_artifacts,
            agent::store_browser_attachment,
            agent::prepare_chat_attachment,
            agent::read_cached_image,
            agent::read_local_image,
            agent::index_local_logs,
            agent::search_local_logs,
            agent::read_local_log_range,
            agent::rebuild_local_log_timeline,
            agent::cancel_agent_request,
            agent::get_agent_settings,
            agent::update_agent_settings,
            agent::execute_conversation_command,
            agent::steer_agent_request,
            agent::resolve_agent_permission,
            agent::discover_agent_models,
            agent::check_agent_provider,
            agent::test_agent_provider_chat,
            agent::get_provider_quota,
            agent::refresh_agent_capabilities,
            model_capability_cache::get_agent_model_capabilities,
            agent::get_agent_model_context_window,
            agent::agent_one_shot,
            agent::enqueue_agent_message,
            agent::get_agent_thread_snapshot,
            agent::resume_agent_run,
            file_changes::snapshot_file_changes,
            file_changes::diff_file_changes,
            file_changes::open_workspace_file,
            file_changes::review_agent_file_change,
            file_changes::undo_agent_file_change,
            history::save_history_session,
            history::append_history_session_events,
            history::list_agent_runs,
            history::recover_interrupted_agent_runs,
            history::create_collaboration_job,
            history::get_collaboration_job,
            history::list_collaboration_jobs,
            history::recover_collaboration_agents,
            collaboration::run_collaboration_agent,
            history::get_conversation_thread,
            history::list_conversation_checkpoints,
            history::create_conversation_checkpoint,
            history::revert_conversation_to_checkpoint,
            history::rewind_conversation_before_message,
            history::fork_conversation_thread,
            history::list_history_sessions,
            history::list_history_session_summaries,
            history::get_history_session,
            history::get_history_turn_events,
            history::search_history_sessions,
            history::archive_history_session,
            history::delete_history_session,
            history::export_history_sessions,
            history::write_history_export,
            skill_runtime::list_skills,
            skill_runtime::management::read_skill_details,
            skill_runtime::management::set_skill_enabled,
            skill_runtime::management::delete_skill,
            project_preferences::get_project_preferences,
            project_preferences::save_project_preference,
            skill_runtime::save_skill_repositories,
            skill_runtime::preview_skill_repository,
            skill_runtime::import_skill_repository,
            skill_runtime::list_selinux_sources,
            skill_runtime::save_selinux_sources,
            mcp_runtime::connect_mcp_server,
            mcp_runtime::list_mcp_servers,
            mcp_runtime::disconnect_mcp_server,
            mcp_runtime::remove_mcp_server,
            mcp_runtime::get_mcp_connection_status,
            lsp::list_lsp_servers,
            lsp::save_lsp_server,
            lsp::reset_lsp_servers
        ])
        .build(context)
        .expect("error while building tauri application")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { code, .. } => {
                agent::flush_running_processes(app);
                diagnostics::record(
                    "info",
                    "application.exit_requested",
                    "native",
                    serde_json::json!({"code": code}),
                );
            }
            tauri::RunEvent::WindowEvent { label, event, .. }
                if matches!(event, tauri::WindowEvent::Destroyed) =>
            {
                diagnostics::record(
                    "info",
                    "window.destroyed",
                    &label,
                    serde_json::json!({}),
                );
            }
            tauri::RunEvent::Exit => {
                // Resident language servers are children of this process; kill
                // them before the host goes away so none survives it.
                lsp::shutdown_sessions_blocking();
                plugin_host::shutdown();
                model_plugins::shutdown(app);
                diagnostics::shutdown();
            }
            _ => {}
        });
}
