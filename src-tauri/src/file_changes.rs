//! Desktop file-review commands and application-scoped backup persistence.
pub use agent_core::file_changes::{
    capture_file_snapshot, capture_file_snapshot_with_cancellation, diff_file_snapshots,
    FileChangeSummary, FileSnapshot,
};
use agent_core::file_changes::{line_diff, text_line_changes, text_line_count};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{command, AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REVIEW_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[tauri::command]
pub async fn open_workspace_file(
    app: AppHandle,
    path: String,
    workspace_root: String,
    reveal: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = fs::canonicalize(workspace_root.trim())
            .map_err(|error| format!("无法访问工作区目录：{error}"))?;
        if !root.is_dir() {
            return Err("工作区路径不是目录".to_string());
        }
        let requested = PathBuf::from(path.trim());
        let candidate = if requested.is_absolute() {
            requested
        } else {
            root.join(requested)
        };
        let target = if reveal && !candidate.exists() {
            candidate
                .parent()
                .ok_or("无法确定文件所在目录")?
                .to_path_buf()
        } else {
            candidate
        };
        let target =
            fs::canonicalize(&target).map_err(|error| format!("无法访问文件路径：{error}"))?;
        if !target.starts_with(&root) {
            return Err("文件路径不在当前工作区内".to_string());
        }
        if !reveal && !target.is_file() {
            return Err("只能用默认程序打开文件".to_string());
        }
        if reveal {
            app.opener()
                .reveal_item_in_dir(target)
                .map_err(|error| error.to_string())
        } else {
            app.opener()
                .open_path(target.to_string_lossy(), None::<&str>)
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone)]
struct FileBackup {
    path: String,
    existed: bool,
    bytes: Option<Vec<u8>>,
    line_count: Option<u64>,
    after_state: Option<FileState>,
}

/// Captures the version written by the Agent. Rollback is only safe while the
/// file still matches this version; otherwise it could erase a later edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileState {
    exists: bool,
    sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedFileBackup {
    path: String,
    existed: bool,
    line_count: Option<u64>,
    blob: Option<String>,
    #[serde(default)]
    after_state: Option<FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedBackupManifest {
    version: u8,
    files: Vec<PersistedFileBackup>,
}

fn change_backups() -> &'static Mutex<HashMap<String, Vec<FileBackup>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<FileBackup>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn backup_key(session_id: &str, turn_id: &str) -> String {
    format!("{session_id}\n{turn_id}")
}

/// The global store only bridges capture and asynchronous review. Historical
/// rollback reads the durable manifest; completed/aborted turns retain no blobs.
pub(crate) struct StagedFileBackups(String);

impl Drop for StagedFileBackups {
    fn drop(&mut self) {
        if let Ok(mut store) = change_backups().lock() { store.remove(&self.0); }
    }
}

pub(crate) fn stage_turn_file_backups(session_id: &str, turn_id: &str, before: &[FileSnapshot]) -> StagedFileBackups {
    let mut total = 0usize;
    let mut backups = Vec::new();
    for snapshot in before {
        let path = Path::new(&snapshot.path);
        if !path.is_file() {
            continue;
        }
        let too_large =
            snapshot.size > 256 * 1024 || total + snapshot.size as usize > 8 * 1024 * 1024;
        let bytes = if too_large { None } else { fs::read(path).ok() };
        if let Some(bytes) = &bytes {
            total += bytes.len();
        }
        backups.push(FileBackup {
            path: snapshot.path.clone(),
            existed: true,
            bytes,
            line_count: snapshot.line_count,
            after_state: None,
        });
    }
    if let Ok(mut store) = change_backups().lock() {
        store.insert(backup_key(session_id, turn_id), backups);
    }
    StagedFileBackups(backup_key(session_id, turn_id))
}

#[allow(dead_code)]
pub(crate) fn extend_turn_file_backups(session_id: &str, turn_id: &str, before: &[FileSnapshot]) {
    if let Ok(mut store) = change_backups().lock() {
        let backups = store.entry(backup_key(session_id, turn_id)).or_default();
        let mut total = backups
            .iter()
            .filter_map(|backup| backup.bytes.as_ref())
            .map(Vec::len)
            .sum::<usize>();
        for snapshot in before {
            if backups.iter().any(|backup| backup.path == snapshot.path) {
                continue;
            }
            let bytes = if snapshot.size <= 256 * 1024
                && total.saturating_add(snapshot.size as usize) <= 8 * 1024 * 1024
            {
                fs::read(&snapshot.path).ok()
            } else {
                None
            };
            total = total.saturating_add(bytes.as_ref().map_or(0, Vec::len));
            backups.push(FileBackup {
                path: snapshot.path.clone(),
                existed: true,
                bytes,
                line_count: snapshot.line_count,
                after_state: None,
            });
        }
    }
}

pub fn retain_changed_file_backups(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    changes: &mut FileChangeSummary,
    after_files: &[FileSnapshot],
) -> Result<(), String> {
    update_turn_file_backups(app, session_id, turn_id, changes, after_files, true)
}

/// Keep the original baseline in memory while tools are still executing.
/// Persist only the currently changed files so stop/error paths retain review.
pub(crate) fn update_turn_file_backups(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    changes: &mut FileChangeSummary,
    after_files: &[FileSnapshot],
    final_snapshot: bool,
) -> Result<(), String> {
    let after_states = after_files
        .iter()
        .map(|snapshot| (snapshot.path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let key = backup_key(session_id, turn_id);
    let changed = changes
        .created
        .iter()
        .chain(changes.modified.iter())
        .chain(changes.deleted.iter())
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let backups = if let Ok(mut store) = change_backups().lock() {
        let Some(backups) = store.get_mut(&key) else {
            return Ok(());
        };
        if final_snapshot { backups.retain(|item| changed.contains(&item.path)); }
        for path in &changes.created {
            if !backups.iter().any(|item| &item.path == path) {
                backups.push(FileBackup {
                    path: path.clone(),
                    existed: false,
                    bytes: None,
                    line_count: None,
                    after_state: None,
                });
            }
        }
        for backup in backups.iter_mut().filter(|backup| changed.contains(&backup.path)) {
            backup.after_state = Some(match after_states.get(backup.path.as_str()) {
                Some(snapshot) => FileState {
                    exists: true,
                    sha256: (snapshot.size <= MAX_HASH_BYTES).then(|| snapshot.hash.clone()),
                },
                None => file_state(&backup.path)?,
            });
        }
        backups.iter().filter(|backup| changed.contains(&backup.path)).cloned().collect::<Vec<_>>()
    } else {
        return Err("无法保存文件变更备份".to_string());
    };
    if final_snapshot {
        if let Ok(mut store) = change_backups().lock() { store.remove(&key); }
    }
    if backups.is_empty() {
        let directory = persistent_turn_directory(app, session_id, turn_id)?;
        if directory.exists() {
            fs::remove_dir_all(directory)
                .map_err(|error| format!("无法清理空会话文件备份: {error}"))?;
        }
        return Ok(());
    }
    enrich_file_change_entries(changes, &backups);
    persist_turn_file_backups(app, session_id, turn_id, &backups)?;
    Ok(())
}

fn backup_component(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn persistent_turn_directory(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| {
            directory
                .join("agent-file-backups")
                .join(backup_component(session_id))
                .join(backup_component(turn_id))
        })
        .map_err(|error| format!("无法定位会话文件备份目录: {error}"))
}

fn manifest_path(directory: &Path) -> PathBuf {
    directory.join("manifest.json")
}

fn persist_turn_file_backups(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    backups: &[FileBackup],
) -> Result<(), String> {
    let directory = persistent_turn_directory(app, session_id, turn_id)?;
    persist_file_backups_at(&directory, backups)
}

fn persist_file_backups_at(directory: &Path, backups: &[FileBackup]) -> Result<(), String> {
    if directory.exists() {
        fs::remove_dir_all(&directory).map_err(|error| format!("无法更新会话文件备份: {error}"))?;
    }
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建会话文件备份: {error}"))?;
    let mut files = Vec::with_capacity(backups.len());
    for (index, backup) in backups.iter().enumerate() {
        let blob = backup
            .bytes
            .as_ref()
            .map(|bytes| {
                let name = format!("file-{index}.bin");
                fs::write(directory.join(&name), bytes)
                    .map_err(|error| format!("无法写入会话文件备份: {error}"))?;
                Ok::<String, String>(name)
            })
            .transpose()?;
        files.push(PersistedFileBackup {
            path: backup.path.clone(),
            existed: backup.existed,
            line_count: backup.line_count,
            blob,
            after_state: backup.after_state.clone(),
        });
    }
    let manifest = serde_json::to_vec_pretty(&PersistedBackupManifest { version: 1, files })
        .map_err(|error| format!("无法编码会话文件备份清单: {error}"))?;
    let temporary = directory.join("manifest.json.tmp");
    fs::write(&temporary, manifest)
        .map_err(|error| format!("无法写入会话文件备份清单: {error}"))?;
    fs::rename(temporary, manifest_path(&directory))
        .map_err(|error| format!("无法提交会话文件备份清单: {error}"))
}

fn load_persisted_backup(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    path: &str,
) -> Result<Option<FileBackup>, String> {
    let directory = persistent_turn_directory(app, session_id, turn_id)?;
    load_file_backup_at(&directory, path)
}

fn load_file_backup_at(directory: &Path, path: &str) -> Result<Option<FileBackup>, String> {
    let manifest = manifest_path(&directory);
    if !manifest.exists() {
        return Ok(None);
    }
    let manifest =
        fs::read(&manifest).map_err(|error| format!("无法读取会话文件备份清单: {error}"))?;
    let manifest: PersistedBackupManifest = serde_json::from_slice(&manifest)
        .map_err(|error| format!("会话文件备份清单损坏: {error}"))?;
    let Some(entry) = manifest.files.into_iter().find(|entry| entry.path == path) else {
        return Ok(None);
    };
    let bytes = entry
        .blob
        .map(|blob| {
            fs::read(directory.join(blob))
                .map_err(|error| format!("无法读取会话文件备份内容: {error}"))
        })
        .transpose()?;
    Ok(Some(FileBackup {
        path: entry.path,
        existed: entry.existed,
        bytes,
        line_count: entry.line_count,
        after_state: entry.after_state,
    }))
}

fn file_state(path: &str) -> Result<FileState, String> {
    let path = Path::new(path);
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileState {
                exists: false,
                sha256: None,
            });
        }
        Err(error) => return Err(format!("无法读取当前文件状态: {error}")),
    };
    if !metadata.is_file() {
        return Err("文件已被替换为目录或其他非普通文件，无法安全撤销".to_string());
    }
    if metadata.len() > MAX_HASH_BYTES {
        return Ok(FileState {
            exists: true,
            sha256: None,
        });
    }
    let bytes = fs::read(path).map_err(|error| format!("无法读取当前文件状态: {error}"))?;
    Ok(FileState {
        exists: true,
        sha256: Some(format!("{:x}", Sha256::digest(bytes))),
    })
}

