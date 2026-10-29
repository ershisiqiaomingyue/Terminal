use super::*;

pub fn list_adb_devices() -> Result<Vec<AdbDevice>, String> {
    let output = adb_output(&["devices", "-l"])?;
    Ok(output
        .lines()
        .skip_while(|line| !line.starts_with("List of devices attached"))
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 2 || fields[0].starts_with('*') {
                return None;
            }
            let lookup = |prefix: &str| {
                fields
                    .iter()
                    .find_map(|field| field.strip_prefix(prefix).map(str::to_string))
            };
            Some(AdbDevice {
                serial: fields[0].to_string(),
                state: fields[1].to_string(),
                product: lookup("product:"),
                model: lookup("model:"),
                device: lookup("device:"),
                transport_id: lookup("transport_id:"),
            })
        })
        .collect())
}

/// ADB 服务（`adb server`）是否已经在跑。
///
/// 只看 127.0.0.1:5037 有没有监听，**不会**去拉起 adb：用户在终端页里没打开
/// adb 之前，任何自动刷新都不该偷偷把 adb server 启起来。

pub fn get_adb_device_info(serial: String) -> Result<AdbDeviceInfo, String> {
    let serial = serial.trim();
    if serial.is_empty() {
        return Err("请选择设备。".to_string());
    }
    let output = adb_output(&["-s", serial, "shell", "getprop"])?;
    let properties = output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().strip_prefix('[')?.split_once("]: [")?;
            Some((key, value.strip_suffix(']')?))
        })
        .collect::<HashMap<_, _>>();
    let property = |key: &str| properties.get(key).copied().unwrap_or("未知").to_string();
    Ok(AdbDeviceInfo {
        android_version: property("ro.build.version.release"),
        sdk: property("ro.build.version.sdk"),
        manufacturer: property("ro.product.manufacturer"),
        model: property("ro.product.model"),
        build_fingerprint: property("ro.build.fingerprint"),
    })
}

pub fn list_adb_packages(serial: String) -> Result<Vec<String>, String> {
    let serial = serial.trim();
    if serial.is_empty() {
        return Err("请选择设备。".to_string());
    }
    let output = adb_output(&["-s", serial, "shell", "pm", "list", "packages"])?;
    let mut packages = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("package:").map(str::to_string))
        .collect::<Vec<_>>();
    packages.sort_unstable();
    packages.dedup();
    Ok(packages)
}

pub fn list_adb_processes(serial: String) -> Result<HashMap<String, String>, String> {
    let serial = serial.trim();
    if serial.is_empty() {
        return Err("请选择设备。".to_string());
    }
    let output = adb_output(&["-s", serial, "shell", "ps", "-A", "-o", "PID,NAME"])?;
    Ok(output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?;
            let name = fields.next()?;
            pid.parse::<u32>()
                .ok()
                .map(|_| (pid.to_string(), name.to_string()))
        })
        .collect())
}

