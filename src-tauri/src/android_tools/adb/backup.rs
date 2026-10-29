use super::*;

pub fn list_adb_remote_files(serial: String, path: String) -> Result<Vec<AdbRemoteFile>, String> {
    let serial = checked_serial(&serial)?;
    let path = checked_remote_path(&path)?;
    let output = adb_output(&["-s", serial, "shell", "ls", "-la", "--", path])?;
    Ok(parse_adb_remote_files(path, &output))
}

pub fn list_adb_backups(app: AppHandle, serial: String) -> Result<Vec<AdbBackupEntry>, String> {
    let serial = checked_serial(&serial)?;
    let root = backup_root(&app, serial)?;
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("无法读取备份目录：{error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read(backup_metadata_path(&entry.path())).ok())
        .filter_map(|data| serde_json::from_slice::<AdbBackupEntry>(&data).ok())
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(entries)
}

pub fn get_adb_backup_directory(app: AppHandle, serial: String) -> Result<String, String> {
    let serial = checked_serial(&serial)?;
    Ok(backup_root(&app, serial)?.to_string_lossy().to_string())
}

#[tauri::command]
pub fn restore_adb_backup(app: AppHandle, request: AdbBackupRestoreRequest) -> Result<(), String> {
    let serial = checked_serial(&request.serial)?;
    if request.id.contains('/') || request.id.contains('\\') || request.id.contains("..") {
        return Err("备份标识无效。".to_string());
    }
    let directory = backup_root(&app, serial)?.join(&request.id);
    let data =
        fs::read(backup_metadata_path(&directory)).map_err(|_| "未找到备份元数据。".to_string())?;
    let entry: AdbBackupEntry =
        serde_json::from_slice(&data).map_err(|_| "备份元数据无效。".to_string())?;
    if entry.serial != serial {
        return Err("该备份不属于当前设备。".to_string());
    }
    let payload = directory.join("payload");
    if !payload.exists() {
        return Err("备份文件不存在。".to_string());
    }
    let target = if entry.is_directory {
        Path::new(&entry.remote_path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("/")
    } else {
        entry.remote_path.as_str()
    };
    let payload_argument = payload.to_string_lossy().to_string();
    let output = adb_command_output(
        &["-s", serial, "push", &payload_argument, target],
        Duration::from_secs(10 * 60),
    )
    .map_err(|error| format!("无法恢复备份：{error}"))?;
    let result = if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    };
    let detail = result
        .as_ref()
        .map(|_| format!("已恢复 {}", entry.remote_path))
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        "恢复设备文件备份",
        &format!("{serial}:{}", entry.remote_path),
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}
