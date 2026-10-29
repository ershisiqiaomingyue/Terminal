//! Desktop settings and background jobs; compression/SSH live in agent-core.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

#[derive(Default, Clone)]
pub struct FileTransferState(Arc<Mutex<HashMap<String, TransferRecord>>>);
struct TransferRecord {
    view: TransferView,
    cancellation: Arc<AtomicBool>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    id: String,
    connection_id: String,
    server: String,
    name: String,
    direction: String,
    status: String,
    phase: String,
    bytes_done: u64,
    bytes_total: u64,
    started_at: u64,
    finished_at: Option<u64>,
    output_path: Option<String>,
    file_path: Option<String>,
    error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSettings {
    download_directory: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSettings {
    download_directory: Option<String>,
    default_directory: String,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn settings_file(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory.join("file-transfer-settings.json"))
}
fn download_paths_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(settings_file(app)?.with_file_name("code-download-paths.json"))
}
fn read_download_paths(app: &AppHandle) -> Result<Vec<String>, String> {
    let file = download_paths_file(app)?;
    if !file.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&fs::read(file).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}
fn remember_download_path(
    app: &AppHandle,
    state: &FileTransferState,
    path: &std::path::Path,
) -> Result<(), String> {
    // Serialize the two transfer workers so neither loses the other's record.
    let _jobs = state.0.lock().map_err(|_| "传输状态不可用")?;
    let path = fs::canonicalize(path)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    let mut paths = read_download_paths(app)?;
    paths.retain(|saved| saved != &path);
    paths.push(path);
    if paths.len() > 200 {
        paths.drain(..paths.len() - 200);
    }
    fs::write(
        download_paths_file(app)?,
        serde_json::to_vec(&paths).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn inspect_code_download_files(paths: Vec<String>) -> Result<Vec<agent_core::BrowserDownloadStatus>, String> {
    if paths.len() > 100 { return Err("下载记录检查数量超限".into()); }
    tauri::async_runtime::spawn_blocking(move || paths.iter().map(|path| agent_core::inspect_browser_download(path)).collect())
        .await.map_err(|error| error.to_string())
}
#[tauri::command]
pub async fn open_code_download(app: AppHandle, path: String, reveal: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = get_code_transfer_settings(app.clone())?;
        let mut roots = vec![PathBuf::from(settings.default_directory)];
        if let Some(directory) = settings.download_directory {
            roots.push(PathBuf::from(directory));
        }
        let path = agent_core::validate_browser_download_path(
            std::path::Path::new(&path),
            &roots,
            &read_download_paths(&app)?,
        )?;
        if reveal {
            app.opener()
                .reveal_item_in_dir(path)
                .map_err(|error| error.to_string())
        } else {
            app.opener()
                .open_path(path.to_string_lossy(), None::<&str>)
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
pub fn get_code_transfer_settings(app: AppHandle) -> Result<TransferSettings, String> {
    let path = settings_file(&app)?;
    let stored: StoredSettings = if path.exists() {
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?
    } else {
        StoredSettings::default()
    };
    Ok(TransferSettings {
        download_directory: stored.download_directory,
        default_directory: app
            .path()
            .download_dir()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned(),
    })
}
#[tauri::command]
pub fn save_code_transfer_settings(
    app: AppHandle,
    download_directory: Option<String>,
) -> Result<(), String> {
    let directory = download_directory
        .filter(|path| !path.trim().is_empty())
        .map(|path| -> Result<String, String> {
            let path =
                fs::canonicalize(path).map_err(|error| format!("下载目录不可访问：{error}"))?;
            if !path.is_dir() {
                return Err("请选择下载文件夹".into());
            }
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()?;
    fs::write(
        settings_file(&app)?,
        serde_json::to_vec(&StoredSettings {
            download_directory: directory,
        })
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}
#[tauri::command]
pub async fn inspect_code_download(
    app: AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    path: String,
    download_access: Option<agent_core::SshDownloadAccess>,
) -> Result<agent_core::BrowserTransferInfo, String> {
    let (config, password) =
        crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let config = agent_core::transfer_connection(
        &config,
        &format!(
            ":transfer-inspect-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let result = agent_core::ssh_browser_transfer_info_with_access(&config, password.as_deref(), &path,
            &download_access.unwrap_or_default());
        agent_core::disconnect_ssh_connection(&config);
        result
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
pub async fn inspect_code_upload(
    app: AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    connection_id: String,
    paths: Vec<String>,
    destination: String,
) -> Result<Vec<agent_core::UploadConflict>, String> {
    if paths.is_empty() || paths.len() > 128 {
        return Err("一次请选择 1 到 128 个文件或文件夹".into());
    }
    let (config, password) =
        crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let config = agent_core::transfer_connection(
        &config,
        &format!(
            ":upload-inspect-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let result = agent_core::inspect_ssh_browser_upload(
            &config,
            password.as_deref(),
            &paths,
            &destination,
        );
        agent_core::disconnect_ssh_connection(&config);
        result
    })
    .await
    .map_err(|error| error.to_string())?
}
fn reserve(
    state: &FileTransferState,
    connection: &agent_core::SshConnectionConfig,
    name: String,
    direction: &str,
) -> Result<TransferView, String> {
    let mut jobs = state.0.lock().map_err(|_| "传输状态不可用")?;
    if jobs
        .values()
        .filter(|job| job.view.status == "running")
        .count()
        >= 2
    {
        return Err("同时最多执行两个传输，请等待当前任务完成".into());
    }
    if jobs.len() >= 40 {
        if let Some(oldest) = jobs
            .values()
            .filter(|job| job.view.status != "running")
            .min_by_key(|job| job.view.started_at)
            .map(|job| job.view.id.clone())
        {
            jobs.remove(&oldest);
        }
    }
    let id = format!(
        "transfer-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let view = TransferView {
        id: id.clone(),
        connection_id: connection.id.clone(),
        server: connection.name.clone(),
        name,
        direction: direction.into(),
        status: "running".into(),
        phase: "checking".into(),
        bytes_done: 0,
        bytes_total: 0,
        started_at: now(),
        finished_at: None,
        output_path: None,
        file_path: None,
        error: None,
    };
    jobs.insert(
        id,
        TransferRecord {
            view: view.clone(),
            cancellation: Arc::new(AtomicBool::new(false)),
        },
    );
    Ok(view)
}
fn update(
    app: &AppHandle,
    state: &FileTransferState,
    id: &str,
    action: impl FnOnce(&mut TransferView),
) {
    let view = state.0.lock().ok().and_then(|mut jobs| {
        jobs.get_mut(id).map(|job| {
            action(&mut job.view);
            job.view.clone()
        })
    });
    if let Some(view) = view {
        let _ = app.emit("code-transfer-progress", view);
    }
}
fn server_directory(config: &agent_core::SshConnectionConfig) -> String {
    agent_core::browser_download_server_directory(&config.name, &config.host, config.port)
}
#[tauri::command]
pub async fn start_code_download(
    app: AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    state: tauri::State<'_, FileTransferState>,
    connection_id: String,
    path: String,
    confirmed_large: bool,
    download_access: Option<agent_core::SshDownloadAccess>,
) -> Result<TransferView, String> {
    let (config, password) =
        crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let settings = get_code_transfer_settings(app.clone())?;
    let custom_directory = settings.download_directory.is_some();
    let base = PathBuf::from(
        settings
            .download_directory
            .unwrap_or(settings.default_directory),
    );
    let view = reserve(
        &state,
        &config,
        path.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .into(),
        "download",
    )?;
    dispatch(
        app,
        state.inner().clone(),
        view.clone(),
        config,
        password,
        Work::Download {
            path,
            base,
            confirmed_large,
            custom_directory,
            access: download_access.unwrap_or_default(),
        },
    );
    Ok(view)
}
#[tauri::command]
pub fn start_code_upload(
    app: AppHandle,
    runtime: tauri::State<'_, crate::ssh::SshRuntimeState>,
    state: tauri::State<'_, FileTransferState>,
    connection_id: String,
    paths: Vec<String>,
    destination: String,
    policy: agent_core::UploadPolicy,
    conflicts: Vec<agent_core::UploadConflict>,
) -> Result<TransferView, String> {
    if paths.is_empty() || paths.len() > 128 {
        return Err("一次请选择 1 到 128 个文件或文件夹".into());
    }
    let (config, password) =
        crate::ssh::connected_connection_for_agent(&app, &runtime, &connection_id)?;
    let name = if paths.len() == 1 {
        PathBuf::from(&paths[0])
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        format!("{} 个文件或目录", paths.len())
    };
    let view = reserve(&state, &config, name, "upload")?;
    dispatch(
        app,
        state.inner().clone(),
        view.clone(),
        config,
        password,
        Work::Upload {
            paths,
            destination,
            policy,
            conflicts,
        },
    );
    Ok(view)
}
enum Work {
    Download {
        path: String,
        base: PathBuf,
        confirmed_large: bool,
        custom_directory: bool,
        access: agent_core::SshDownloadAccess,
    },
    Upload {
        paths: Vec<String>,
        destination: String,
        policy: agent_core::UploadPolicy,
        conflicts: Vec<agent_core::UploadConflict>,
    },
}
fn dispatch(
    app: AppHandle,
    state: FileTransferState,
    view: TransferView,
    config: agent_core::SshConnectionConfig,
    password: Option<String>,
    work: Work,
) {
    tauri::async_runtime::spawn_blocking(move || {
        let id = view.id;
        let cancellation = match state
            .0
            .lock()
            .ok()
            .and_then(|jobs| jobs.get(&id).map(|job| job.cancellation.clone()))
        {
            Some(cancel) => cancel,
            None => return,
        };
        let server = server_directory(&config);
        let config = agent_core::transfer_connection(&config, &format!(":{id}"));
        let callback_app = app.clone();
        let callback_state = state.clone();
        let callback_id = id.clone();
        let reported = Mutex::new((String::new(), std::time::Instant::now()));
        let progress: agent_core::TransferProgress = Arc::new(move |phase, done, total| {
            if let Ok(mut last) = reported.lock() {
                if last.0 == phase && last.1.elapsed() < std::time::Duration::from_millis(200) {
                    return;
                }
                *last = (phase.into(), std::time::Instant::now());
            }
            update(&callback_app, &callback_state, &callback_id, |view| {
                view.phase = phase.into();
                view.bytes_done = done;
                view.bytes_total = total;
            });
        });
        let mut owned_directory: Option<PathBuf> = None;
        let mut downloaded_file: Option<PathBuf> = None;
        let result = (|| -> Result<String, String> {
            match work {
                Work::Download {
                    path,
                    base,
                    confirmed_large,
                    custom_directory,
                    access,
                } => {
                    let parent = if custom_directory {
                        base
                    } else {
                        base.join(server)
                    };
                    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
                    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string();
                    let final_path =
                        parent.join(format!("{stamp}-{}", id.trim_start_matches("transfer-")));
                    let staging = parent.join(format!(".{id}.part"));
                    fs::create_dir(&staging).map_err(|error| error.to_string())?;
                    owned_directory = Some(staging.clone());
                    agent_core::download_ssh_browser_path_with_access(
                        &config,
                        password.as_deref(),
                        &path,
                        &staging,
                        confirmed_large,
                        &access,
                        cancellation.clone(),
                        progress,
                    )?;
                    if cancellation.load(Ordering::Relaxed) {
                        return Err("传输已取消".into());
                    }
                    let name = path
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .ok_or("下载文件名无效")?;
                    let output = if custom_directory {
                        downloaded_file = Some(agent_core::publish_browser_download(
                            &staging, &parent, name,
                        )?);
                        parent
                    } else {
                        fs::rename(&staging, &final_path).map_err(|error| error.to_string())?;
                        owned_directory = None;
                        downloaded_file = Some(final_path.join(name));
                        final_path
                    };
                    if let Some(file) = downloaded_file.as_ref() {
                        if let Err(error) = remember_download_path(&app, &state, file) {
                            eprintln!("无法保存下载路径记录：{error}");
                        }
                    }
                    Ok(output.to_string_lossy().into_owned())
                }
                Work::Upload {
                    paths,
                    destination,
                    policy,
                    conflicts,
                } => {
                    let temp = app
                        .path()
                        .app_cache_dir()
                        .map_err(|error| error.to_string())?
                        .join("file-transfers")
                        .join(&id);
                    fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
                    owned_directory = Some(temp.clone());
                    agent_core::upload_ssh_browser_paths(
                        &config,
                        password.as_deref(),
                        &paths,
                        &destination,
                        &temp.join("payload.tar.gz"),
                        policy,
                        &conflicts,
                        cancellation.clone(),
                        progress,
                    )?;
                    Ok(destination)
                }
            }
        })();
        if let Some(directory) = owned_directory {
            let _ = fs::remove_dir_all(directory);
        }
        agent_core::disconnect_ssh_connection(&config);
        update(&app, &state, &id, |view| {
            view.finished_at = Some(now());
            match result {
                Ok(path) => {
                    view.status = "completed".into();
                    view.phase = "completed".into();
                    view.output_path = Some(path);
                    view.file_path = downloaded_file
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned());
                }
                Err(error) => {
                    view.status = if cancellation.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    view.error = Some(error);
                }
            }
        });
    });
}
#[tauri::command]
pub fn list_code_transfers(
    state: tauri::State<'_, FileTransferState>,
) -> Result<Vec<TransferView>, String> {
    let jobs = state.0.lock().map_err(|_| "传输状态不可用")?;
    let mut views = jobs
        .values()
        .map(|job| job.view.clone())
        .collect::<Vec<_>>();
    views.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    Ok(views)
}
#[tauri::command]
pub fn cancel_code_transfer(
    state: tauri::State<'_, FileTransferState>,
    id: String,
) -> Result<(), String> {
    if let Some(job) = state.0.lock().map_err(|_| "传输状态不可用")?.get(&id) {
        job.cancellation.store(true, Ordering::Relaxed);
    }
    Ok(())
}
