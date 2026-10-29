use super::*;

#[tauri::command]
pub fn start_winscope_capture(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    serial: String,
) -> Result<String, String> {
    let serial = checked_serial(&serial)?.to_string();
    if tools
        .winscope
        .lock()
        .map_err(|_| "WinScope 采集状态不可用。".to_string())?
        .contains_key(&serial)
    {
        return Err("该设备的 WinScope 采集已在进行。".to_string());
    }
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "winscope",
        &serial,
        "正在启动 WindowManager Trace 采集...",
    )?;
    let adb = match find_adb() {
        Ok(adb) => adb,
        Err(error) => {
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&error));
            return Err(error);
        }
    };
    let start_args = [
        "-s",
        serial.as_str(),
        "shell",
        "cmd",
        "window",
        "tracing",
        "start",
    ];
    let started = match adb_command_output(&start_args, Duration::from_secs(30)) {
        Ok(output) => output,
        Err(error) => {
            let _ = android_tasks::record_task_audit(
                &app,
                &task_id,
                "command-start-failed",
                &format!("启动 WindowManager Trace 前无法执行 adb：{error}"),
            );
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&error));
            return Err(error);
        }
    };
    android_tasks::record_command_audit(
        &app,
        &task_id,
        "启动 WindowManager Trace",
        &adb.to_string_lossy(),
        &start_args,
        &started,
    )?;
    let start_error = winscope_trace_command_error(
        started.status.success(),
        &String::from_utf8_lossy(&started.stdout),
        &String::from_utf8_lossy(&started.stderr),
    );
    if let Some(error) = start_error {
        let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&error));
        return Err(error);
    }
    tools
        .winscope
        .lock()
        .map_err(|_| "WinScope 采集状态不可用。".to_string())?
        .insert(serial.clone(), task_id.clone());
    android_tasks::update_task(
        &app,
        &task_id,
        15,
        "正在采集 WindowManager Trace；停止录制后将自动导出产物。",
    )?;

    let sessions = tools.winscope.clone();
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let active = sessions
            .lock()
            .ok()
            .and_then(|items| items.get(&serial).cloned())
            .is_some_and(|id| id == task_id_for_thread);
        if !active {
            break;
        }
        if cancellation.load(Ordering::Relaxed) {
            if let Ok(mut items) = sessions.lock() {
                items.remove(&serial);
            }
            let stop_args = [
                "-s",
                serial.as_str(),
                "shell",
                "cmd",
                "window",
                "tracing",
                "stop",
            ];
            let detail = match adb_command_output(&stop_args, Duration::from_secs(30)) {
                Ok(output) => {
                    let _ = android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        "取消 WinScope 采集",
                        "adb",
                        &stop_args,
                        &output,
                    );
                    winscope_trace_command_error(
                        output.status.success(),
                        &String::from_utf8_lossy(&output.stdout),
                        &String::from_utf8_lossy(&output.stderr),
                    )
                    .map(|error| format!("用户取消 WinScope 采集时停止命令失败：{error}"))
                    .unwrap_or_else(|| "用户取消 WinScope 采集，已发送停止命令。".to_string())
                }
                Err(error) => format!("用户取消 WinScope 采集时停止命令失败：{error}"),
            };
            let _ = android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "cancelled",
                &detail,
            );
            let _ =
                android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, None, None);
            break;
        }
    });
    Ok(task_id)
}

