use serde::Serialize;
use std::{fs, path::PathBuf};

const MAX_ENTRIES: usize = 500;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[tauri::command]
pub async fn preview_code_viewer_mutation(request: agent_core::BrowserMutationRequest) -> Result<agent_core::BrowserMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || agent_core::preview_local_browser_mutation(&request))
        .await.map_err(|error| format!("预览文件操作失败：{error}"))?
}

#[tauri::command]
pub async fn execute_code_viewer_mutation(request: agent_core::BrowserMutationRequest) -> Result<agent_core::BrowserMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || agent_core::execute_local_browser_mutation(&request))
        .await.map_err(|error| format!("执行文件操作失败：{error}"))?
}

#[tauri::command]
pub async fn preview_ssh_code_viewer_mutation(app: tauri::AppHandle, runtime: tauri::State<'_, crate::ssh::SshRuntimeState>, connection_id: String, request: agent_core::BrowserMutationRequest) -> Result<agent_core::BrowserMutationPreview, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::preview_ssh_browser_mutation(&config, password.as_deref(), &request))
        .await.map_err(|error| format!("预览 SSH 文件操作失败：{error}"))?
}

#[tauri::command]
pub async fn execute_ssh_code_viewer_mutation(app: tauri::AppHandle, runtime: tauri::State<'_, crate::ssh::SshRuntimeState>, connection_id: String, request: agent_core::BrowserMutationRequest) -> Result<agent_core::BrowserMutationPreview, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::execute_ssh_browser_mutation(&config, password.as_deref(), &request))
        .await.map_err(|error| format!("执行 SSH 文件操作失败：{error}"))?
}

fn local_file_version(file: &std::path::Path) -> Result<String, String> {
    let metadata = fs::metadata(file).map_err(|error| format!("无法检查文件：{error}"))?;
    let modified = metadata.modified().map_err(|error| format!("无法检查修改时间：{error}"))?
        .duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}

#[tauri::command]
pub async fn code_viewer_git_directory(path: String) -> Result<Option<agent_core::GitBrowserDirectory>, String> {
    agent_core::local_git_browser_directory(&path).await
}

#[tauri::command]
pub async fn code_viewer_git_file(path: String) -> Result<agent_core::GitBrowserDetails, String> {
    agent_core::local_git_browser_details(&path).await
}

#[tauri::command]
pub async fn ssh_code_viewer_git_directory(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    path: String,
) -> Result<Option<agent_core::GitBrowserDirectory>, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::ssh_git_browser_directory(&config, password.as_deref(), &path))
        .await.map_err(|error| format!("SSH Git 状态任务失败：{error}"))?
}

#[tauri::command]
pub async fn ssh_code_viewer_git_file(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: String,
    path: String,
) -> Result<agent_core::GitBrowserDetails, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::ssh_git_browser_details(&config, password.as_deref(), &root, &path))
        .await.map_err(|error| format!("SSH Git 文件任务失败：{error}"))?
}

/// 右栏「版本控制」页签在 SSH 页签下的取数入口。
///
/// 形状刻意与本地 `workspace_environment` / `workspace_git_changes` 对齐：
/// 前端 GitPane 只需要按 target.kind 换调用，渲染与缓存逻辑一行都不用改。
#[tauri::command]
pub async fn ssh_workspace_git(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: Option<String>,
) -> Result<agent_core::SshWorkspaceGit, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let root = root.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        agent_core::ssh_workspace_git(&config, password.as_deref(), &root)
    })
    .await
    .map_err(|error| format!("SSH Git 状态任务失败：{error}"))?
}

/// cwd 跟随的 SSH 兜底：远端 shell 不上报 OSC 7 时，由前端低频轮询 `pwd`。
///
/// 只读一个短字符串，失败必须保持静默——这条路径每 5 秒跑一次，
/// 弹 toast 会把用户淹掉，调用方按返回值自己决定是否忽略。
#[tauri::command]
pub async fn query_ssh_cwd(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: Option<String>,
) -> Result<String, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let root = root.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        // `root` 是上一次已知目录：远端每条命令都是新进程，先 cd 过去才能读到真实 cwd。
        agent_core::query_ssh_cwd(&config, password.as_deref(), &root)
    })
    .await
    .map_err(|error| format!("查询 SSH 工作目录任务失败：{error}"))?
}

#[tauri::command]
pub async fn code_viewer_file_version(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        local_file_version(&canonical_path(&path)?)
    }).await.map_err(|error| format!("检查文件任务失败：{error}"))?
}

