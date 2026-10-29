use super::*;

#[tauri::command]
pub fn begin_apk_edit_session(
    app: AppHandle,
    request: ArchiveEditSessionRequest,
) -> Result<ArchiveEditSession, String> {
    begin_apk_edit_session_inner(&app, &request, None)
}

pub(super) fn begin_apk_edit_session_inner(
    app: &AppHandle,
    request: &ArchiveEditSessionRequest,
    task: Option<(&str, &AtomicBool)>,
) -> Result<ArchiveEditSession, String> {
    let source = archive_path(&request.archive_path)?;
    if !source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"))
    {
        return Err("只有 APK 支持解码、编辑与重建。AAR 和 JAR 保持只读浏览。".to_string());
    }
    let apktool = find_apktool().ok_or_else(|| "未找到 Apktool。请将 apktool_*.jar 放到 %LOCALAPPDATA%\\Android Toolkit\\tools，或设置 ANDROID_TOOL_APKTOOL_JAR。".to_string())?;
    let java = find_java().ok_or_else(|| "未找到 Java，无法运行 Apktool。".to_string())?;
    let source_hash = sha256(&source)?;
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("无法创建工作区：{error}"))?
        .as_millis();
    let session_id = format!("{}-{}", &source_hash[..12], created);
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("archive-workspaces")
        .join(&session_id);
    let decoded = root.join("decoded");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建编辑工作区：{error}"))?;
    let args = [
        "-jar",
        apktool.to_string_lossy().as_ref(),
        "d",
        "--force",
        "--output",
        decoded.to_string_lossy().as_ref(),
        source.to_string_lossy().as_ref(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if let Some((task_id, cancellation)) = task {
        android_tasks::update_task(app, task_id, 12, "正在使用 Apktool 解码 APK...")?;
        if cancellation.load(Ordering::Relaxed) {
            let _ = fs::remove_dir_all(&root);
            return Err("APK 解码已取消。".to_string());
        }
    }
    let mut command = Command::new(&java);
    command.args(&args);
    let output = native_command_output(
        command,
        task.map(|(_, cancellation)| cancellation),
        Duration::from_secs(30 * 60),
        MAX_PROCESS_OUTPUT_BYTES,
    );
    let decoded_ok = match output {
        Ok(output) => {
            if let Some((task_id, _)) = task {
                let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                android_tasks::record_command_audit(
                    app,
                    task_id,
                    "APK 解码",
                    &java.to_string_lossy(),
                    &refs,
                    &output,
                )?;
            }
            output.status.success()
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&root);
            return Err(error);
        }
    };
    if !decoded_ok {
        let _ = fs::remove_dir_all(&root);
        return Err("APK 解码失败，请在任务审计中查看 Apktool 输出。".to_string());
    }
    if let Some((task_id, cancellation)) = task {
        if cancellation.load(Ordering::Relaxed) {
            let _ = fs::remove_dir_all(&root);
            return Err("APK 解码已取消。".to_string());
        }
        android_tasks::update_task(app, task_id, 88, "正在准备可编辑工作区...")?;
    }
    let manifest_path = decoded.join("AndroidManifest.xml");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("Apktool 未生成可编辑的 AndroidManifest.xml：{error}"))?;
    repair_apktool_resource_package(&decoded, &manifest)?;
    fs::write(root.join("original-manifest.xml"), manifest.as_bytes())
        .map_err(|error| format!("无法保存 Manifest 基线：{error}"))?;
    let version_args = ["-jar", apktool.to_string_lossy().as_ref(), "--version"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let apktool_version = require_success(&java, &version_args, "读取 Apktool 版本")
        .unwrap_or_else(|_| "未知版本".to_string());
    let timestamp = now_millis();
    let record = ArchiveHistoryRecord {
        id: session_id.clone(),
        archive_path: source.to_string_lossy().to_string(),
        file_name: source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("archive.apk")
            .to_string(),
        sha256: source_hash,
        opened_at: timestamp,
        updated_at: timestamp,
        workspace_path: Some(root.to_string_lossy().to_string()),
        apktool_version: Some(apktool_version.clone()),
        manifest_changed: false,
        output_path: None,
        verification: None,
        installed_serial: None,
        install_result: None,
    };
    upsert_archive_history(&app, &record)?;
    Ok(ArchiveEditSession {
        id: session_id,
        archive_path: source.to_string_lossy().to_string(),
        workspace_path: root.to_string_lossy().to_string(),
        manifest,
        apktool_version,
    })
}

