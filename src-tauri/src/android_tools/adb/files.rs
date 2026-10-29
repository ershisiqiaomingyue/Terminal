use super::*;

pub fn read_adb_remote_text(serial: String, path: String) -> Result<AdbRemoteText, String> {
    let serial = checked_serial(&serial)?;
    let path = checked_remote_path(&path)?;
    let temporary = std::env::temp_dir().join(format!(
        "android-tool-read-{}-{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let temporary_argument = temporary.to_string_lossy().to_string();
    let output = adb_command_output(
        &["-s", serial, "pull", path, &temporary_argument],
        Duration::from_secs(60),
    )
    .map_err(|error| format!("无法拉取设备文件：{error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let result = (|| -> Result<AdbRemoteText, String> {
        let size = fs::metadata(&temporary)
            .map_err(|error| format!("无法读取临时文件：{error}"))?
            .len();
        if size > MAX_REMOTE_TEXT_BYTES {
            return Err(format!(
                "仅支持编辑不超过 {} MB 的文本文件。",
                MAX_REMOTE_TEXT_BYTES / 1024 / 1024
            ));
        }
        let content = fs::read_to_string(&temporary)
            .map_err(|_| "该文件不是 UTF-8 文本，不能在内置编辑器中修改。".to_string())?;
        Ok(AdbRemoteText { content, size })
    })();
    let _ = fs::remove_file(&temporary);
    let AdbRemoteText { content, size } = result?;
    Ok(AdbRemoteText { content, size })
}

#[tauri::command]
pub fn write_adb_remote_text(app: AppHandle, request: AdbWriteTextRequest) -> Result<(), String> {
    let serial = checked_serial(&request.serial)?;
    let path = checked_remote_path(&request.path)?;
    if request.content.as_bytes().len() as u64 > MAX_REMOTE_TEXT_BYTES {
        return Err(format!(
            "仅支持写入不超过 {} MB 的文本文件。",
            MAX_REMOTE_TEXT_BYTES / 1024 / 1024
        ));
    }
    if is_protected_remote_path(path) {
        if !request.confirm_protected {
            return Err("写入系统受保护分区需要在界面中明确授权。".to_string());
        }
        if !get_adb_privilege_status(serial.to_string())?.is_root {
            return Err("写入系统分区需要 Root adbd；量产版设备通常不支持。".to_string());
        }
    }
    let temporary = std::env::temp_dir().join(format!(
        "android-tool-edit-{}-{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::write(&temporary, &request.content)
        .map_err(|error| format!("无法创建临时文件：{error}"))?;
    let temporary_argument = temporary.to_string_lossy().to_string();
    let result = adb_command_output(
        &["-s", serial, "push", &temporary_argument, path],
        Duration::from_secs(60),
    );
    let _ = fs::remove_file(&temporary);
    let output = result.map_err(|error| format!("无法写入设备文件：{error}"))?;
    let result = if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    };
    let detail = result
        .as_ref()
        .map(|_| format!("已写入 {} 字节", request.content.len()))
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        "写入设备文本",
        &format!("{serial}:{path}"),
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

#[tauri::command]
pub fn pull_adb_file(app: AppHandle, request: AdbTransferRequest) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let remote_path = checked_remote_path(&request.remote_path)?;
    let destination = fs::canonicalize(request.local_path.trim())
        .map_err(|error| format!("无法访问本地目录：{error}"))?;
    if !destination.is_dir() {
        return Err("请选择本地目录。".to_string());
    }
    let destination_argument = destination.to_string_lossy().to_string();
    let output = adb_command_output(
        &["-s", serial, "pull", remote_path, &destination_argument],
        Duration::from_secs(10 * 60),
    )
    .map_err(|error| format!("无法拉取设备文件：{error}"))?;
    let result = if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    };
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "拉取设备文件",
        &format!("{serial}:{remote_path}"),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn push_adb_file(app: AppHandle, request: AdbTransferRequest) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let remote_path = checked_remote_path(&request.remote_path)?;
    let source = fs::canonicalize(request.local_path.trim())
        .map_err(|error| format!("无法读取本地文件：{error}"))?;
    if !source.is_file() {
        return Err("请选择一个本地文件。".to_string());
    }
    let source_argument = source.to_string_lossy().to_string();
    let output = adb_command_output(
        &["-s", serial, "push", &source_argument, remote_path],
        Duration::from_secs(10 * 60),
    )
    .map_err(|error| format!("无法推送设备文件：{error}"))?;
    let result = if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    };
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "推送设备文件",
        &format!("{serial}:{remote_path}"),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn create_adb_directory(app: AppHandle, request: AdbRemotePathRequest) -> Result<(), String> {
    let serial = checked_serial(&request.serial)?;
    let path = checked_remote_path(&request.path)?;
    let result = adb_output(&["-s", serial, "shell", "mkdir", "-p", "--", path]);
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "创建设备目录",
        &format!("{serial}:{path}"),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result.map(|_| ())
}

#[tauri::command]
pub fn delete_adb_remote_path(
    app: AppHandle,
    request: AdbDeleteRequest,
) -> Result<AdbBackupEntry, String> {
    let serial = checked_serial(&request.serial)?;
    let path = checked_remote_path(&request.path)?;
    if !request.confirmed {
        return Err("删除操作需要在界面中确认。".to_string());
    }
    if matches!(path, "/" | "/sdcard" | "/data") || is_protected_remote_path(path) {
        return Err("不能删除设备根目录、关键目录或系统受保护分区。".to_string());
    }
    let backup = backup_remote_path(&app, serial, path, request.is_directory)?;
    adb_output(&["-s", serial, "shell", "rm", "-rf", "--", path])?;
    let _ = android_tasks::record_operation_audit(
        &app,
        "删除设备文件",
        &format!("{serial}:{path}"),
        "done",
        &format!("已创建本地备份：{}", backup.local_path),
    );
    Ok(backup)
}

#[tauri::command]
pub fn capture_adb_screenshot(
    app: AppHandle,
    request: AdbTransferRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let destination = PathBuf::from(request.local_path.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "截图保存路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建截图目录：{error}"))?;
    let output = adb_command_output_with_limit(
        &["-s", serial, "exec-out", "screencap", "-p"],
        Duration::from_secs(30),
        MAX_SCREENSHOT_OUTPUT_BYTES,
    )
    .map_err(|error| format!("无法截图：{error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if !is_png_bytes(&output.stdout) {
        let detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if detail.is_empty() {
            "设备没有返回有效 PNG 截图。".to_string()
        } else {
            format!("设备未返回有效 PNG 截图：{detail}")
        });
    }
    fs::write(&destination, output.stdout).map_err(|error| format!("无法保存截图：{error}"))?;
    let destination = destination.to_string_lossy().to_string();
    let _ = android_tasks::record_operation_audit(
        &app,
        "设备截图",
        serial,
        "done",
        &format!("已保存到 {destination}"),
    );
    Ok(destination)
}
