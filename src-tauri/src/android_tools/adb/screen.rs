use super::*;

#[tauri::command]
pub fn start_adb_screen_record(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    serial: String,
) -> Result<String, String> {
    let serial = checked_serial(&serial)?.to_string();
    let mut sessions = tools
        .screen_records
        .lock()
        .map_err(|_| "录屏状态不可用。".to_string())?;
    if sessions.contains_key(&serial) {
        return Err("该设备正在录屏。".to_string());
    }
    let remote = format!(
        "/sdcard/Download/android-tool-record-{}.mp4",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "screen-record",
        &serial,
        "正在启动设备录屏...",
    )?;
    let adb = match find_adb() {
        Ok(adb) => adb,
        Err(error) => {
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&error));
            return Err(error);
        }
    };
    let command_args = [
        "-s",
        serial.as_str(),
        "shell",
        "screenrecord",
        "--time-limit",
        "180",
        remote.as_str(),
    ];
    // The recording process is polled for completion, not streamed. Leaving an
    // unread stderr pipe here can eventually block screenrecord on a verbose
    // device, so inherit neither output stream for this long-lived process.
    let child = match background_command(&adb)
        .args(command_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let message = format!("无法开始录屏：{error}");
            let _ = android_tasks::finish_task(&app, &manager, &task_id, None, Some(&message));
            return Err(message);
        }
    };
    let capture = ScreenRecordCapture {
        child: Arc::new(Mutex::new(child)),
        remote: remote.clone(),
        task_id: task_id.clone(),
    };
    sessions.insert(serial.clone(), capture.clone());
    drop(sessions);
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "command",
        &format!(
            "启动设备录屏\n命令: {} {}\n远端产物: {remote}",
            adb.display(),
            command_args.join(" ")
        ),
    )?;
    android_tasks::update_task(
        &app,
        &task_id,
        15,
        "正在录制设备画面；停止后选择本地位置导出 MP4。",
    )?;
    let sessions = tools.screen_records.clone();
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
            let cleanup = adb_output(&["-s", &serial, "shell", "rm", "-f", &active.remote]);
            let detail = match cleanup {
                Ok(output) => format!(
                    "用户取消设备录屏，已结束录屏并清理远端文件。\n{}",
                    output.trim()
                ),
                Err(error) => format!("用户取消设备录屏，远端清理失败：{error}"),
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
            let message =
                format!("设备录屏进程已结束（{status}）。超过 180 秒的录制需要重新开始。");
            let _ = android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "recording-ended",
                &message,
            );
            let _ = android_tasks::finish_task(
                &task_app,
                &manager,
                &task_id_for_thread,
                None,
                Some(&message),
            );
            break;
        }
    });
    Ok(task_id)
}

#[tauri::command]
pub fn stop_adb_screen_record(
    app: AppHandle,
    tools: State<'_, AndroidToolState>,
    state: State<'_, AndroidTaskManager>,
    request: AdbScreenRecordStopRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let capture = tools
        .screen_records
        .lock()
        .map_err(|_| "录屏状态不可用。".to_string())?
        .remove(&serial)
        .ok_or_else(|| "该设备没有进行中的录屏。".to_string())?;
    let manager = state.inner().clone();
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "录屏保存路径无效。".to_string())?;
    let result = (|| -> Result<String, String> {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建录屏目录：{error}"))?;
        android_tasks::update_task(&app, &capture.task_id, 55, "正在停止设备录屏并拉取 MP4...")?;
        if let Ok(mut child) = capture.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let adb = find_adb()?;
        let destination_argument = destination.to_string_lossy().to_string();
        let pull_args = [
            "-s",
            serial.as_str(),
            "pull",
            capture.remote.as_str(),
            destination_argument.as_str(),
        ];
        let output = adb_command_output(&pull_args, Duration::from_secs(10 * 60))
            .map_err(|error| format!("无法导出录屏：{error}"))?;
        android_tasks::record_command_audit(
            &app,
            &capture.task_id,
            "pull-screen-record",
            &adb.to_string_lossy(),
            &pull_args,
            &output,
        )?;
        let cleanup = adb_command_output(
            &["-s", &serial, "shell", "rm", "-f", &capture.remote],
            Duration::from_secs(30),
        );
        if let Ok(cleanup) = cleanup {
            let _ = android_tasks::record_command_audit(
                &app,
                &capture.task_id,
                "cleanup-screen-record",
                &adb.to_string_lossy(),
                &["-s", &serial, "shell", "rm", "-f", &capture.remote],
                &cleanup,
            );
        }
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        let size = fs::metadata(&destination)
            .map_err(|error| format!("无法读取录屏产物：{error}"))?
            .len();
        if size == 0 {
            return Err("设备录屏产物为空。".to_string());
        }
        android_tasks::update_task(&app, &capture.task_id, 95, "MP4 已保存，正在完成任务...")?;
        Ok(destination.to_string_lossy().to_string())
    })();
    let artifact = result.as_ref().ok().map(String::as_str);
    let error = result.as_ref().err().map(String::as_str);
    let _ = android_tasks::finish_task(&app, &manager, &capture.task_id, artifact, error);
    result
}
