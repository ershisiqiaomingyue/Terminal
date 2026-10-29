use super::*;

#[tauri::command]
pub fn restart_adb_device(app: AppHandle, serial: String, mode: String) -> Result<(), String> {
    let serial = checked_serial(&serial)?;
    let (action, args) = match mode.as_str() {
        "normal" => ("重启设备", vec!["-s", serial, "reboot"]),
        "recovery" => ("重启到 Recovery", vec!["-s", serial, "reboot", "recovery"]),
        "bootloader" => (
            "重启到 Bootloader",
            vec!["-s", serial, "reboot", "bootloader"],
        ),
        _ => return Err("不支持的重启模式。".to_string()),
    };
    let result = adb_output(&args);
    let detail = match &result {
        Ok(output) => output.as_str(),
        Err(error) => error.as_str(),
    };
    let _ = android_tasks::record_operation_audit(
        &app,
        action,
        serial,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result.map(|_| ())
}

#[tauri::command]
pub fn run_adb_device_action(
    app: AppHandle,
    serial: String,
    action: String,
) -> Result<String, String> {
    let serial = checked_serial(&serial)?;
    let action = match action.as_str() {
        "vol-up" => "volume-up",
        "vol-down" => "volume-down",
        "vol-mute" => "volume-mute",
        other => other,
    };
    if action == "rotate" {
        let current = adb_output(&[
            "-s",
            serial,
            "shell",
            "settings",
            "get",
            "system",
            "user_rotation",
        ])
        .unwrap_or_default();
        let next = match current.trim() {
            "0" => "1",
            "1" => "2",
            "2" => "3",
            _ => "0",
        };
        let _ = adb_output(&[
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "system",
            "accelerometer_rotation",
            "0",
        ]);
        let result = adb_output(&[
            "-s",
            serial,
            "shell",
            "settings",
            "put",
            "system",
            "user_rotation",
            next,
        ]);
        let detail = match &result {
            Ok(output) => output.as_str(),
            Err(error) => error.as_str(),
        };
        let _ = android_tasks::record_operation_audit(
            &app,
            "旋转设备屏幕",
            serial,
            if result.is_ok() { "done" } else { "error" },
            detail,
        );
        return result.map(|_| format!("user_rotation={next}"));
    }
    let (label, args): (&str, Vec<&str>) = match action {
        "wake" => (
            "唤醒设备屏幕",
            vec!["-s", serial, "shell", "input", "keyevent", "KEYCODE_WAKEUP"],
        ),
        "sleep" => (
            "关闭设备屏幕",
            vec!["-s", serial, "shell", "input", "keyevent", "KEYCODE_SLEEP"],
        ),
        "power" => (
            "设备电源键",
            vec!["-s", serial, "shell", "input", "keyevent", "KEYCODE_POWER"],
        ),
        "home" => (
            "设备返回桌面",
            vec!["-s", serial, "shell", "input", "keyevent", "KEYCODE_HOME"],
        ),
        "back" => (
            "设备返回",
            vec!["-s", serial, "shell", "input", "keyevent", "KEYCODE_BACK"],
        ),
        "app-switch" => (
            "打开近期任务",
            vec![
                "-s",
                serial,
                "shell",
                "input",
                "keyevent",
                "KEYCODE_APP_SWITCH",
            ],
        ),
        "volume-up" => (
            "提高设备音量",
            vec![
                "-s",
                serial,
                "shell",
                "input",
                "keyevent",
                "KEYCODE_VOLUME_UP",
            ],
        ),
        "volume-down" => (
            "降低设备音量",
            vec![
                "-s",
                serial,
                "shell",
                "input",
                "keyevent",
                "KEYCODE_VOLUME_DOWN",
            ],
        ),
        "volume-mute" => (
            "设备静音",
            vec![
                "-s",
                serial,
                "shell",
                "input",
                "keyevent",
                "KEYCODE_VOLUME_MUTE",
            ],
        ),
        "clear-logcat" => ("清空设备 Logcat", vec!["-s", serial, "logcat", "-c"]),
        _ => return Err("不支持的设备操作。".to_string()),
    };
    let result = adb_output(&args);
    let detail = match &result {
        Ok(output) => output.as_str(),
        Err(error) => error.as_str(),
    };
    let _ = android_tasks::record_operation_audit(
        &app,
        label,
        serial,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}