pub fn get_adb_device_status(serial: String) -> Result<AdbDeviceStatus, String> {
    let serial = checked_serial(&serial)?;
    let properties = adb_output(&["-s", serial, "shell", "getprop"])?;
    let package_probe = adb_output(&["-s", serial, "shell", "pm", "list", "packages"]);
    let service_message = match &package_probe {
        Ok(output) if android_service_unavailable_message(output).is_some() => {
            android_service_unavailable_message(output)
        }
        Ok(_) => None,
        Err(error) => Some(format!("无法访问 PackageManager：{error}")),
    };
    let battery = adb_output(&["-s", serial, "shell", "dumpsys", "battery"]).unwrap_or_default();
    let storage = adb_output(&["-s", serial, "shell", "df", "/data"]).unwrap_or_default();
    let battery_value = |key: &str| {
        battery
            .lines()
            .find_map(|line| line.trim().strip_prefix(key).map(str::trim))
            .unwrap_or("未知")
            .to_string()
    };
    let property_value = |key: &str| {
        properties
            .lines()
            .find_map(|line| {
                let (name, value) = line.trim().strip_prefix('[')?.split_once("]: [")?;
                (name == key).then(|| value.strip_suffix(']').unwrap_or(value).to_string())
            })
            .unwrap_or_else(|| "未知".to_string())
    };
    let status = match battery_value("status:").as_str() {
        "2" => "充电中",
        "3" => "放电中",
        "4" => "未充电",
        "5" => "已充满",
        _ => "未知",
    }
    .to_string();
    let storage_line = storage
        .lines()
        .nth(1)
        .unwrap_or("未知")
        .split_whitespace()
        .collect::<Vec<_>>();
    let format_kib = |value: &str| {
        value
            .parse::<f64>()
            .map(|kib| {
                if kib >= 1024.0 * 1024.0 {
                    format!("{:.1} G", kib / 1024.0 / 1024.0)
                } else {
                    format!("{:.0} M", kib / 1024.0)
                }
            })
            .unwrap_or_else(|_| "未知".to_string())
    };
    let storage = if storage_line.len() >= 5 {
        format!(
            "已用 {} / {}（{} 可用）",
            format_kib(storage_line[2]),
            format_kib(storage_line[1]),
            format_kib(storage_line[3])
        )
    } else {
        "未知".to_string()
    };
    Ok(AdbDeviceStatus {
        battery_level: format!("{}%", battery_value("level:")),
        battery_status: status,
        battery_temperature: battery_value("temperature:")
            .parse::<f32>()
            .map(|value| format!("{:.1} °C", value / 10.0))
            .unwrap_or_else(|_| "未知".to_string()),
        storage,
        abi: property_value("ro.product.cpu.abi"),
        security_patch: property_value("ro.build.version.security_patch"),
        system_services_ready: service_message.is_none(),
        system_services_message: service_message.unwrap_or_default(),
    })
}

pub fn get_adb_storage_info(serial: String, path: String) -> Result<AdbStorageInfo, String> {
    let serial = checked_serial(&serial)?;
    let path = checked_remote_path(&path)?;
    let output = adb_output(&["-s", serial, "shell", "df", "-k", &aosp_shell_quote(path)])?;
    let (total_bytes, available_bytes) = crate::adb_metadata::parse_storage(&output)
        .ok_or_else(|| "设备未返回有效的存储容量。".to_string())?;
    Ok(AdbStorageInfo {
        total_bytes,
        available_bytes,
    })
}

pub fn get_adb_capture_info(serial: String) -> Result<AdbCaptureInfo, String> {
    let serial = checked_serial(&serial)?;
    let size = adb_output(&["-s", serial, "shell", "wm", "size"])?;
    let density = adb_output(&["-s", serial, "shell", "wm", "density"]).unwrap_or_default();
    Ok(AdbCaptureInfo {
        physical_size: crate::adb_metadata::parse_physical_value(&size, "Physical size:"),
        physical_density: crate::adb_metadata::parse_physical_value(&density, "Physical density:"),
    })
}

pub fn list_adb_applications(serial: String) -> Result<Vec<AdbApplication>, String> {
    let serial = checked_serial(&serial)?;
    let output = adb_output(&["-s", serial, "shell", "pm", "list", "packages", "-f"])?;
    let mut applications = output
        .lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("package:")?;
            let (source_path, package_name) = value.rsplit_once('=')?;
            Some(AdbApplication {
                package_name: package_name.to_string(),
                source_path: source_path.to_string(),
                kind: if source_path.starts_with("/data/") {
                    "用户".to_string()
                } else {
                    "系统".to_string()
                },
            })
        })
        .collect::<Vec<_>>();
    applications.sort_by(|left, right| left.package_name.cmp(&right.package_name));
    Ok(applications)
}

pub fn list_adb_process_details(serial: String) -> Result<Vec<AdbProcess>, String> {
    let serial = checked_serial(&serial)?;
    let output = adb_output(&["-s", serial, "shell", "ps", "-A", "-o", "PID,USER,NAME"])?;
    Ok(output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(AdbProcess {
                pid: fields.next()?.to_string(),
                user: fields.next()?.to_string(),
                name: fields.next()?.to_string(),
            })
        })
        .collect())
}
