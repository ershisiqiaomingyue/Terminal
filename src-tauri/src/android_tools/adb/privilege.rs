use super::*;

pub fn get_adb_privilege_status(serial: String) -> Result<AdbPrivilegeStatus, String> {
    let serial = checked_serial(&serial)?;
    let id = adb_output(&["-s", serial, "shell", "id"])?;
    let properties_output = adb_output(&["-s", serial, "shell", "getprop"])?;
    let properties = properties_output
        .lines()
        .filter_map(|line| {
            let (name, value) = line.trim().strip_prefix('[')?.split_once("]: [")?;
            Some((name, value.strip_suffix(']')?))
        })
        .collect::<HashMap<_, _>>();
    let property = |name: &str| properties.get(name).copied().unwrap_or_default();
    let build_type = property("ro.build.type").to_string();
    let debuggable = property("ro.debuggable") == "1";
    let secure = property("ro.secure") != "0";
    let selinux = adb_output(&["-s", serial, "shell", "getenforce"])
        .unwrap_or_else(|_| "未知".to_string())
        .trim()
        .to_string();
    let verified_boot_state = property("ro.boot.verifiedbootstate").to_string();
    let bootloader_locked = match property("ro.boot.flash.locked") {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    let oem_unlock_supported = match property("ro.oem_unlock_supported") {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    let slot_suffix = property("ro.boot.slot_suffix").to_string();
    let can_request_root = debuggable || matches!(build_type.as_str(), "userdebug" | "eng");
    Ok(AdbPrivilegeStatus {
        is_root: id.contains("uid=0("),
        build_type: if build_type.is_empty() {
            "未知".to_string()
        } else {
            build_type
        },
        debuggable,
        secure,
        selinux: if selinux.is_empty() {
            "未知".to_string()
        } else {
            selinux
        },
        verified_boot_state: if verified_boot_state.is_empty() {
            "未知".to_string()
        } else {
            verified_boot_state
        },
        bootloader_locked,
        oem_unlock_supported,
        slot_suffix: if slot_suffix.is_empty() {
            "未知".to_string()
        } else {
            slot_suffix
        },
        can_request_root,
    })
}

#[tauri::command]
pub fn request_adb_root(app: AppHandle, serial: String) -> Result<AdbPrivilegeStatus, String> {
    let serial = checked_serial(&serial)?.to_string();
    adb_output(&["-s", &serial, "root"])?;
    std::thread::sleep(Duration::from_millis(900));
    let result = get_adb_privilege_status(serial.clone());
    let detail = result
        .as_ref()
        .map(|status| {
            format!(
                "adbd Root 状态：{}",
                if status.is_root {
                    "已获取"
                } else {
                    "未获取"
                }
            )
        })
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        "请求 ADB Root",
        &serial,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

#[tauri::command]
pub fn remount_adb_system(app: AppHandle, serial: String) -> Result<String, String> {
    let serial = checked_serial(&serial)?;
    let status = get_adb_privilege_status(serial.to_string())?;
    if !status.is_root {
        return Err(
            "当前 adbd 不是 Root，请先请求 Root 权限。量产版设备通常不支持此操作。".to_string(),
        );
    }
    let result = adb_output(&["-s", serial, "remount"]);
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "重新挂载系统分区",
        serial,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}