#[tauri::command]
pub async fn code_viewer_file_metadata(path: String) -> Result<agent_core::BrowserFileMetadata, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let file = canonical_path(&path)?;
        let metadata = fs::metadata(file).map_err(|error| format!("无法读取文件信息：{error}"))?;
        let to_millis = |time: std::io::Result<std::time::SystemTime>| time.ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_millis().min(u64::MAX as u128) as u64);
        Ok(agent_core::BrowserFileMetadata {
            size: metadata.len(),
            modified_at: to_millis(metadata.modified()),
            created_at: to_millis(metadata.created()),
        })
    }).await.map_err(|error| format!("读取文件信息任务失败：{error}"))?
}

#[tauri::command]
pub async fn ssh_code_viewer_file_version(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: String,
    path: String,
) -> Result<String, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move ||
        agent_core::ssh_browser_file_version(&config, password.as_deref(), &root, &path))
        .await.map_err(|error| format!("检查 SSH 文件任务失败：{error}"))?
}

#[tauri::command]
pub async fn ssh_code_viewer_file_metadata(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: String,
    path: String,
) -> Result<agent_core::BrowserFileMetadata, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::ssh_browser_file_metadata(&config, password.as_deref(), &root, &path))
        .await.map_err(|error| format!("读取 SSH 文件信息任务失败：{error}"))?
}

#[tauri::command]
pub async fn list_ssh_code_viewer_directory(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    path: String,
) -> Result<agent_core::SshBrowserDirectory, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move ||
        agent_core::list_ssh_browser_directory(&config, password.as_deref(), &path))
        .await.map_err(|error| format!("读取 SSH 目录任务失败：{error}"))?
}

#[tauri::command]
pub async fn read_ssh_code_viewer_file(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: String,
    path: String,
) -> Result<String, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move ||
        agent_core::read_ssh_browser_file(&config, password.as_deref(), &root, &path))
        .await.map_err(|error| format!("读取 SSH 文件任务失败：{error}"))?
}

#[tauri::command]
pub async fn write_ssh_code_viewer_file(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    root: String,
    path: String,
    content: String,
    expected_version: String,
) -> Result<String, String> {
    let (config, password) = crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    tauri::async_runtime::spawn_blocking(move || agent_core::write_ssh_browser_file(&config, password.as_deref(), &root, &path, &content, &expected_version))
        .await.map_err(|error| format!("保存 SSH 文件任务失败：{error}"))?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeViewerEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub kind: String,
    pub symlink_target: Option<String>,
}

fn canonical_path(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("目录路径为空".to_string());
    }
    fs::canonicalize(path).map_err(|error| format!("无法访问路径：{error}"))
}

fn hidden_or_generated(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".idea" | ".gradle" | "node_modules" | "target" | "dist" | "build" | ".next"
    )
}

fn is_non_editable_file(path: &PathBuf) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("apk" | "aar" | "jar" | "so" | "dex" | "class" | "zip" | "7z" | "rar" | "gz" | "xz" | "png" | "jpg" | "jpeg" | "webp" | "gif" | "ico" | "woff" | "woff2" | "ttf" | "otf" | "mp3" | "wav" | "mp4" | "avi" | "pdf")
    )
}

#[tauri::command]
pub async fn list_code_viewer_directory(path: String) -> Result<Vec<CodeViewerEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let directory = canonical_path(&path)?;
        if !directory.is_dir() {
            return Err("请拖入或选择文件夹，而不是单个文件".to_string());
        }
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("无法读取目录：{error}"))?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                if hidden_or_generated(&name) {
                    return None;
                }
                let link_metadata = fs::symlink_metadata(entry.path()).ok()?;
                let symlink_target = if link_metadata.file_type().is_symlink() {
                    fs::read_link(entry.path()).ok().map(|path| path.to_string_lossy().to_string())
                } else { None };
                let metadata = fs::metadata(entry.path()).ok();
                let is_directory = metadata.as_ref().is_some_and(|metadata| metadata.is_dir());
                let kind = if is_directory { "directory" } else if metadata.as_ref().is_some_and(|metadata| metadata.is_file()) {
                    "file"
                } else if symlink_target.is_some() { "symlink" } else { "other" };
                Some(CodeViewerEntry {
                    name,
                    // Retain the opened path/alias, including directory links.
                    path: PathBuf::from(&path).join(entry.file_name()).to_string_lossy().to_string(),
                    is_directory,
                    size: if is_directory { None } else { metadata.as_ref().map(|metadata| metadata.len()) },
                    kind: kind.into(),
                    symlink_target,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .is_directory
                .cmp(&left.is_directory)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        entries.truncate(MAX_ENTRIES);
        Ok(entries)
    })
    .await
    .map_err(|error| format!("读取目录任务失败：{error}"))?
}