#[tauri::command]
pub fn start_apk_edit_session(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: ArchiveEditSessionRequest,
) -> Result<String, String> {
    let source = archive_path(&request.archive_path)?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "apk-decode",
        &source.to_string_lossy(),
        "正在准备 APK 解码...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = begin_apk_edit_session_inner(
            &task_app,
            &request,
            Some((&task_id_for_thread, &cancellation)),
        );
        let artifact = result
            .as_ref()
            .ok()
            .map(|session| session.workspace_path.as_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

pub(super) fn archive_build_step(
    app: &AppHandle,
    task: Option<(&str, &AtomicBool, u8)>,
    label: &str,
    program: &Path,
    args: &[String],
) -> Result<String, String> {
    if let Some((task_id, cancellation, progress)) = task {
        android_tasks::update_task(app, task_id, progress, label)?;
        if cancellation.load(Ordering::Relaxed) {
            return Err("APK 构建已取消。".to_string());
        }
    }
    let mut command = environment_tool_command(program);
    command.args(args);
    let output = native_command_output(
        command,
        task.map(|(_, cancellation, _)| cancellation),
        Duration::from_secs(20 * 60),
        MAX_PROCESS_OUTPUT_BYTES,
    )?;
    if let Some((task_id, _, _)) = task {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        android_tasks::record_command_audit(
            app,
            task_id,
            label,
            &program.to_string_lossy(),
            &refs,
            &output,
        )?;
    }
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!(
        "{label}失败：{}",
        if detail.is_empty() {
            format!("退出码：{:?}", output.status.code())
        } else {
            detail
        }
    ))
}

pub(super) fn ensure_archive_build_not_cancelled(
    task: Option<(&str, &AtomicBool)>,
) -> Result<(), String> {
    if task.is_some_and(|(_, cancellation)| cancellation.load(Ordering::Relaxed)) {
        return Err("APK 构建已取消。".to_string());
    }
    Ok(())
}

