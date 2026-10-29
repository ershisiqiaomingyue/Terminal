use super::*;

const ADB_SERVER_PORT: u16 = 5037;

pub fn adb_server_running() -> bool {
    probe_listener(ADB_SERVER_PORT, Duration::from_millis(120))
}

pub(super) fn probe_listener(port: u16, timeout: Duration) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, timeout).is_ok()
}

/// 被动刷新：服务没在跑就直接返回空列表，绝不触发 `adb start-server`。
/// 界面上的自动轮询（进页面、定时器）走这个，用户手动点刷新才走
/// [`list_adb_devices`]。
pub fn list_adb_devices_passive() -> Result<Vec<AdbDevice>, String> {
    if !adb_server_running() {
        return Ok(Vec::new());
    }
    list_adb_devices()
}

/// Execute one explicitly requested command inside the selected device.
pub fn run_adb_shell(
    app: AppHandle,
    serial: String,
    command: String,
    timeout_ms: Option<u64>,
) -> Result<AdbShellResult, String> {
    let serial = checked_serial(&serial)?.to_string();
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("ADB shell 命令不能为空。".to_string());
    }
    if command.len() > 32 * 1024
        || command.contains('\0')
        || command.contains('\r')
        || command.contains('\n')
    {
        return Err("ADB shell 命令长度或字符无效。".to_string());
    }
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000).clamp(100, 120_000));
    let args = ["-s", serial.as_str(), "shell", "sh", "-c", command.as_str()];
    let output = adb_command_output_with_limit(&args, timeout, 1024 * 1024)
        .map_err(|error| format!("ADB shell 执行失败：{error}"))?;
    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut truncated = false;
    const MAX_TEXT: usize = 512 * 1024;
    if stdout.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !stdout.is_char_boundary(end) {
            end -= 1;
        }
        stdout.truncate(end);
        stdout.push_str("\n[stdout 已截断]");
        truncated = true;
    }
    if stderr.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !stderr.is_char_boundary(end) {
            end -= 1;
        }
        stderr.truncate(end);
        stderr.push_str("\n[stderr 已截断]");
        truncated = true;
    }
    let result = AdbShellResult {
        serial: serial.clone(),
        command: command.clone(),
        stdout,
        stderr,
        exit_code: output.status.code(),
        truncated,
    };
    let detail = format!(
        "退出码 {:?}，stdout {} 字节，stderr {} 字节",
        result.exit_code,
        result.stdout.len(),
        result.stderr.len()
    );
    let _ = android_tasks::record_operation_audit(
        &app,
        "ADB Shell",
        &serial,
        if output.status.success() {
            "done"
        } else {
            "error"
        },
        &format!("{detail}；命令：{command}"),
    );
    Ok(result)
}

#[tauri::command]
pub fn restart_adb(app: AppHandle) -> Result<(), String> {
    let result = adb_output(&["start-server"]);
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "启动 ADB 服务",
        "本机 ADB Server",
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result.map(|_| ())
}

#[tauri::command]
pub fn pair_adb_wireless(
    app: AppHandle,
    request: AdbWirelessPairRequest,
) -> Result<String, String> {
    let endpoint = checked_adb_wireless_endpoint(&request.endpoint)?;
    let code = checked_adb_pairing_code(&request.pairing_code)?;
    let result = adb_output(&["pair", &endpoint, &code]).map(|output| output.trim().to_string());
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "无线 ADB 配对",
        &endpoint,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn connect_adb_wireless(
    app: AppHandle,
    request: AdbWirelessConnectRequest,
) -> Result<String, String> {
    let endpoint = checked_adb_wireless_endpoint(&request.endpoint)?;
    let result = adb_output(&["connect", &endpoint]).map(|output| output.trim().to_string());
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "无线 ADB 连接",
        &endpoint,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn disconnect_adb_wireless(
    app: AppHandle,
    request: AdbWirelessConnectRequest,
) -> Result<String, String> {
    let endpoint = checked_adb_wireless_endpoint(&request.endpoint)?;
    let result = adb_output(&["disconnect", &endpoint]).map(|output| output.trim().to_string());
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "无线 ADB 断开",
        &endpoint,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}