#[tauri::command]
pub fn stop_winscope_capture(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    request: WinscopeCaptureStopRequest,
) -> Result<WinscopeCaptureResult, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "WinScope 保存路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 WinScope 目录：{error}"))?;
    let task_id = tools
        .winscope
        .lock()
        .map_err(|_| "WinScope 采集状态不可用。".to_string())?
        .remove(&serial);
    let manager = state.inner().clone();
    let result = (|| -> Result<WinscopeCaptureResult, String> {
        if let Some(id) = task_id.as_deref() {
            android_tasks::update_task(&app, id, 55, "正在停止录制并拉取 WindowManager Trace...")?;
        }
        let adb = find_adb()?;
        let stop_args = [
            "-s",
            serial.as_str(),
            "shell",
            "cmd",
            "window",
            "tracing",
            "stop",
        ];
        let stopped = adb_command_output(&stop_args, Duration::from_secs(30))
            .map_err(|error| format!("无法停止 WindowManager Trace：{error}"))?;
        if let Some(id) = task_id.as_deref() {
            android_tasks::record_command_audit(
                &app,
                id,
                "停止 WindowManager Trace",
                &adb.to_string_lossy(),
                &stop_args,
                &stopped,
            )?;
        }
        if let Some(error) = winscope_trace_command_error(
            stopped.status.success(),
            &String::from_utf8_lossy(&stopped.stdout),
            &String::from_utf8_lossy(&stopped.stderr),
        ) {
            return Err(format!("无法停止 WindowManager Trace：{error}"));
        }
        let remote = "/data/misc/wmtrace/wm_trace.winscope";
        let destination_argument = destination.to_string_lossy().to_string();
        let pull_args = [
            "-s",
            serial.as_str(),
            "pull",
            remote,
            destination_argument.as_str(),
        ];
        let pulled = adb_command_output(&pull_args, Duration::from_secs(5 * 60))
            .map_err(|error| format!("无法拉取 WinScope Trace：{error}"))?;
        if let Some(id) = task_id.as_deref() {
            android_tasks::record_command_audit(
                &app,
                id,
                "pull-winscope-trace",
                &adb.to_string_lossy(),
                &pull_args,
                &pulled,
            )?;
        }
        if !pulled.status.success() {
            let detail = String::from_utf8_lossy(&pulled.stderr).trim().to_string();
            if detail.to_ascii_lowercase().contains("permission denied") {
                return Err("无法读取 WinScope Trace：设备拒绝访问 /data/misc/wmtrace。请在开发设备中先请求 Root adbd；量产设备通常不支持此操作。".to_string());
            }
            return Err(format!("无法拉取 WinScope Trace：{detail}"));
        }
        let metadata = fs::metadata(&destination)
            .map_err(|error| format!("无法读取 WinScope Trace：{error}"))?;
        if metadata.len() == 0 {
            return Err("WinScope Trace 文件为空。".to_string());
        }
        if let Some(id) = task_id.as_deref() {
            android_tasks::update_task(&app, id, 95, "Trace 已保存，正在完成任务...")?;
        }
        Ok(WinscopeCaptureResult {
            name: destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("wm_trace.winscope")
                .to_string(),
            path: destination.to_string_lossy().to_string(),
            size: metadata.len(),
        })
    })();
    if let Some(id) = task_id.as_deref() {
        let artifact = result.as_ref().ok().map(|value| value.path.as_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(&app, &manager, id, artifact, error);
    }
    result
}

