use super::*;

#[tauri::command]
pub fn run_adb_package_action(
    app: AppHandle,
    serial: String,
    package_name: String,
    action: String,
) -> Result<String, String> {
    let serial = checked_serial(&serial)?;
    let package_name = checked_package(&package_name)?;
    let args: Vec<&str> = match action.as_str() {
        "launch" => vec![
            "-s",
            serial,
            "shell",
            "monkey",
            "-p",
            package_name,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
        "force-stop" => vec!["-s", serial, "shell", "am", "force-stop", package_name],
        "clear-data" => vec!["-s", serial, "shell", "pm", "clear", package_name],
        "uninstall" => vec!["-s", serial, "uninstall", package_name],
        "disable" => vec![
            "-s",
            serial,
            "shell",
            "pm",
            "disable-user",
            "--user",
            "0",
            package_name,
        ],
        "enable" => vec!["-s", serial, "shell", "pm", "enable", package_name],
        _ => return Err("不支持的应用操作。".to_string()),
    };
    let result = adb_output(&args);
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        &format!("应用操作：{action}"),
        &format!("{serial}:{package_name}"),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

pub fn get_adb_package_details(
    serial: String,
    package_name: String,
) -> Result<AdbPackageDetails, String> {
    let serial = checked_serial(&serial)?;
    let package_name = checked_package(&package_name)?;
    // Some vendor builds do not register a `package` dumpsys service even
    // though the package manager shell command is available. Details should
    // remain usable in that case instead of failing the entire panel.
    let dump = adb_output(&["-s", serial, "shell", "pm", "dump", package_name])
        .or_else(|_| adb_output(&["-s", serial, "shell", "dumpsys", "package", package_name]))
        .unwrap_or_default();
    let paths = adb_output(&["-s", serial, "shell", "pm", "path", package_name])?
        .lines()
        .filter_map(|line| line.trim().strip_prefix("package:").map(str::to_string))
        .collect::<Vec<_>>();
    let field = |name: &str| {
        dump.lines()
            .find_map(|line| {
                line.trim().strip_prefix(name).map(str::trim).map(|value| {
                    value
                        .split_whitespace()
                        .next()
                        .unwrap_or("未知")
                        .to_string()
                })
            })
            .unwrap_or_else(|| "未知".to_string())
    };
    let user = adb_output(&["-s", serial, "shell", "am", "get-current-user"])
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let permissions: Vec<AdbPermission> = crate::adb_metadata::parse_permissions(&dump, user)
        .into_iter()
        .map(|(name, granted)| AdbPermission { name, granted })
        .collect();
    Ok(AdbPackageDetails {
        package_name: package_name.to_string(),
        version_name: field("versionName="),
        version_code: field("versionCode="),
        uid: field("userId="),
        enabled: field("enabled="),
        source_paths: paths,
        requested_permissions: permissions
            .iter()
            .map(|permission| permission.name.clone())
            .collect(),
        permissions,
        enabled_state: crate::adb_metadata::parse_enabled(&dump, user),
    })
}

#[tauri::command]
pub fn export_adb_package(
    app: AppHandle,
    request: AdbPackageExportRequest,
) -> Result<Vec<String>, String> {
    let serial = checked_serial(&request.serial)?;
    let package_name = checked_package(&request.package_name)?;
    let destination = fs::canonicalize(request.destination.trim())
        .map_err(|error| format!("无法访问导出目录：{error}"))?;
    if !destination.is_dir() {
        return Err("导出位置必须是一个目录。".to_string());
    }
    let paths = adb_output(&["-s", serial, "shell", "pm", "path", package_name])?;
    let mut saved = Vec::new();
    for (index, remote) in paths
        .lines()
        .filter_map(|line| line.trim().strip_prefix("package:"))
        .enumerate()
    {
        let file_name = Path::new(remote)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("base.apk");
        let local_name = if index == 0 {
            format!("{package_name}.apk")
        } else {
            format!("{package_name}-{file_name}")
        };
        let target = destination.join(local_name);
        let target_argument = target.to_string_lossy().to_string();
        let output = adb_command_output(
            &["-s", serial, "pull", remote, &target_argument],
            Duration::from_secs(10 * 60),
        )
        .map_err(|error| format!("无法导出 APK：{error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        saved.push(target.to_string_lossy().to_string());
    }
    if saved.is_empty() {
        return Err("未找到该应用的 APK 路径。".to_string());
    }
    let _ = android_tasks::record_operation_audit(
        &app,
        "导出设备 APK",
        &format!("{serial}:{package_name}"),
        "done",
        &format!("已导出 {} 个 APK 到 {}", saved.len(), destination.display()),
    );
    Ok(saved)
}

#[tauri::command]
pub fn install_adb_package(
    app: AppHandle,
    request: AdbPackageInstallRequest,
) -> Result<String, String> {
    install_adb_package_with_audit(app, request, None)
}