fn find_backup(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    path: &str,
) -> Result<FileBackup, String> {
    if let Ok(store) = change_backups().lock() {
        if let Some(backup) = store
            .get(&backup_key(session_id, turn_id))
            .and_then(|items| items.iter().find(|item| item.path == path).cloned())
        {
            return Ok(backup);
        }
    }
    load_persisted_backup(app, session_id, turn_id, path)?
        .ok_or_else(|| "这次改动没有可撤销的备份".to_string())
}

fn text_of(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| "这个文件不是文本，无法按行审核".to_string())
}

fn enrich_file_change_entries(changes: &mut FileChangeSummary, backups: &[FileBackup]) {
    for entry in &mut changes.entries {
        let backup = backups.iter().find(|backup| backup.path == entry.path);
        let after = fs::metadata(&entry.path)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() <= MAX_REVIEW_FILE_BYTES)
            .and_then(|_| fs::read(&entry.path).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok());
        match (entry.status.as_str(), backup, after) {
            ("created", _, Some(after)) => {
                entry.added_lines = text_line_count(&after);
                entry.removed_lines = 0;
            }
            ("deleted", Some(backup), _) => {
                entry.added_lines = 0;
                entry.removed_lines = backup
                    .bytes
                    .as_ref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .map(text_line_count)
                    .or(backup.line_count)
                    .unwrap_or(0);
            }
            ("modified", Some(backup), Some(after)) => {
                if let Some(before) = backup
                    .bytes
                    .as_ref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                {
                    let (added, removed) = text_line_changes(before, &after);
                    entry.added_lines = added;
                    entry.removed_lines = removed;
                }
            }
            _ => {}
        }
    }
    changes.added_lines = changes.entries.iter().map(|entry| entry.added_lines).sum();
    changes.removed_lines = changes
        .entries
        .iter()
        .map(|entry| entry.removed_lines)
        .sum();
}

