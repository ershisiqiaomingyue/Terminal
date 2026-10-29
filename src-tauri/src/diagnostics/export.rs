use super::{store, STORE};
use serde::Serialize;
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogInfo {
    directory: String,
    run_id: String,
    enabled: bool,
    last_error: Option<String>,
    file_count: usize,
    total_bytes: u64,
    max_file_bytes: u64,
    max_files: usize,
    retention_days: i64,
}

#[tauri::command]
pub fn get_diagnostic_log_info() -> Result<LogInfo, String> {
    let store = STORE
        .get()
        .ok_or("诊断日志未初始化")?
        .lock()
        .map_err(|error| error.to_string())?;
    let files = store::log_files(&store.directory).map_err(|error| error.to_string())?;
    Ok(LogInfo {
        directory: store.directory.to_string_lossy().into_owned(),
        run_id: store.run_id.clone(),
        enabled: store.current.is_some() && store.last_error.is_none(),
        last_error: store.last_error.clone(),
        file_count: files.len(),
        total_bytes: files
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum(),
        max_file_bytes: store::MAX_FILE_BYTES,
        max_files: store::MAX_FILES,
        retention_days: store::RETENTION_DAYS,
    })
}

#[tauri::command]
pub fn open_diagnostic_log_directory(app: AppHandle) -> Result<(), String> {
    let info = get_diagnostic_log_info()?;
    app.opener()
        .open_path(info.directory, None::<String>)
        .map_err(|error| error.to_string())
}

fn snapshot() -> Result<(String, Vec<(String, Vec<u8>)>), String> {
    let store = STORE
        .get()
        .ok_or("诊断日志未初始化")?
        .lock()
        .map_err(|error| error.to_string())?;
    let mut result = Vec::new();
    for path in store::log_files(&store.directory).map_err(|error| error.to_string())? {
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| {
                file.take(store::MAX_FILE_BYTES + 64 * 1024)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| error.to_string())?;
        result.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            bytes,
        ));
    }
    // Preserve earlier WebView incidents too, and scrub their older unstructured format.
    for name in ["webview-health.log", "webview-health.previous.log"] {
        if let Ok(file) = fs::File::open(store.directory.join(name)) {
            let mut text = String::new();
            let _ = file.take(2 * 1024 * 1024).read_to_string(&mut text);
            let clean = text
                .lines()
                .map(store::redact)
                .collect::<Vec<_>>()
                .join("\n");
            result.push((name.into(), clean.into_bytes()));
        }
    }
    for entry in fs::read_dir(&store.directory)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("panic-") && name.ends_with(".json") {
            if let Ok(file) = fs::File::open(entry.path()) {
                let mut text = String::new();
                let _ = file.take(32 * 1024).read_to_string(&mut text);
                result.push((name, store::redact(&text).into_bytes()));
            }
        }
    }
    Ok((store.run_id.clone(), result))
}

#[tauri::command]
pub async fn export_diagnostic_logs(app: AppHandle) -> Result<String, String> {
    let downloads = app
        .path()
        .download_dir()
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let (run_id, files) = snapshot()?;
        fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
        let filename = format!("Terminal-diagnostics-{}-{}.zip", chrono::Local::now().format("%Y%m%d-%H%M%S"), run_id);
        let mut path: PathBuf = downloads.join(&filename);
        let file = (0..100).find_map(|index| {
            if index > 0 { path = downloads.join(format!("{} ({index}).zip", filename.trim_end_matches(".zip"))); }
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) { Ok(file) => Some(Ok(file)), Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None, Err(error) => Some(Err(error)) }
        }).ok_or("无法创建诊断包")?.map_err(|error| error.to_string())?;
        let result = (|| -> Result<(), String> {
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            archive.start_file("manifest.json", options).map_err(|error| error.to_string())?;
            archive.write_all(json!({"schemaVersion":1,"exportedAt":chrono::Utc::now().to_rfc3339(),"runId":run_id,"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"files":files.iter().map(|(name,_)| name).collect::<Vec<_>>(),"retentionDays":store::RETENTION_DAYS}).to_string().as_bytes()).map_err(|error| error.to_string())?;
            for (name, bytes) in files { archive.start_file(name, options).map_err(|error| error.to_string())?; archive.write_all(&bytes).map_err(|error| error.to_string())?; }
            archive.finish().and_then(|file| file.sync_all().map(|_| file).map_err(zip::result::ZipError::Io)).map_err(|error| error.to_string())?;
            Ok(())
        })();
        if let Err(error) = result { let _ = fs::remove_file(&path); return Err(error); }
        super::record("info", "diagnostics.exported", "native", json!({"filename":path.file_name().unwrap().to_string_lossy()}));
        Ok(path.to_string_lossy().into_owned())
    }).await.map_err(|error| error.to_string())?
}