#[tauri::command]
pub fn install_split_apks(
    app: AppHandle,
    request: SplitApkInstallRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    if request.apk_paths.len() < 2 {
        return Err("Split APK 安装至少需要两个 APK 文件。".to_string());
    }
    if request.apk_paths.len() > 100 {
        return Err("一次最多安装 100 个 APK 文件。".to_string());
    }
    let paths = request
        .apk_paths
        .iter()
        .map(|path| {
            fs::canonicalize(path.trim()).map_err(|error| format!("无法读取 APK 文件：{error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().any(|path| {
        !path.is_file()
            || !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("apk"))
    }) {
        return Err("Split APK 列表中包含无效 APK 文件。".to_string());
    }
    let target = format!("{}:{} 个 APK", serial, paths.len());
    let result = (|| -> Result<String, String> {
        let adb = find_adb()?;
        let mut command = Command::new(adb);
        command.args(["-s", serial, "install-multiple"]);
        if request.replace {
            command.arg("-r");
        }
        command.args(&paths);
        let output = command_output_with_timeout(command, Duration::from_secs(5 * 60))
            .map_err(|error| format!("无法安装 Split APK：{error}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .trim()
        .to_string();
        if !output.status.success() {
            return Err(if text.is_empty() {
                "Split APK 安装失败。".to_string()
            } else {
                text
            });
        }
        Ok(text)
    })();
    let detail = result.as_deref().unwrap_or_else(|error| error);
    let _ = android_tasks::record_operation_audit(
        &app,
        "安装 Split APK",
        &target,
        if result.is_ok() { "done" } else { "error" },
        detail,
    );
    result
}

#[tauri::command]
pub fn start_split_apks_install(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: SplitApkInstallRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    if request.apk_paths.len() < 2 {
        return Err("Split APK 安装至少需要两个 APK 文件。".to_string());
    }
    if request.apk_paths.len() > 100 {
        return Err("一次最多安装 100 个 APK 文件。".to_string());
    }
    let paths = request
        .apk_paths
        .iter()
        .map(|path| {
            fs::canonicalize(path.trim()).map_err(|error| format!("无法读取 APK 文件：{error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().any(|path| {
        !path.is_file()
            || !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"))
    }) {
        return Err("Split APK 列表中包含无效 APK 文件。".to_string());
    }
    for path in &paths {
        check_apk_device_compatibility(&serial, path)?;
    }
    let manager = state.inner().clone();
    let target = format!("{}:{} 个 APK", serial, paths.len());
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "split-apk-install",
        &target,
        "正在准备 Split APK 安装...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let artifact = paths.first().map(|path| path.to_string_lossy().to_string());
        let result = (|| -> Result<String, String> {
            if cancellation.load(Ordering::Relaxed) {
                return Err("任务在安装前已取消。".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                45,
                "正在通过 ADB 安装 Split APK...",
            )?;
            android_tasks::enter_non_cancellable_phase(
                &task_app,
                &manager,
                &task_id_for_thread,
                "已进入 ADB 多 APK 设备安装阶段；后续将记录实际安装结果。",
            )?;
            let adb = find_adb()?;
            let mut command = Command::new(&adb);
            command.args(["-s", serial.as_str(), "install-multiple"]);
            if request.replace {
                command.arg("-r");
            }
            command.args(&paths);
            let output = command_output_with_timeout(command, Duration::from_secs(5 * 60))
                .map_err(|error| format!("无法安装 Split APK：{error}"))?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .trim()
            .to_string();
            let refs = paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            let arg_refs = std::iter::once("-s".to_string())
                .chain(std::iter::once(serial.clone()))
                .chain(std::iter::once("install-multiple".to_string()))
                .chain(if request.replace {
                    vec!["-r".to_string()]
                } else {
                    Vec::new()
                })
                .chain(refs)
                .collect::<Vec<_>>();
            let arg_refs = arg_refs.iter().map(String::as_str).collect::<Vec<_>>();
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "安装 Split APK",
                &adb.to_string_lossy(),
                &arg_refs,
                &output,
            )?;
            if !output.status.success() {
                return Err(if text.is_empty() {
                    "Split APK 安装失败。".to_string()
                } else {
                    text
                });
            }
            Ok(text)
        })();
        let detail = result.as_deref().unwrap_or_else(|error| error);
        let _ = android_tasks::record_task_audit(
            &task_app,
            &task_id_for_thread,
            "install-result",
            detail,
        );
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &task_id_for_thread,
            artifact.as_deref(),
            error,
        );
    });
    Ok(task_id)
}

#[tauri::command]
pub fn dump_uiautomator_hierarchy(request: UiHierarchyDumpRequest) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?;
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "UI 层级保存路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 UI 层级目录：{error}"))?;
    let remote = "/data/local/tmp/android-tool-ui-hierarchy.xml";
    let dumped = adb_command_output(
        &[
            "-s",
            serial,
            "shell",
            "uiautomator",
            "dump",
            "--compressed",
            remote,
        ],
        Duration::from_secs(12),
    )
    .map_err(|error| format!("无法导出 UI Automator 层级：{error}"))?;
    if !dumped.status.success() {
        let stderr = String::from_utf8_lossy(&dumped.stderr);
        let stdout = String::from_utf8_lossy(&dumped.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.as_ref()
        } else {
            stderr.as_ref()
        };
        return Err(format!(
            "UI Automator 导出失败：{}",
            normalize_uiautomator_error(detail)
        ));
    }
    let destination_argument = destination.to_string_lossy().to_string();
    let pull = adb_command_output(
        &["-s", serial, "pull", remote, &destination_argument],
        Duration::from_secs(60),
    )
    .map_err(|error| format!("无法拉取 UI 层级：{error}"))?;
    let _ = adb_command_output(
        &["-s", serial, "shell", "rm", "-f", remote],
        Duration::from_secs(30),
    );
    if !pull.status.success() {
        return Err(format!(
            "无法拉取 UI 层级：{}",
            String::from_utf8_lossy(&pull.stderr).trim()
        ));
    }
    Ok(destination.to_string_lossy().to_string())
}