pub(super) fn build_signed_apk_inner(
    app: &AppHandle,
    request: &ArchiveBuildRequest,
    task: Option<(&str, &AtomicBool)>,
) -> Result<ArchiveBuildResult, String> {
    let root = archive_session_root(&app, &request.session_id)?;
    let decoded = root.join("decoded");
    let manifest_path = decoded.join("AndroidManifest.xml");
    if !decoded.is_dir() || !manifest_path.is_file() {
        return Err("编辑工作区不完整，无法构建。".to_string());
    }
    if request.manifest.len() > MAX_REMOTE_TEXT_BYTES as usize {
        return Err("Manifest 超过 8 MB 编辑限制。".to_string());
    }
    if !request.manifest.trim_start().starts_with('<') {
        return Err("Manifest 必须是 XML 内容。".to_string());
    }
    let destination = PathBuf::from(request.destination.trim());
    if destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("apk"))
    {
        return Err("构建产物必须保存为 .apk 文件。".to_string());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建输出目录：{error}"))?;
    }
    fs::write(&manifest_path, request.manifest.as_bytes())
        .map_err(|error| format!("无法写入 Manifest：{error}"))?;
    let apktool = find_apktool().ok_or_else(|| "未找到 Apktool。".to_string())?;
    let java = find_java().ok_or_else(|| "未找到 Java，无法运行 Apktool。".to_string())?;
    let unsigned = root.join("rebuilt-unsigned.apk");
    let aligned = root.join("rebuilt-aligned.apk");
    let signed = root.join("rebuilt-signed.apk");
    for path in [&unsigned, &aligned, &signed] {
        let _ = fs::remove_file(path);
    }
    let result = (|| {
        let build_args = [
            "-jar",
            apktool.to_string_lossy().as_ref(),
            "b",
            decoded.to_string_lossy().as_ref(),
            "--output",
            unsigned.to_string_lossy().as_ref(),
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        archive_build_step(
            app,
            task.map(|(id, cancellation)| (id, cancellation, 20)),
            "APK 重建",
            &java,
            &build_args,
        )?;
        ensure_archive_build_not_cancelled(task)?;
        let zipalign = find_android_build_tool(if cfg!(target_os = "windows") {
            "zipalign.exe"
        } else {
            "zipalign"
        })
        .ok_or_else(|| {
            "未找到 Android SDK zipalign。请安装 Android SDK Build-Tools。".to_string()
        })?;
        let align_args = [
            "-f",
            "4",
            unsigned.to_string_lossy().as_ref(),
            aligned.to_string_lossy().as_ref(),
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        archive_build_step(
            app,
            task.map(|(id, cancellation)| (id, cancellation, 45)),
            "APK 对齐",
            &zipalign,
            &align_args,
        )?;
        ensure_archive_build_not_cancelled(task)?;
        let apksigner = find_android_build_tool(if cfg!(target_os = "windows") {
            "apksigner.bat"
        } else {
            "apksigner"
        })
        .ok_or_else(|| {
            "未找到 Android SDK apksigner。请安装 Android SDK Build-Tools。".to_string()
        })?;
        let keystore = debug_keystore(&app)?;
        let sign_args = [
            "sign",
            "--ks",
            keystore.to_string_lossy().as_ref(),
            "--ks-key-alias",
            "androiddebugkey",
            "--ks-pass",
            "pass:android",
            "--key-pass",
            "pass:android",
            "--out",
            signed.to_string_lossy().as_ref(),
            aligned.to_string_lossy().as_ref(),
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        archive_build_step(
            app,
            task.map(|(id, cancellation)| (id, cancellation, 65)),
            "APK 签名",
            &apksigner,
            &sign_args,
        )?;
        ensure_archive_build_not_cancelled(task)?;
        let verify_args = [
            "verify",
            "--verbose",
            "--print-certs",
            signed.to_string_lossy().as_ref(),
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let verification = archive_build_step(
            app,
            task.map(|(id, cancellation)| (id, cancellation, 85)),
            "APK 验签",
            &apksigner,
            &verify_args,
        )?;
        ensure_archive_build_not_cancelled(task)?;
        fs::copy(&signed, &destination).map_err(|error| format!("无法保存签名 APK：{error}"))?;
        let original_manifest =
            fs::read_to_string(root.join("original-manifest.xml")).unwrap_or_default();
        archive_history_store(&app)?.execute("UPDATE archive_history SET updated_at=?2, manifest_changed=?3, output_path=?4, verification=?5 WHERE id=?1", params![request.session_id, now_millis(), request.manifest != original_manifest, destination.to_string_lossy().to_string(), verification]).map_err(|error| format!("无法更新 APK 构建历史：{error}"))?;
        Ok(ArchiveBuildResult {
            output_path: destination.to_string_lossy().to_string(),
            verification,
        })
    })();
    if result.is_err() {
        for path in [&unsigned, &aligned, &signed] {
            let _ = fs::remove_file(path);
        }
    }
    result
}

#[tauri::command]
pub fn build_signed_apk(
    app: AppHandle,
    request: ArchiveBuildRequest,
) -> Result<ArchiveBuildResult, String> {
    let target = format!("{}:{}", request.session_id, request.destination);
    let result = build_signed_apk_inner(&app, &request, None);
    let detail = match &result {
        Ok(build) => format!("APK 已重建、zipalign、签名并验签：{}", build.output_path),
        Err(error) => format!("APK 重建未完成：{error}"),
    };
    let _ = android_tasks::record_operation_audit(
        &app,
        "重建并签名 APK",
        &target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

#[tauri::command]
pub fn start_signed_apk_build(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: ArchiveBuildRequest,
) -> Result<String, String> {
    let manager = state.inner().clone();
    let target = format!("{}:{}", request.session_id, request.destination);
    let (task_id, cancellation) =
        android_tasks::create_task(&app, &manager, "apk-build", &target, "正在准备 APK 重建...")?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = build_signed_apk_inner(
            &task_app,
            &request,
            Some((&task_id_for_thread, &cancellation)),
        );
        let artifact = result.as_ref().ok().map(|build| build.output_path.as_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_archive_build_result(
    app: AppHandle,
    session_id: String,
) -> Result<ArchiveBuildResult, String> {
    archive_session_root(&app, &session_id)?;
    archive_history_store(&app)?.query_row(
        "SELECT output_path,verification FROM archive_history WHERE id=?1 AND output_path IS NOT NULL AND verification IS NOT NULL",
        params![session_id],
        |row| Ok(ArchiveBuildResult { output_path: row.get(0)?, verification: row.get(1)? }),
    ).map_err(|_| "当前工作区还没有已验签的构建产物。".to_string())
}

#[tauri::command]
pub fn install_archive_build(
    app: AppHandle,
    request: ArchiveBuildInstallRequest,
) -> Result<String, String> {
    // The session lookup prevents an arbitrary history id from being used to
    // attach installation records to a different managed workspace.
    archive_session_root(&app, &request.session_id)?;
    let output_path: Option<String> = archive_history_store(&app)?
        .query_row(
            "SELECT output_path FROM archive_history WHERE id=?1",
            params![request.session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 APK 构建历史：{error}"))?;
    let apk_path = output_path.ok_or_else(|| "当前工作区还没有可安装的构建产物。".to_string())?;
    let result = install_adb_package(
        app.clone(),
        AdbPackageInstallRequest {
            serial: request.serial.clone(),
            apk_path,
            replace: request.replace,
            allow_test: false,
        },
    );
    let audit_result = match &result {
        Ok(output) if !output.is_empty() => output.clone(),
        Ok(_) => "安装成功".to_string(),
        Err(error) => format!("安装失败：{error}"),
    };
    archive_history_store(&app)?.execute(
        "UPDATE archive_history SET updated_at=?2, installed_serial=?3, install_result=?4 WHERE id=?1",
        params![request.session_id, now_millis(), request.serial, audit_result],
    ).map_err(|error| format!("无法更新 APK 安装历史：{error}"))?;
    let _ = android_tasks::record_operation_audit(
        &app,
        "安装已构建 APK",
        &format!("{}:{}", request.serial, request.session_id),
        if result.is_ok() { "done" } else { "error" },
        &audit_result,
    );
    result
}

/// Runs installation of a signed archive build under the common Android task
/// contract. Cancellation remains available during validation; after ADB starts
/// its device transaction the task must report the actual device result.
#[tauri::command]
pub fn start_archive_build_install(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: ArchiveBuildInstallRequest,
) -> Result<String, String> {
    archive_session_root(&app, &request.session_id)?;
    let serial = checked_serial(&request.serial)?.to_string();
    let apk_path: String = archive_history_store(&app)?
        .query_row(
            "SELECT output_path FROM archive_history WHERE id=?1 AND output_path IS NOT NULL",
            params![request.session_id],
            |row| row.get(0),
        )
        .map_err(|_| "当前工作区还没有可安装的构建产物。".to_string())?;
    let manager = state.inner().clone();
    let target = format!("{}:{}", serial, request.session_id);
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "apk-install",
        &target,
        "正在检查 APK 与设备兼容性...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let artifact_path = apk_path.clone();
        let result = (|| -> Result<String, String> {
            if cancellation.load(Ordering::Relaxed) {
                return Err("任务在安装前已取消。".to_string());
            }
            let apk =
                fs::canonicalize(&apk_path).map_err(|error| format!("无法读取 APK：{error}"))?;
            if !apk.is_file()
                || !apk
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"))
            {
                return Err("当前构建产物不是有效的 APK 文件。".to_string());
            }
            check_apk_device_compatibility(&serial, &apk)?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                55,
                "正在通过 Windows ADB 安装 APK...",
            )?;
            android_tasks::enter_non_cancellable_phase(
                &task_app,
                &manager,
                &task_id_for_thread,
                "已进入 Windows ADB 设备安装阶段；后续将记录实际设备结果。",
            )?;
            let result = install_adb_package_with_audit(
                task_app.clone(),
                AdbPackageInstallRequest {
                    serial: serial.clone(),
                    apk_path: apk.to_string_lossy().to_string(),
                    replace: request.replace,
                    allow_test: false,
                },
                Some(&task_id_for_thread),
            );
            let audit_result = match &result {
                Ok(output) if !output.is_empty() => output.clone(),
                Ok(_) => "安装成功".to_string(),
                Err(error) => format!("安装失败：{error}"),
            };
            archive_history_store(&task_app)?.execute(
                "UPDATE archive_history SET updated_at=?2, installed_serial=?3, install_result=?4 WHERE id=?1",
                params![request.session_id, now_millis(), serial, audit_result],
            ).map_err(|error| format!("无法更新 APK 安装历史：{error}"))?;
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "install-result",
                &audit_result,
            )?;
            result
        })();
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &task_id_for_thread,
            Some(artifact_path.as_str()),
            error,
        );
    });
    Ok(task_id)
}

#[tauri::command]
pub fn list_archive_history(app: AppHandle) -> Result<Vec<ArchiveHistoryRecord>, String> {
    let connection = archive_history_store(&app)?;
    let mut statement = connection.prepare("SELECT id,archive_path,file_name,sha256,opened_at,updated_at,workspace_path,apktool_version,manifest_changed,output_path,verification,installed_serial,install_result FROM archive_history ORDER BY updated_at DESC").map_err(|error| format!("无法读取 APK 历史：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(ArchiveHistoryRecord {
                id: row.get(0)?,
                archive_path: row.get(1)?,
                file_name: row.get(2)?,
                sha256: row.get(3)?,
                opened_at: row.get(4)?,
                updated_at: row.get(5)?,
                workspace_path: row.get(6)?,
                apktool_version: row.get(7)?,
                manifest_changed: row.get::<_, i32>(8)? != 0,
                output_path: row.get(9)?,
                verification: row.get(10)?,
                installed_serial: row.get(11)?,
                install_result: row.get(12)?,
            })
        })
        .map_err(|error| format!("无法读取 APK 历史：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("APK 历史格式无效：{error}"))
}