fn remove_persisted_backup(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    path: &str,
) -> Result<(), String> {
    let directory = persistent_turn_directory(app, session_id, turn_id)?;
    let manifest_file = manifest_path(&directory);
    if !manifest_file.exists() {
        return Ok(());
    }
    let encoded =
        fs::read(&manifest_file).map_err(|error| format!("无法读取会话文件备份清单: {error}"))?;
    let mut manifest: PersistedBackupManifest = serde_json::from_slice(&encoded)
        .map_err(|error| format!("会话文件备份清单损坏: {error}"))?;
    let removed = manifest
        .files
        .iter()
        .find(|entry| entry.path == path)
        .cloned();
    manifest.files.retain(|entry| entry.path != path);
    if let Some(entry) = removed {
        if let Some(blob) = entry.blob {
            let _ = fs::remove_file(directory.join(blob));
        }
    }
    if manifest.files.is_empty() {
        fs::remove_dir_all(&directory).map_err(|error| format!("无法清理会话文件备份: {error}"))?;
        return Ok(());
    }
    let encoded = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("无法编码会话文件备份清单: {error}"))?;
    fs::write(manifest_file, encoded).map_err(|error| format!("无法更新会话文件备份清单: {error}"))
}

pub fn delete_session_file_backups(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let root = app
        .path()
        .app_data_dir()
        .map(|directory| {
            directory
                .join("agent-file-backups")
                .join(backup_component(session_id))
        })
        .map_err(|error| format!("无法定位会话文件备份目录: {error}"))?;
    if root.exists() {
        fs::remove_dir_all(root).map_err(|error| format!("无法删除会话文件备份: {error}"))?;
    }
    if let Ok(mut store) = change_backups().lock() {
        let prefix = format!("{session_id}\n");
        store.retain(|key, _| !key.starts_with(&prefix));
    }
    Ok(())
}

