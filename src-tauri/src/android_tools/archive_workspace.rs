use super::*;

#[tauri::command]
pub fn delete_archive_history(
    app: AppHandle,
    request: ArchiveHistoryDeleteRequest,
) -> Result<(), String> {
    let connection = archive_history_store(&app)?;
    let workspace: Option<String> = connection
        .query_row(
            "SELECT workspace_path FROM archive_history WHERE id=?1",
            params![request.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 APK 历史：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_history WHERE id=?1",
            params![request.id],
        )
        .map_err(|error| format!("无法删除 APK 历史：{error}"))?;
    if request.delete_workspace {
        if let Some(path) = workspace {
            let root = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法获取应用数据目录：{error}"))?
                .join("archive-workspaces");
            let candidate = PathBuf::from(path);
            if candidate.starts_with(&root) && candidate.is_dir() {
                fs::remove_dir_all(candidate)
                    .map_err(|error| format!("无法清理编辑工作区：{error}"))?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn resume_apk_edit_session(
    app: AppHandle,
    request: ArchiveHistoryResumeRequest,
) -> Result<ArchiveEditSession, String> {
    let connection = archive_history_store(&app)?;
    let record: Option<(String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT archive_path,workspace_path,apktool_version FROM archive_history WHERE id=?1",
            params![request.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| format!("无法读取 APK 历史：{error}"))?;
    let (archive_path, workspace_path, apktool_version) =
        record.ok_or_else(|| "APK 历史不存在。".to_string())?;
    let workspace = workspace_path
        .map(PathBuf::from)
        .ok_or_else(|| "该记录没有可恢复的编辑工作区。".to_string())?;
    let managed_root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("archive-workspaces");
    if !workspace.starts_with(&managed_root) || !workspace.is_dir() {
        return Err("编辑工作区已不存在或不属于当前应用。".to_string());
    }
    let manifest = fs::read_to_string(workspace.join("decoded").join("AndroidManifest.xml"))
        .map_err(|error| format!("无法读取已解码 Manifest：{error}"))?;
    Ok(ArchiveEditSession {
        id: request.id,
        archive_path,
        workspace_path: workspace.to_string_lossy().to_string(),
        manifest,
        apktool_version: apktool_version.unwrap_or_else(|| "未知版本".to_string()),
    })
}

#[tauri::command]
pub fn read_archive_original_manifest(
    app: AppHandle,
    session_id: String,
) -> Result<String, String> {
    let root = archive_session_root(&app, &session_id)?;
    fs::read_to_string(root.join("original-manifest.xml"))
        .map_err(|error| format!("无法读取 Manifest 基线：{error}"))
}

#[tauri::command]
pub fn decompile_apk_with_jadx(
    app: AppHandle,
    request: JadxDecompileRequest,
) -> Result<JadxDecompileResult, String> {
    decompile_apk_with_jadx_inner(&app, &request, None)
}

pub(super) fn decompile_apk_with_jadx_inner(
    app: &AppHandle,
    request: &JadxDecompileRequest,
    task: Option<(&str, &AtomicBool)>,
) -> Result<JadxDecompileResult, String> {
    let source = archive_path(&request.archive_path)?;
    let jadx = find_jadx().ok_or_else(|| "未找到 JADX。请将其放到 %LOCALAPPDATA%\\Android Toolkit\\tools，或设置 ANDROID_TOOL_JADX。".to_string())?;
    let hash = sha256(&source)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("jadx-workspaces")
        .join(format!("{}-{}", &hash[..12], now_millis()));
    let source_root = root.join("sources");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建 JADX 工作区：{error}"))?;
    let args = [
        "--output-dir",
        root.to_string_lossy().as_ref(),
        source.to_string_lossy().as_ref(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if let Some((task_id, cancellation)) = task {
        android_tasks::update_task(app, task_id, 10, "正在启动 JADX 反编译...")?;
        if cancellation.load(Ordering::Relaxed) {
            return Err("JADX 反编译已取消。".to_string());
        }
    }
    let mut command = environment_tool_command(&jadx);
    command.args(&args);
    let output = native_command_output(
        command,
        task.map(|(_, cancellation)| cancellation),
        Duration::from_secs(30 * 60),
        MAX_PROCESS_OUTPUT_BYTES,
    )?;
    if let Some((task_id, _)) = task {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        android_tasks::record_command_audit(
            app,
            task_id,
            "JADX 反编译",
            &jadx.to_string_lossy(),
            &refs,
            &output,
        )?;
    }
    if !output.status.success() {
        let _ = fs::remove_dir_all(&root);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(format!(
            "JADX 反编译失败：{}",
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }
    if let Some((task_id, cancellation)) = task {
        if cancellation.load(Ordering::Relaxed) {
            let _ = fs::remove_dir_all(&root);
            return Err("JADX 反编译已取消。".to_string());
        }
        android_tasks::update_task(app, task_id, 90, "正在整理 JADX 源码索引...")?;
    }
    let mut files = Vec::new();
    if source_root.is_dir() {
        collect_jadx_sources(&source_root, &source_root, &mut files)?;
    }
    files.sort();
    let version = require_success(&jadx, &["--version".to_string()], "读取 JADX 版本")
        .unwrap_or_else(|_| "未知版本".to_string());
    Ok(JadxDecompileResult {
        workspace_path: root.to_string_lossy().to_string(),
        version,
        files,
    })
}

#[tauri::command]
pub fn start_jadx_decompile(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: JadxDecompileRequest,
) -> Result<String, String> {
    let source = archive_path(&request.archive_path)?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "jadx-decompile",
        &source.to_string_lossy(),
        "正在准备 JADX 反编译...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = decompile_apk_with_jadx_inner(
            &task_app,
            &request,
            Some((&task_id_for_thread, &cancellation)),
        );
        let artifact = result
            .as_ref()
            .ok()
            .map(|result| result.workspace_path.as_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn inspect_jadx_workspace(workspace_path: String) -> Result<JadxDecompileResult, String> {
    let root = fs::canonicalize(workspace_path.trim())
        .map_err(|error| format!("无法读取 JADX 工作区：{error}"))?;
    let source_root = root.join("sources");
    if !source_root.is_dir() {
        return Err("JADX 工作区不完整。".to_string());
    }
    let mut files = Vec::new();
    collect_jadx_sources(&source_root, &source_root, &mut files)?;
    files.sort();
    Ok(JadxDecompileResult {
        workspace_path: root.to_string_lossy().to_string(),
        version: "已完成".to_string(),
        files,
    })
}

#[tauri::command]
pub fn read_jadx_source(request: JadxSourceRequest) -> Result<String, String> {
    let root = fs::canonicalize(&request.workspace_path)
        .map_err(|error| format!("无法读取 JADX 工作区：{error}"))?;
    let source_root = root.join("sources");
    let source_root =
        fs::canonicalize(&source_root).map_err(|error| format!("JADX 源码目录不存在：{error}"))?;
    let relative = Path::new(&request.relative_path);
    if relative.is_absolute() {
        return Err("源码路径必须是相对路径。".to_string());
    }
    let candidate = fs::canonicalize(source_root.join(relative))
        .map_err(|error| format!("无法读取源码：{error}"))?;
    if !candidate.starts_with(&source_root) {
        return Err("源码路径越界。".to_string());
    }
    let metadata =
        fs::metadata(&candidate).map_err(|error| format!("无法读取源码信息：{error}"))?;
    if metadata.len() > MAX_REMOTE_TEXT_BYTES {
        return Err("源码文件超过 8 MB 阅读限制。".to_string());
    }
    fs::read_to_string(candidate).map_err(|error| format!("无法读取源码：{error}"))
}

#[tauri::command]
pub fn list_apk_workspace_files(
    app: AppHandle,
    session_id: String,
) -> Result<Vec<WorkspaceFile>, String> {
    let root = archive_session_root(&app, &session_id)?;
    let decoded = root.join("decoded");
    let mut files = Vec::new();
    collect_editable_workspace_files(&decoded, &decoded, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

#[tauri::command]
pub fn read_apk_workspace_file(
    app: AppHandle,
    request: WorkspaceFileRequest,
) -> Result<WorkspaceFileDiff, String> {
    let (root, file) = workspace_file_path(&app, &request.session_id, &request.relative_path)?;
    let metadata = fs::metadata(&file).map_err(|error| format!("无法读取文件信息：{error}"))?;
    if metadata.len() > MAX_REMOTE_TEXT_BYTES {
        return Err("编辑文件超过 8 MB 限制。".to_string());
    }
    let baseline = root.join("baseline").join(&request.relative_path);
    if !baseline.is_file() {
        if let Some(parent) = baseline.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建文件基线：{error}"))?;
        }
        fs::copy(&file, &baseline).map_err(|error| format!("无法保存文件基线：{error}"))?;
    }
    Ok(WorkspaceFileDiff {
        original: fs::read_to_string(&baseline)
            .map_err(|error| format!("无法读取文件基线：{error}"))?,
        current: fs::read_to_string(file).map_err(|error| format!("无法读取编辑文件：{error}"))?,
    })
}

#[tauri::command]
pub fn write_apk_workspace_file(
    app: AppHandle,
    request: WorkspaceWriteRequest,
) -> Result<(), String> {
    if request.content.as_bytes().len() as u64 > MAX_REMOTE_TEXT_BYTES {
        return Err("编辑文件超过 8 MB 限制。".to_string());
    }
    let (root, file) = workspace_file_path(&app, &request.session_id, &request.relative_path)?;
    let baseline = root.join("baseline").join(&request.relative_path);
    if !baseline.is_file() {
        if let Some(parent) = baseline.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建文件基线：{error}"))?;
        }
        fs::copy(&file, &baseline).map_err(|error| format!("无法保存文件基线：{error}"))?;
    }
    fs::write(file, request.content).map_err(|error| format!("无法写入编辑文件：{error}"))
}

#[tauri::command]
pub fn summarize_apk_workspace_diff(
    app: AppHandle,
    session_id: String,
) -> Result<WorkspaceChangeSummary, String> {
    const MAX_SUMMARY_FILES: usize = 1_000;
    let root = archive_session_root(&app, &session_id)?;
    let decoded = root.join("decoded");
    let baseline_root = root.join("baseline");
    let mut files = Vec::new();
    collect_editable_workspace_files(&decoded, &decoded, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut entries = Vec::new();
    let mut added_lines = 0;
    let mut removed_lines = 0;
    let mut truncated = false;
    let mut changed_files = 0;
    for file in files {
        let current_path = decoded.join(&file.path);
        let baseline_path = if file.path == "AndroidManifest.xml" {
            root.join("original-manifest.xml")
        } else {
            baseline_root.join(&file.path)
        };
        if !baseline_path.is_file() {
            continue;
        }
        let current_size = fs::metadata(&current_path)
            .map_err(|error| format!("无法读取编辑文件信息：{error}"))?
            .len();
        let baseline_size = fs::metadata(&baseline_path)
            .map_err(|error| format!("无法读取文件基线信息：{error}"))?
            .len();
        if current_size > MAX_REMOTE_TEXT_BYTES || baseline_size > MAX_REMOTE_TEXT_BYTES {
            continue;
        }
        let original = fs::read_to_string(&baseline_path)
            .map_err(|error| format!("无法读取文件基线：{error}"))?;
        let current = fs::read_to_string(&current_path)
            .map_err(|error| format!("无法读取编辑文件：{error}"))?;
        if original == current {
            continue;
        }
        changed_files += 1;
        let (added, removed) = changed_line_counts(&original, &current);
        added_lines += added;
        removed_lines += removed;
        if entries.len() < MAX_SUMMARY_FILES {
            entries.push(WorkspaceChangeEntry {
                path: file.path.clone(),
                category: workspace_change_category(&file.path).to_string(),
                added_lines: added,
                removed_lines: removed,
                detail: workspace_change_detail(&file.path, &original, &current),
                changes: workspace_structured_changes(&file.path, &original, &current),
            });
        } else {
            truncated = true;
        }
    }
    Ok(WorkspaceChangeSummary {
        changed_files,
        added_lines,
        removed_lines,
        entries,
        truncated,
    })
}

#[tauri::command]
pub fn build_apks_from_aab(
    app: AppHandle,
    request: BundletoolBuildRequest,
) -> Result<String, String> {
    build_apks_from_aab_inner(&app, &request, None)
}

pub(super) fn build_apks_from_aab_inner(
    app: &AppHandle,
    request: &BundletoolBuildRequest,
    task: Option<(&str, &AtomicBool)>,
) -> Result<String, String> {
    let bundle = archive_path(&request.aab_path)?;
    if !bundle
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("aab"))
    {
        return Err("请选择 .aab 文件。".to_string());
    }
    let destination = PathBuf::from(request.destination.trim());
    if !destination
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("apks"))
    {
        return Err("APK Set 必须保存为 .apks 文件。".to_string());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建输出目录：{error}"))?;
    }
    let java = find_java().ok_or_else(|| "未找到 Java，无法运行 Bundletool。".to_string())?;
    let bundletool = find_bundletool().ok_or_else(|| "未找到 Bundletool。".to_string())?;
    let keystore = debug_keystore(&app)?;
    let args = [
        "-jar",
        bundletool.to_string_lossy().as_ref(),
        "build-apks",
        "--bundle",
        bundle.to_string_lossy().as_ref(),
        "--output",
        destination.to_string_lossy().as_ref(),
        "--mode",
        "universal",
        "--ks",
        keystore.to_string_lossy().as_ref(),
        "--ks-key-alias",
        "androiddebugkey",
        "--ks-pass",
        "pass:android",
        "--key-pass",
        "pass:android",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if let Some((task_id, cancellation)) = task {
        android_tasks::update_task(app, task_id, 15, "正在使用 Bundletool 构建 APK Set...")?;
        if cancellation.load(Ordering::Relaxed) {
            return Err("AAB 构建已取消。".to_string());
        }
    }
    let existed_before = destination.exists();
    let mut command = Command::new(&java);
    command.args(&args);
    let output = native_command_output(
        command,
        task.map(|(_, cancellation)| cancellation),
        Duration::from_secs(20 * 60),
        MAX_PROCESS_OUTPUT_BYTES,
    );
    let result = match output {
        Ok(output) => {
            if let Some((task_id, _)) = task {
                let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                android_tasks::record_command_audit(
                    app,
                    task_id,
                    "AAB 构建 APK Set",
                    &java.to_string_lossy(),
                    &refs,
                    &output,
                )?;
            }
            if output.status.success() {
                Ok(destination.to_string_lossy().to_string())
            } else {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(format!(
                    "AAB 构建 APK Set失败：{}",
                    if detail.is_empty() {
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    } else {
                        detail
                    }
                ))
            }
        }
        Err(error) => Err(error),
    };
    if result.is_err() && !existed_before {
        let _ = fs::remove_file(&destination);
    }
    result
}

#[tauri::command]
pub fn start_apks_from_aab_build(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: BundletoolBuildRequest,
) -> Result<String, String> {
    let manager = state.inner().clone();
    let target = format!("{}:{}", request.aab_path, request.destination);
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "aab-build",
        &target,
        "正在准备 AAB APK Set 构建...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = build_apks_from_aab_inner(
            &task_app,
            &request,
            Some((&task_id_for_thread, &cancellation)),
        );
        let artifact = result.as_ref().ok().map(String::as_str);
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn install_apks_with_bundletool(
    app: AppHandle,
    request: BundletoolInstallRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let apks = fs::canonicalize(request.apks_path.trim())
        .map_err(|error| format!("无法读取 APK Set：{error}"))?;
    if !apks
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("apks"))
    {
        return Err("请选择 .apks 文件。".to_string());
    }
    let java = find_java().ok_or_else(|| "未找到 Java，无法运行 Bundletool。".to_string())?;
    let bundletool = find_bundletool().ok_or_else(|| "未找到 Bundletool。".to_string())?;
    let args = [
        "-jar",
        bundletool.to_string_lossy().as_ref(),
        "install-apks",
        "--apks",
        apks.to_string_lossy().as_ref(),
        "--device-id",
        serial,
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let result = require_success(&java, &args, "安装 APK Set");
    let detail = result.as_deref().unwrap_or_else(|error| error);
    let _ = android_tasks::record_operation_audit(
        &app,
        "安装 AAB APK Set",
        &format!("{serial}:{}", apks.display()),
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn start_apks_with_bundletool_install(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: BundletoolInstallRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let apks = fs::canonicalize(request.apks_path.trim())
        .map_err(|error| format!("无法读取 APK Set：{error}"))?;
    if !apks.is_file()
        || !apks
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("apks"))
    {
        return Err("请选择 .apks 文件。".to_string());
    }
    let java = find_java().ok_or_else(|| "未找到 Java，无法运行 Bundletool。".to_string())?;
    let bundletool = find_bundletool().ok_or_else(|| "未找到 Bundletool。".to_string())?;
    let manager = state.inner().clone();
    let target = format!("{}:{}", serial, apks.display());
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "apks-install",
        &target,
        "正在准备 APK Set 安装...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let artifact = apks.to_string_lossy().to_string();
        let result = (|| -> Result<String, String> {
            if cancellation.load(Ordering::Relaxed) {
                return Err("任务在安装前已取消。".to_string());
            }
            let args = [
                "-jar",
                bundletool.to_string_lossy().as_ref(),
                "install-apks",
                "--apks",
                apks.to_string_lossy().as_ref(),
                "--device-id",
                serial.as_str(),
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                40,
                "正在通过 Bundletool 安装 APK Set...",
            )?;
            android_tasks::enter_non_cancellable_phase(
                &task_app,
                &manager,
                &task_id_for_thread,
                "已进入 Bundletool 设备安装阶段；后续将记录实际安装结果。",
            )?;
            let result = require_success(&java, &args, "安装 APK Set");
            let detail = result.as_deref().unwrap_or_else(|error| error);
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "install-result",
                detail,
            )?;
            result
        })();
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &task_id_for_thread,
            Some(artifact.as_str()),
            error,
        );
    });
    Ok(task_id)
}