#[tauri::command]
pub async fn read_code_viewer_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let file = canonical_path(&path)?;
        let metadata = fs::metadata(&file).map_err(|error| format!("无法读取文件信息：{error}"))?;
        if !metadata.is_file() {
            return Err("只能打开文本文件".to_string());
        }
        if is_non_editable_file(&file) {
            return Err("该文件为二进制或归档文件，无法在代码查看器中编辑".to_string());
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err("文件超过 1 MB，请使用外部编辑器查看".to_string());
        }
        let bytes = fs::read(&file).map_err(|error| format!("无法读取文件：{error}"))?;
        String::from_utf8(bytes)
            .map_err(|_| "该文件不是 UTF-8 文本，无法在代码查看器中预览".to_string())
    })
    .await
    .map_err(|error| format!("读取文件任务失败：{error}"))?
}

#[tauri::command]
pub async fn write_code_viewer_file(path: String, content: String, expected_version: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err("编辑内容超过 1 MB 限制".to_string());
        }
        let file = canonical_path(&path)?;
        let metadata = fs::metadata(&file).map_err(|error| format!("无法读取文件信息：{error}"))?;
        if !metadata.is_file() {
            return Err("只能保存到普通文件".to_string());
        }
        if is_non_editable_file(&file) {
            return Err("该文件为二进制或归档文件，无法在代码查看器中编辑".to_string());
        }
        if let Some(expected) = expected_version {
            if local_file_version(&file)? != expected { return Err("磁盘文件已被其他编辑器修改，未覆盖保存，请先重新加载".into()); }
        }
        fs::write(&file, content).map_err(|error| format!("保存文件失败：{error}"))?;
        local_file_version(&file)
    })
    .await
    .map_err(|error| format!("保存文件任务失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listed_subdirectories_accept_create_in_the_opened_browser_root() {
        let root = std::env::temp_dir().join(format!("android-tool-listed-create-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(root.join("nested")).unwrap();
        let root_text = root.to_string_lossy().into_owned();
        let entries = list_code_viewer_directory(root_text.clone()).await.unwrap();
        let nested = entries.iter().find(|entry| entry.name == "nested").unwrap();
        assert_eq!(nested.path, root.join("nested").to_string_lossy());
        for path in [root_text.clone(), nested.path.clone()] {
            for action in ["create-file", "create-directory"] {
                let name = if action == "create-file" { "1.txt" } else { "new-folder" };
                let mut request = agent_core::BrowserMutationRequest { root: root_text.clone(), path: path.clone(), action: action.into(), new_name: Some(name.into()), recursive: false, expected_version: None, acknowledged: true, destination_directory: None, overwrite: false };
                request.expected_version = Some(preview_code_viewer_mutation(request.clone()).await.unwrap().version);
                execute_code_viewer_mutation(request).await.unwrap();
                assert!(PathBuf::from(&path).join(name).exists());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn linked_directories_keep_browser_aliases_when_creating_children() {
        let fixture = std::env::temp_dir().join(format!("android-tool-linked-create-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let root = fixture.join("opened"); let destination = fixture.join("outside");
        fs::create_dir_all(&root).unwrap(); fs::create_dir_all(destination.join("nested")).unwrap();
        let link = root.join("linked");
        let output = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&destination).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let root_text = root.to_string_lossy().into_owned();
        let entries = list_code_viewer_directory(link.to_string_lossy().into_owned()).await.unwrap();
        let nested = entries.iter().find(|entry| entry.name == "nested").unwrap();
        assert_eq!(nested.path, link.join("nested").to_string_lossy());
        for path in [link.to_string_lossy().into_owned(), nested.path.clone()] {
            let mut request = agent_core::BrowserMutationRequest { root: root_text.clone(), path: path.clone(), action: "create-file".into(), new_name: Some("1.txt".into()), recursive: false, expected_version: None, acknowledged: true, destination_directory: None, overwrite: false };
            request.expected_version = Some(preview_code_viewer_mutation(request.clone()).await.unwrap().version);
            execute_code_viewer_mutation(request).await.unwrap();
            assert!(PathBuf::from(path).join("1.txt").exists());
        }
        fs::remove_dir(link).unwrap();
        fs::remove_dir_all(fixture).unwrap();
    }
}
