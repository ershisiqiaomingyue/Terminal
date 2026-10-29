use super::*;

pub(crate) fn checked_serial(serial: &str) -> Result<&str, String> {
    let serial = serial.trim();
    if serial.is_empty() {
        Err("请选择设备。".to_string())
    } else {
        Ok(serial)
    }
}

pub(crate) fn checked_package(package_name: &str) -> Result<&str, String> {
    let package_name = package_name.trim();
    if package_name.is_empty()
        || !package_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '$')
        })
    {
        return Err("包名格式无效。".to_string());
    }
    Ok(package_name)
}

pub(crate) fn checked_remote_path(path: &str) -> Result<&str, String> {
    let path = path.trim();
    if !path.starts_with('/') || path.contains('\0') || path.contains('\n') || path.contains('\r') {
        return Err("设备路径无效。".to_string());
    }
    Ok(path)
}

fn remote_join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

pub(crate) fn parse_adb_remote_files(parent: &str, output: &str) -> Vec<AdbRemoteFile> {
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 {
            continue;
        }
        let name_index = if fields.len() >= 8 {
            7
        } else {
            fields.len() - 1
        };
        let name = fields[name_index..].join(" ");
        if name == "." || name == ".." {
            continue;
        }
        let is_directory = fields[0].starts_with('d');
        let size = fields.get(4).and_then(|value| value.parse::<u64>().ok());
        let modified = if fields.len() >= 7 {
            fields[5..name_index].join(" ")
        } else {
            String::new()
        };
        files.push(AdbRemoteFile {
            path: remote_join(parent, &name),
            name,
            is_directory,
            size,
            modified,
        });
    }
    files.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    files
}

pub(crate) fn install_adb_package_with_audit(
    app: AppHandle,
    request: AdbPackageInstallRequest,
    task_audit: Option<&str>,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let apk = fs::canonicalize(request.apk_path.trim())
        .map_err(|error| format!("无法读取 APK：{error}"))?;
    if !apk.is_file()
        || !apk
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"))
    {
        return Err("请选择有效的 APK 文件。".to_string());
    }
    check_apk_device_compatibility(serial, &apk)?;
    let adb = find_adb()?;
    let mut args = vec!["-s".to_string(), serial.to_string(), "install".to_string()];
    if request.replace {
        args.push("-r".to_string());
    }
    if request.allow_test {
        args.push("-t".to_string());
    }
    args.push(apk.to_string_lossy().to_string());
    let mut command = Command::new(&adb);
    command.args(&args);
    let output = command_output_with_timeout(command, Duration::from_secs(5 * 60))
        .map_err(|error| format!("安装 APK 失败：{error}"))?;
    if let Some(task_id) = task_audit {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        android_tasks::record_command_audit(
            &app,
            task_id,
            "安装 AOSP APK",
            &adb.to_string_lossy(),
            &refs,
            &output,
        )?;
    }
    let result = if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let raw = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(normalize_install_error(&raw))
    };
    let detail = result.as_deref().unwrap_or_else(|error| error.as_str());
    let _ = android_tasks::record_operation_audit(
        &app,
        "安装 APK",
        &format!("{serial}:{}", apk.display()),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

pub(crate) fn normalize_install_error(raw: &str) -> String {
    if raw.contains("INSTALL_FAILED_OLDER_SDK")
        && raw.contains("development platform")
        && raw.contains("release platform")
    {
        return format!("APK 需要开发版 Android 平台（设备当前是 release 镜像），无法安装。原始 ADB 错误：{raw}");
    }
    if raw.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        return format!(
            "设备上的应用版本高于当前 APK，需要允许降级安装或先卸载旧版本。原始 ADB 错误：{raw}"
        );
    }
    if raw.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        return format!("APK 签名或应用身份与设备上的现有版本不兼容。原始 ADB 错误：{raw}");
    }
    if raw.is_empty() {
        "ADB 安装失败，但设备没有返回错误详情。".to_string()
    } else {
        raw.to_string()
    }
}

pub(super) fn is_protected_remote_path(path: &str) -> bool {
    [
        "/system",
        "/vendor",
        "/product",
        "/odm",
        "/apex",
        "/system_ext",
    ]
    .iter()
    .any(|root| path == *root || path.starts_with(&format!("{root}/")))
}

pub(super) fn backup_root(app: &AppHandle, serial: &str) -> Result<PathBuf, String> {
    let serial = serial
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("backups")
        .join(serial);
    fs::create_dir_all(&root).map_err(|error| format!("无法创建备份目录：{error}"))?;
    Ok(root)
}

pub(super) fn backup_metadata_path(directory: &Path) -> PathBuf {
    directory.join("metadata.json")
}

pub(super) fn safe_backup_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("item")
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) {
                '_'
            } else {
                character
            }
        })
        .collect()
}

pub(super) fn backup_remote_path(
    app: &AppHandle,
    serial: &str,
    remote_path: &str,
    is_directory: bool,
) -> Result<AdbBackupEntry, String> {
    let created_at = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let id = format!("{created_at}_{}", safe_backup_name(remote_path));
    let directory = backup_root(app, serial)?.join(&id);
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建备份目录：{error}"))?;
    let payload = directory.join("payload");
    let payload_argument = payload.to_string_lossy().to_string();
    let output = adb_command_output(
        &["-s", serial, "pull", remote_path, &payload_argument],
        Duration::from_secs(10 * 60),
    )
    .map_err(|error| format!("无法备份设备文件：{error}"))?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&directory);
        return Err(format!(
            "备份失败，未执行删除：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if !payload.exists() {
        let _ = fs::remove_dir_all(&directory);
        return Err("备份失败，未执行删除。".to_string());
    }
    let entry = AdbBackupEntry {
        id,
        serial: serial.to_string(),
        remote_path: remote_path.to_string(),
        name: safe_backup_name(remote_path),
        is_directory,
        created_at,
        local_path: directory.to_string_lossy().to_string(),
    };
    fs::write(
        backup_metadata_path(&directory),
        serde_json::to_vec_pretty(&entry).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("无法写入备份元数据：{error}"))?;
    Ok(entry)
}

pub(crate) fn is_png_bytes(bytes: &[u8]) -> bool {
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.starts_with(&PNG_SIGNATURE)
}

pub(crate) fn bounded_performance_support_output(stdout: String, stderr: String) -> String {
    const MAX: usize = 4 * 1024;
    let mut output = if stderr.trim().is_empty() {
        stdout
    } else {
        format!("{stdout}\n{stderr}")
    };
    if output.len() > MAX {
        let mut end = MAX;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str("\n[能力探测输出已截断]");
    }
    output
}