#[tauri::command]
pub fn start_uiautomator_hierarchy_dump(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: UiHierarchyDumpRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "UI 层级保存路径无效。".to_string())?
        .to_path_buf();
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "uiautomator",
        &serial,
        "正在准备 UI 层级导出...",
    )?;
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "export-plan",
        &format!("serial={serial}\ndestination={}", destination.display()),
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            fs::create_dir_all(&parent)
                .map_err(|error| format!("无法创建 UI 层级目录：{error}"))?;
            let adb = find_adb()?;
            let remote = "/data/local/tmp/android-tool-ui-hierarchy.xml";
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                15,
                "正在读取设备当前界面...",
            )?;
            let dump_args = [
                "-s",
                serial.as_str(),
                "shell",
                "uiautomator",
                "dump",
                "--compressed",
                remote,
            ];
            let mut dump_command = Command::new(&adb);
            dump_command.args(dump_args);
            // UI Automator often reports the saved XML path on stdout, while
            // device and permission failures are written to stderr. Capture
            // both streams so the task audit explains a failed export.
            let mut child =
                CapturedCommandOutput::spawn_with_output_limit(dump_command, 128 * 1024)
                    .map_err(|error| format!("无法启动 UI Automator：{error}"))?;
            let started = SystemTime::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    return Err("请求已取消".to_string());
                }
                match child
                    .try_wait()
                    .map_err(|error| format!("无法等待 UI Automator：{error}"))?
                {
                    Some(status) => {
                        let dumped = child
                            .finish(status)
                            .map_err(|error| format!("无法读取 UI Automator 输出：{error}"))?;
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "uiautomator-dump",
                            &adb.to_string_lossy(),
                            &dump_args,
                            &dumped,
                        )?;
                        if !dumped.status.success() {
                            let stderr = String::from_utf8_lossy(&dumped.stderr).trim().to_string();
                            let stdout = String::from_utf8_lossy(&dumped.stdout).trim().to_string();
                            let detail = normalize_uiautomator_error(if stderr.is_empty() {
                                &stdout
                            } else {
                                &stderr
                            });
                            return Err(format!(
                                "UI Automator 导出失败（退出码 {:?}）：{}",
                                dumped.status.code(),
                                detail,
                            ));
                        }
                        break;
                    }
                    None if started.elapsed().unwrap_or_default() > Duration::from_secs(12) => {
                        child.terminate();
                        return Err(
                            "UI Automator 在 12 秒内未返回，设备当前界面可能不支持层级导出。"
                                .to_string(),
                        );
                    }
                    None => std::thread::sleep(Duration::from_millis(100)),
                }
            }
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                62,
                "正在拉取 UI 层级 XML...",
            )?;
            let destination_argument = destination.to_string_lossy().to_string();
            let pull_args = [
                "-s",
                serial.as_str(),
                "pull",
                remote,
                destination_argument.as_str(),
            ];
            let pull = adb_command_output(&pull_args, Duration::from_secs(60))
                .map_err(|error| format!("无法拉取 UI 层级：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "pull-ui-hierarchy",
                &adb.to_string_lossy(),
                &pull_args,
                &pull,
            )?;
            let cleanup = adb_command_output(
                &["-s", &serial, "shell", "rm", "-f", remote],
                Duration::from_secs(30),
            );
            if let Ok(cleanup) = cleanup {
                let _ = android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    "cleanup-ui-hierarchy",
                    &adb.to_string_lossy(),
                    &["-s", &serial, "shell", "rm", "-f", remote],
                    &cleanup,
                );
            }
            if !pull.status.success() {
                return Err(format!(
                    "无法拉取 UI 层级：{}",
                    String::from_utf8_lossy(&pull.stderr).trim()
                ));
            }
            let metadata = fs::metadata(&destination)
                .map_err(|error| format!("无法读取 UI 层级结果：{error}"))?;
            if metadata.len() == 0 {
                return Err("UI 层级导出结果为空。".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                95,
                "UI 层级 XML 已保存，正在完成任务...",
            )?;
            Ok(destination)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn start_scrcpy(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    request: ScrcpyRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let mut sessions = tools
        .scrcpy
        .lock()
        .map_err(|_| "scrcpy 状态不可用。".to_string())?;
    if sessions.contains_key(&serial) {
        return Err("该设备的 scrcpy 已在运行。".to_string());
    }
    let manager = state.inner().clone();
    let (task_id, cancellation) =
        android_tasks::create_task(&app, &manager, "scrcpy", &serial, "正在启动 scrcpy 投屏...")?;
    let scrcpy = match find_scrcpy() {
        Some(path) => path,
        None => {
            let error =
                "未找到 scrcpy。请将其放入 %LOCALAPPDATA%\\Android Toolkit\\tools。".to_string();
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&error));
            return Err(error);
        }
    };
    let mut command = background_command(&scrcpy);
    command.args(["-s", &serial, "--window-title", "Android Toolkit · scrcpy"]);
    if request.no_audio {
        command.arg("--no-audio");
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("无法启动 scrcpy：{error}");
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&message));
            return Err(message);
        }
    };
    let capture = ScrcpyCapture {
        child: Arc::new(Mutex::new(child)),
        task_id: task_id.clone(),
    };
    sessions.insert(serial.clone(), capture.clone());
    drop(sessions);
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "command",
        &format!(
            "启动 scrcpy\n命令: {} -s {serial} --window-title Android Toolkit · scrcpy{}",
            scrcpy.display(),
            if request.no_audio { " --no-audio" } else { "" }
        ),
    )?;
    android_tasks::update_task(
        &app,
        &task_id,
        15,
        "scrcpy 投屏已启动；关闭投屏或在 Android 任务中取消即可结束。",
    )?;
    let sessions = tools.scrcpy.clone();
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let active = sessions
            .lock()
            .ok()
            .and_then(|items| items.get(&serial).cloned())
            .filter(|capture| capture.task_id == task_id_for_thread);
        let Some(active) = active else {
            break;
        };
        if cancellation.load(Ordering::Relaxed) {
            if let Ok(mut items) = sessions.lock() {
                items.remove(&serial);
            }
            if let Ok(mut child) = active.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "cancelled",
                "已按用户请求结束 scrcpy 投屏进程。",
            );
            let _ =
                android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, None, None);
            break;
        }
        let completed = active
            .child
            .lock()
            .ok()
            .and_then(|mut child| child.try_wait().ok())
            .flatten();
        if let Some(status) = completed {
            if let Ok(mut items) = sessions.lock() {
                items.remove(&serial);
            }
            let detail = format!("scrcpy 投屏进程已退出（{status}）。");
            let _ = android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "process-exit",
                &detail,
            );
            let _ =
                android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, None, None);
            break;
        }
    });
    Ok(task_id)
}

#[tauri::command]
pub fn stop_scrcpy(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    serial: String,
) -> Result<(), String> {
    let serial = checked_serial(&serial)?.to_string();
    let capture = tools
        .scrcpy
        .lock()
        .map_err(|_| "scrcpy 状态不可用。".to_string())?
        .remove(&serial);
    let Some(capture) = capture else {
        return Ok(());
    };
    let manager = state.inner().clone();
    let result = if let Ok(mut child) = capture.child.lock() {
        child
            .kill()
            .map_err(|error| format!("无法停止 scrcpy：{error}"))
    } else {
        Err("scrcpy 进程状态不可用。".to_string())
    };
    let detail = match &result {
        Ok(()) => "已从设备控制页面关闭 scrcpy 投屏。",
        Err(error) => error.as_str(),
    };
    let _ = android_tasks::record_task_audit(&app, &capture.task_id, "stop", detail);
    let error = result.as_ref().err().map(String::as_str);
    let _ = android_tasks::finish_task(&app, &manager, &capture.task_id, None, error);
    result
}