#[command]
pub fn review_agent_file_change(
    app: AppHandle,
    session_id: String,
    turn_id: String,
    path: String,
) -> Result<String, String> {
    let backup = find_backup(&app, &session_id, &turn_id, &path)?;
    let before = match &backup.bytes {
        Some(bytes) => text_of(bytes)?,
        None if backup.existed => return Err("原文件太大，没有留下可审核的文本备份".to_string()),
        None => String::new(),
    };
    let after = match fs::metadata(&path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err("当前路径不是普通文件，无法审核".to_string())
        }
        Ok(metadata) if metadata.len() > MAX_REVIEW_FILE_BYTES => {
            return Err("当前文件过大，无法安全生成完整文本差异".to_string())
        }
        Ok(_) => fs::read(&path).map_err(|error| format!("无法读取当前文件: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("无法读取当前文件: {error}")),
    };
    let after = match after.as_slice() {
        [] if !Path::new(&path).exists() => String::new(),
        bytes => text_of(bytes)?,
    };
    Ok(line_diff(&before, &after))
}

#[command]
pub fn undo_agent_file_change(
    app: AppHandle,
    session_id: String,
    turn_id: String,
    path: String,
) -> Result<(), String> {
    let backup = find_backup(&app, &session_id, &turn_id, &path)?;
    let expected_after = backup
        .after_state
        .as_ref()
        .ok_or_else(|| "这次改动来自旧版本备份，缺少变更后指纹，无法安全撤销".to_string())?;
    if expected_after.sha256.is_none() && expected_after.exists {
        return Err("文件超过安全校验大小限制，无法自动撤销".to_string());
    }
    let current = file_state(&path)?;
    if current.exists != expected_after.exists || current.sha256 != expected_after.sha256 {
        return Err("文件在此轮 Agent 修改后已发生变化，已拒绝撤销以保护后续修改".to_string());
    }
    if !backup.existed {
        if Path::new(&path).exists() {
            fs::remove_file(&path).map_err(|error| format!("无法删除新建文件: {error}"))?;
        }
    } else if let Some(bytes) = &backup.bytes {
        if let Some(parent) = Path::new(&path).parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法恢复目录: {error}"))?;
        }
        fs::write(&path, bytes).map_err(|error| format!("无法恢复文件: {error}"))?;
    } else {
        return Err("原文件太大，无法撤销".to_string());
    }
    if let Ok(mut store) = change_backups().lock() {
        if let Some(items) = store.get_mut(&backup_key(&session_id, &turn_id)) {
            items.retain(|item| item.path != path);
        }
    }
    remove_persisted_backup(&app, &session_id, &turn_id, &path)
}

#[command]
pub fn snapshot_file_changes(paths: Vec<String>) -> Result<Vec<FileSnapshot>, String> {
    capture_file_snapshot(&paths)
}

#[command]
pub fn diff_file_changes(before: Vec<FileSnapshot>, after: Vec<FileSnapshot>) -> FileChangeSummary {
    diff_file_snapshots(&before, &after)
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn staged_backup_blobs_are_released_on_abort_and_remain_readable_from_disk() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("before.txt");
        fs::write(&path, b"original content").unwrap();
        let before = capture_file_snapshot(&[workspace.path().to_string_lossy().into_owned()]).unwrap();
        let session = workspace.path().to_string_lossy().into_owned();
        for turn in 0..200 {
            let turn = turn.to_string();
            let key = backup_key(&session, &turn);
            let lease = stage_turn_file_backups(&session, &turn, &before);
            let backups = change_backups().lock().unwrap().get(&key).unwrap().clone();
            assert!(!backups.is_empty());
            let persisted = workspace.path().join("persisted");
            if turn == "199" { persist_file_backups_at(&persisted, &backups).unwrap(); }
            drop(lease);
            assert!(!change_backups().lock().unwrap().contains_key(&key));
            if turn == "199" {
                let restored = load_file_backup_at(&persisted, &path.to_string_lossy()).unwrap().unwrap();
                assert_eq!(restored.bytes.as_deref(), Some(b"original content".as_slice()));
            }
        }
    }
}
