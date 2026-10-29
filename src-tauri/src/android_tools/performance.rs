use super::*;

#[tauri::command]
pub fn start_adb_bugreport(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: BugreportRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    if destination
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("zip"))
    {
        return Err("Bugreport 产物请保存为 .zip 文件。".to_string());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "Bugreport 保存路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 Bugreport 目录：{error}"))?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "bugreport",
        &serial,
        "正在启动 Bugreport 采集...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在向设备请求 Bugreport...",
            )?;
            let adb = find_adb()?;
            let destination_argument = destination.to_string_lossy().to_string();
            let bugreport_args = [
                "-s",
                serial.as_str(),
                "bugreport",
                destination_argument.as_str(),
            ];
            let mut command = Command::new(&adb);
            command.args(bugreport_args);
            let mut child = CapturedCommandOutput::spawn(command)
                .map_err(|error| format!("无法启动 Bugreport：{error}"))?;
            let started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    android_tasks::record_task_audit(
                        &task_app,
                        &task_id_for_thread,
                        "cancelled",
                        "Bugreport 已按用户请求取消。",
                    )?;
                    return Ok(());
                }
                match child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Bugreport：{error}"))?
                {
                    Some(status) => {
                        let completed = child
                            .finish(status)
                            .map_err(|error| format!("无法读取 Bugreport 结果：{error}"))?;
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "导出 Bugreport",
                            "adb",
                            &bugreport_args,
                            &completed,
                        )?;
                        if !completed.status.success() {
                            return Err(format!(
                                "Bugreport 采集失败：{}",
                                String::from_utf8_lossy(&completed.stderr).trim()
                            ));
                        }
                        break;
                    }
                    None if started.elapsed() >= Duration::from_secs(20 * 60) => {
                        child.terminate();
                        android_tasks::record_task_audit(
                            &task_app,
                            &task_id_for_thread,
                            "command",
                            "Bugreport 超过 20 分钟未完成，已停止命令。",
                        )?;
                        return Err(
                            "Bugreport 超过 20 分钟未完成。设备可能未响应或空间不足。".to_string()
                        );
                    }
                    None => std::thread::sleep(Duration::from_millis(250)),
                }
            }
            if !destination.is_file() {
                return Err("设备未生成 Bugreport ZIP 文件。".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                95,
                "正在保存 Bugreport 产物...",
            )?;
            Ok(())
        })();
        let artifact = result.as_ref().ok().and_then(|_| destination.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

pub(super) fn perfetto_capture_progress(duration_seconds: u32, elapsed_seconds: u32) -> u8 {
    let duration = duration_seconds.max(1);
    15 + (elapsed_seconds.min(duration).saturating_mul(70) / duration) as u8
}

#[tauri::command]
pub fn start_perfetto_capture(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: PerfettoCaptureRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "Perfetto 保存路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 Perfetto 目录：{error}"))?;
    let duration = request.duration_seconds.clamp(1, 300);
    let categories: Vec<&str> = match request.preset.as_str() {
        "ui" => vec![
            "sched/sched_switch",
            "sched/sched_wakeup",
            "gfx",
            "view",
            "wm",
            "am",
        ],
        "cpu" => vec![
            "sched/sched_switch",
            "sched/sched_wakeup",
            "power/cpu_frequency",
        ],
        _ => vec!["sched/sched_switch", "sched/sched_wakeup"],
    };
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "perfetto",
        &serial,
        "正在准备 Perfetto 采集...",
    )?;
    let task_id_for_thread = task_id.clone();
    let task_app = app.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在启动设备端 Perfetto...",
            )?;
            let adb = find_adb()?;
            let duration_arg = format!("{duration}s");
            let remote_path = format!(
                "/data/misc/perfetto-traces/android-tool-{}.pf",
                task_id_for_thread
            );
            let mut capture_args = vec![
                "-s".to_string(),
                serial.clone(),
                "shell".to_string(),
                "perfetto".to_string(),
                "-t".to_string(),
                duration_arg,
                "-o".to_string(),
                remote_path.clone(),
            ];
            capture_args.extend(categories.iter().map(|category| category.to_string()));
            let mut command = Command::new(&adb);
            command.args(&capture_args);
            let mut child = CapturedCommandOutput::spawn(command)
                .map_err(|error| format!("无法启动 Perfetto：{error}"))?;
            let capture_started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    let cleanup = adb_command_output(
                        &["-s", &serial, "shell", "rm", "-f", &remote_path],
                        Duration::from_secs(30),
                    );
                    if let Ok(output) = cleanup {
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "清理已取消的 Perfetto Trace",
                            "adb",
                            &["-s", &serial, "shell", "rm", "-f", &remote_path],
                            &output,
                        )?;
                    }
                    android_tasks::record_task_audit(
                        &task_app,
                        &task_id_for_thread,
                        "cancelled",
                        "Perfetto 采集已按用户请求取消。",
                    )?;
                    return Ok(());
                }
                match child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Perfetto：{error}"))?
                {
                    Some(status) => {
                        let captured = child
                            .finish(status)
                            .map_err(|error| format!("无法读取 Perfetto 结果：{error}"))?;
                        let capture_refs =
                            capture_args.iter().map(String::as_str).collect::<Vec<_>>();
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "设备端 Perfetto 采集",
                            "adb",
                            &capture_refs,
                            &captured,
                        )?;
                        if !captured.status.success() {
                            return Err(format!(
                                "Perfetto 采集失败：{}",
                                String::from_utf8_lossy(&captured.stderr).trim()
                            ));
                        }
                        break;
                    }
                    None => {
                        let elapsed =
                            capture_started.elapsed().as_secs().min(duration as u64) as u32;
                        let percent = perfetto_capture_progress(duration, elapsed);
                        android_tasks::update_task(
                            &task_app,
                            &task_id_for_thread,
                            percent,
                            "正在采集性能 Trace...",
                        )?;
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                85,
                "正在拉取性能 Trace...",
            )?;
            let destination_argument = destination.to_string_lossy().to_string();
            let pull_args = [
                "-s",
                serial.as_str(),
                "pull",
                remote_path.as_str(),
                destination_argument.as_str(),
            ];
            let pull = adb_command_output(&pull_args, Duration::from_secs(5 * 60))
                .map_err(|error| format!("无法拉取 Perfetto Trace：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "拉取 Perfetto Trace",
                "adb",
                &pull_args,
                &pull,
            )?;
            let cleanup_args = [
                "-s",
                serial.as_str(),
                "shell",
                "rm",
                "-f",
                remote_path.as_str(),
            ];
            let cleanup = adb_command_output(&cleanup_args, Duration::from_secs(30));
            if let Ok(output) = cleanup {
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    "清理设备端 Perfetto Trace",
                    "adb",
                    &cleanup_args,
                    &output,
                )?;
            }
            if !pull.status.success() {
                return Err(format!(
                    "无法拉取 Perfetto Trace：{}",
                    String::from_utf8_lossy(&pull.stderr).trim()
                ));
            }
            if fs::metadata(&destination)
                .map_err(|error| format!("无法检查 Trace 文件：{error}"))?
                .len()
                == 0
            {
                return Err("Perfetto 未返回 Trace 数据。".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                95,
                "正在保存性能 Trace...",
            )?;
            Ok(())
        })();
        let artifact = result.as_ref().ok().and_then(|_| destination.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn run_perfetto_sql(app: AppHandle, request: PerfettoSqlRequest) -> Result<String, String> {
    let target = request.trace_path.trim().to_string();
    let query_bytes = request.query.len();
    let result = run_perfetto_sql_inner(request);
    let detail = match &result {
        Ok(output) => format!(
            "查询已完成：{} 字节 SQL，{} 字节输出",
            query_bytes,
            output.len()
        ),
        Err(error) => error.clone(),
    };
    let _ = android_tasks::record_operation_audit(
        &app,
        "Perfetto SQL 查询",
        &target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

pub(super) fn run_perfetto_sql_inner(request: PerfettoSqlRequest) -> Result<String, String> {
    let trace = fs::canonicalize(request.trace_path.trim())
        .map_err(|error| format!("无法读取 Trace 文件：{error}"))?;
    if !trace.is_file() {
        return Err("请选择有效的 Perfetto Trace 文件。".to_string());
    }
    if fs::metadata(&trace)
        .map_err(|error| format!("无法读取 Trace 文件：{error}"))?
        .len()
        > 2 * 1024 * 1024 * 1024
    {
        return Err("Trace 文件超过 2 GB 查询限制。".to_string());
    }
    let query = request.query.trim();
    if query.is_empty() || query.len() > 32 * 1024 {
        return Err("SQL 查询不能为空且不能超过 32 KB。".to_string());
    }
    let normalized = query.trim_start().to_ascii_lowercase();
    if !(normalized.starts_with("select") || normalized.starts_with("with"))
        || query.matches(';').count() > 1
    {
        return Err("仅支持单条只读 SELECT 或 WITH 查询。".to_string());
    }
    let processor = find_trace_processor().ok_or_else(|| "未找到 Trace Processor。请将 trace_processor_shell 放入 %LOCALAPPDATA%\\Android Toolkit\\tools。".to_string())?;
    let mut command = Command::new(processor);
    command.args(["query"]).arg(&trace).arg(query);
    let output = command_output_with_timeout(command, Duration::from_secs(60))
        .map_err(|error| format!("无法执行 Perfetto SQL：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Perfetto SQL 执行失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub fn diagnose_perfetto_trace(
    app: AppHandle,
    trace_path: String,
) -> Result<PerfettoDiagnosis, String> {
    let target = trace_path.trim().to_string();
    let result = diagnose_perfetto_trace_inner(&target);
    let detail = match &result {
        Ok(diagnosis) => format!(
            "诊断已完成：{} 项可用，{} 项不可用",
            diagnosis
                .sections
                .iter()
                .filter(|section| section.available)
                .count(),
            diagnosis
                .sections
                .iter()
                .filter(|section| !section.available)
                .count(),
        ),
        Err(error) => error.clone(),
    };
    let _ = android_tasks::record_operation_audit(
        &app,
        "Perfetto Trace 诊断",
        &target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

pub(super) fn diagnose_perfetto_trace_inner(trace_path: &str) -> Result<PerfettoDiagnosis, String> {
    if trace_path.is_empty() {
        return Err("请选择 Perfetto Trace 文件。".to_string());
    }
    let checks = [
        ("采集时长", "SELECT round((max(ts + dur) - min(ts))/1e9, 3) AS trace_seconds FROM sched"),
        ("调度记录", "SELECT count(*) AS sched_rows FROM sched"),
        ("CPU 忙碌度", "WITH bounds AS (SELECT min(ts) AS start_ts, max(ts + dur) AS end_ts FROM sched) SELECT cpu, round(sum(dur)/1e6, 2) AS running_ms, round(100.0 * sum(dur) / (SELECT end_ts-start_ts FROM bounds), 1) AS busy_percent, count(*) AS switches FROM sched GROUP BY cpu ORDER BY busy_percent DESC LIMIT 8"),
        ("CPU 热点线程", "SELECT coalesce(process.name, 'kernel') AS process_name, thread.name AS thread_name, round(sum(sched.dur)/1e6, 2) AS running_ms FROM sched JOIN thread USING(utid) LEFT JOIN process USING(upid) GROUP BY process.name, thread.name ORDER BY running_ms DESC LIMIT 20"),
        ("Runnable 排队", "SELECT coalesce(process.name, 'kernel') AS process_name, thread.name AS thread_name, round(sum(thread_state.dur)/1e6, 2) AS runnable_ms FROM thread_state JOIN thread USING(utid) LEFT JOIN process USING(upid) WHERE state = 'R' GROUP BY process.name, thread.name ORDER BY runnable_ms DESC LIMIT 20"),
        ("启动线索", "SELECT coalesce(process.name, 'unknown') AS process_name, slice.name, round(slice.ts/1e9, 3) AS started_at_seconds, round(slice.dur/1e6, 2) AS duration_ms FROM slice LEFT JOIN thread_track ON slice.track_id = thread_track.id LEFT JOIN thread USING(utid) LEFT JOIN process USING(upid) WHERE lower(slice.name) LIKE '%launch%' OR lower(slice.name) LIKE '%start activity%' OR lower(slice.name) LIKE '%activitytaskmanager%' ORDER BY slice.ts ASC LIMIT 30"),
        ("界面帧卡顿线索", "SELECT name, round(dur/1e6, 2) AS duration_ms, round(ts/1e9, 3) AS timestamp_seconds FROM slice WHERE (name LIKE '%Choreographer#doFrame%' OR name LIKE '%DrawFrame%' OR name LIKE '%deliverInputEvent%') AND dur >= 16000000 ORDER BY dur DESC LIMIT 30"),
        ("长耗时 Slice", "SELECT name, dur/1e6 AS duration_ms FROM slice WHERE dur > 10000000 ORDER BY dur DESC LIMIT 20"),
    ];
    let mut sections = Vec::with_capacity(checks.len());
    for (title, query) in checks {
        match run_perfetto_sql_inner(PerfettoSqlRequest {
            trace_path: trace_path.to_string(),
            query: query.to_string(),
        }) {
            Ok(output) => sections.push(PerfettoDiagnosisSection {
                title: title.to_string(),
                query: query.to_string(),
                output,
                available: true,
            }),
            Err(error) => sections.push(PerfettoDiagnosisSection {
                title: title.to_string(),
                query: query.to_string(),
                output: format!("当前 Trace 不支持此项：{error}"),
                available: false,
            }),
        }
    }
    Ok(PerfettoDiagnosis {
        trace_path: trace_path.to_string(),
        sections,
    })
}

/// Records CPU samples with the device's bundled simpleperf. The raw data and
/// a CSV report are kept together in the user-selected artifact directory.
pub(super) fn simpleperf_capture_progress(duration_seconds: u32, elapsed_seconds: u32) -> u8 {
    let duration = duration_seconds.max(1);
    15 + (elapsed_seconds.min(duration).saturating_mul(60) / duration) as u8
}

pub(super) fn normalize_simpleperf_error(raw: &str) -> String {
    if raw.contains("System wide profiling needs root privilege") {
        return format!("设备拒绝全系统 Simpleperf 采样：需要 root adbd。可在已 root 的设备上重试，或选择允许采样的应用进程。原始错误：{raw}");
    }
    if raw.contains("Permission denied") && raw.contains("perf event") {
        return format!(
            "设备拒绝 Simpleperf perf_event 权限；当前用户或应用不允许 CPU 采样。原始错误：{raw}"
        );
    }
    if raw.contains("No threads to monitor") {
        return format!(
            "没有可供 Simpleperf 采样的目标线程。请启动目标应用后重试。原始错误：{raw}"
        );
    }
    if raw.is_empty() {
        "Simpleperf 采样失败，但设备没有返回错误详情。".to_string()
    } else {
        raw.to_string()
    }
}

pub(super) fn normalize_uiautomator_error(raw: &str) -> String {
    let detail = raw.trim();
    if detail
        .to_ascii_lowercase()
        .contains("null root node returned by uitestautomationbridge")
    {
        return format!(
            "UI Automator 无法读取当前前台界面。设备可能处于锁屏、系统过渡或 ANR 对话框中；请先解锁或恢复系统界面后重试。原始输出：{detail}"
        );
    }
    if detail.is_empty() {
        "设备没有返回错误详情。".to_string()
    } else {
        detail.to_string()
    }
}

/// Android 15+ can accept the legacy WindowManager tracing command with a
/// zero exit code while explicitly ignoring it. Treat that as unsupported so
/// a stale trace is never presented as a newly captured WinScope artifact.
pub(super) fn winscope_trace_command_error(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let normalized = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if normalized.contains("shell commands are ignored")
        || normalized.contains("should be performed through perfetto")
    {
        return Some("当前 Android 版本已忽略 WindowManager Shell Trace 命令，需要通过 Perfetto 采集对应窗口事件；本次不会导出旧 Trace 文件。".to_string());
    }
    if !success || normalized.contains("error") {
        return Some(if detail.is_empty() {
            "WindowManager Trace 命令失败，设备没有返回错误详情。".to_string()
        } else {
            detail.to_string()
        });
    }
    None
}

#[tauri::command]
pub fn start_simpleperf_capture(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: SimpleperfCaptureRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    if request.destination.trim().is_empty() {
        return Err("请选择 Simpleperf 产物目录。".to_string());
    }
    let duration = request.duration_seconds.clamp(1, 120);
    let package_name = request.package_name.trim().to_string();
    if !package_name.is_empty()
        && (!package_name.contains('.')
            || package_name.len() > 255
            || !package_name
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '-')))
    {
        return Err("包名格式无效。".to_string());
    }
    fs::create_dir_all(&destination)
        .map_err(|error| format!("无法创建 Simpleperf 目录：{error}"))?;
    let manager = state.inner().clone();
    let target = if package_name.is_empty() {
        serial.clone()
    } else {
        format!("{serial}:{package_name}")
    };
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "simpleperf",
        &target,
        "正在准备 Native CPU 热点采样...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let adb = find_adb()?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在检查设备端 Simpleperf...",
            )?;
            let check = adb_command_output(
                &["-s", &serial, "shell", "simpleperf", "--version"],
                Duration::from_secs(30),
            )
            .map_err(|error| format!("无法检查 Simpleperf：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "检查设备端 Simpleperf",
                "adb",
                &["-s", &serial, "shell", "simpleperf", "--version"],
                &check,
            )?;
            if !check.status.success() {
                return Err(format!(
                    "设备不支持 Simpleperf：{}",
                    String::from_utf8_lossy(&check.stderr).trim()
                ));
            }
            let remote_path =
                format!("/data/local/tmp/android-tool-simpleperf-{task_id_for_thread}.data");
            let mut args = vec![
                "-s".to_string(),
                serial.clone(),
                "shell".to_string(),
                "simpleperf".to_string(),
                "record".to_string(),
            ];
            if package_name.is_empty() {
                args.push("-a".to_string());
            } else {
                let pid = adb_command_output(
                    &["-s", &serial, "shell", "pidof", &package_name],
                    Duration::from_secs(30),
                )
                .map_err(|error| format!("无法查询应用进程：{error}"))?;
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    "查询 Simpleperf 应用进程",
                    "adb",
                    &["-s", &serial, "shell", "pidof", &package_name],
                    &pid,
                )?;
                let pid = String::from_utf8_lossy(&pid.stdout)
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| "指定应用当前没有可采样进程。".to_string())?
                    .to_string();
                args.extend(["-p".to_string(), pid]);
            }
            args.extend([
                "--duration".to_string(),
                duration.to_string(),
                "-g".to_string(),
                "-o".to_string(),
                remote_path.clone(),
            ]);
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                15,
                "正在采集 Native CPU 样本...",
            )?;
            let mut command = Command::new(&adb);
            command.args(&args);
            let mut child = CapturedCommandOutput::spawn(command)
                .map_err(|error| format!("无法启动 Simpleperf：{error}"))?;
            let capture_started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    let cleanup = adb_command_output(
                        &["-s", &serial, "shell", "rm", "-f", &remote_path],
                        Duration::from_secs(30),
                    );
                    if let Ok(output) = cleanup {
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "清理已取消的 Simpleperf 数据",
                            "adb",
                            &["-s", &serial, "shell", "rm", "-f", &remote_path],
                            &output,
                        )?;
                    }
                    android_tasks::record_task_audit(
                        &task_app,
                        &task_id_for_thread,
                        "cancelled",
                        "Simpleperf 采样已按用户请求取消。",
                    )?;
                    return Err("请求已取消".to_string());
                }
                match child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Simpleperf：{error}"))?
                {
                    Some(status) => {
                        let recorded = child
                            .finish(status)
                            .map_err(|error| format!("无法读取 Simpleperf 采样结果：{error}"))?;
                        let record_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                        android_tasks::record_command_audit(
                            &task_app,
                            &task_id_for_thread,
                            "设备端 Simpleperf 采样",
                            "adb",
                            &record_refs,
                            &recorded,
                        )?;
                        if !recorded.status.success() {
                            return Err(format!(
                                "Simpleperf 采样失败：{}",
                                normalize_simpleperf_error(
                                    String::from_utf8_lossy(&recorded.stderr).trim()
                                )
                            ));
                        }
                        break;
                    }
                    None => {
                        let elapsed =
                            capture_started.elapsed().as_secs().min(duration as u64) as u32;
                        let percent = simpleperf_capture_progress(duration, elapsed);
                        android_tasks::update_task(
                            &task_app,
                            &task_id_for_thread,
                            percent,
                            "正在采集 Native CPU 样本...",
                        )?;
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            }
            if cancellation.load(Ordering::Relaxed) {
                let cleanup = adb_command_output(
                    &["-s", &serial, "shell", "rm", "-f", &remote_path],
                    Duration::from_secs(30),
                );
                if let Ok(output) = cleanup {
                    android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        "清理已取消的 Simpleperf 数据",
                        "adb",
                        &["-s", &serial, "shell", "rm", "-f", &remote_path],
                        &output,
                    )?;
                }
                android_tasks::record_task_audit(
                    &task_app,
                    &task_id_for_thread,
                    "cancelled",
                    "Simpleperf 采集完成后收到取消请求，已清理设备端临时数据。",
                )?;
                return Err("请求已取消".to_string());
            }
            android_tasks::update_task(&task_app, &task_id_for_thread, 78, "正在导出热点报告...")?;
            let report = adb_command_output(
                &[
                    "-s",
                    &serial,
                    "shell",
                    "simpleperf",
                    "report",
                    "-i",
                    &remote_path,
                    "--csv",
                    "--percent-limit",
                    "0.5",
                ],
                Duration::from_secs(2 * 60),
            )
            .map_err(|error| format!("无法生成 Simpleperf 报告：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "生成 Simpleperf CSV 报告",
                "adb",
                &[
                    "-s",
                    &serial,
                    "shell",
                    "simpleperf",
                    "report",
                    "-i",
                    &remote_path,
                    "--csv",
                    "--percent-limit",
                    "0.5",
                ],
                &report,
            )?;
            if !report.status.success() {
                return Err(format!(
                    "Simpleperf 报告失败：{}",
                    String::from_utf8_lossy(&report.stderr).trim()
                ));
            }
            let task_directory = destination.join(format!("simpleperf-{task_id_for_thread}"));
            fs::create_dir_all(&task_directory)
                .map_err(|error| format!("无法创建 Simpleperf 任务目录：{error}"))?;
            let data_path = task_directory.join("perf.data");
            let data_argument = data_path.to_string_lossy().to_string();
            let pull = adb_command_output(
                &["-s", &serial, "pull", &remote_path, &data_argument],
                Duration::from_secs(5 * 60),
            )
            .map_err(|error| format!("无法导出 perf.data：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "拉取 Simpleperf 数据",
                "adb",
                &["-s", &serial, "pull", &remote_path, &data_argument],
                &pull,
            )?;
            let cleanup = adb_command_output(
                &["-s", &serial, "shell", "rm", "-f", &remote_path],
                Duration::from_secs(30),
            );
            if let Ok(output) = cleanup {
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    "清理设备端 Simpleperf 数据",
                    "adb",
                    &["-s", &serial, "shell", "rm", "-f", &remote_path],
                    &output,
                )?;
            }
            if !pull.status.success() {
                return Err(format!(
                    "无法导出 perf.data：{}",
                    String::from_utf8_lossy(&pull.stderr).trim()
                ));
            }
            fs::write(task_directory.join("report.csv"), &report.stdout)
                .map_err(|error| format!("无法保存 Simpleperf CSV：{error}"))?;
            fs::write(task_directory.join("metadata.txt"), format!("serial={serial}\nduration_seconds={duration}\npackage={package_name}\ncreated_at={}\n", now_millis())).map_err(|error| format!("无法保存 Simpleperf 元数据：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                96,
                "正在保存 Native CPU 热点产物...",
            )?;
            Ok(task_directory)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_simpleperf_report(app: AppHandle, task_id: String) -> Result<SimpleperfReport, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("请提供 Simpleperf 任务 ID。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='simpleperf' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Simpleperf 任务。".to_string())?;
    let root =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取 Simpleperf 产物：{error}"))?;
    let report = root.join("report.csv");
    let bytes = fs::read(&report).map_err(|error| format!("无法读取 Simpleperf CSV：{error}"))?;
    let truncated = bytes.len() > 512 * 1024;
    let content = String::from_utf8_lossy(&bytes[..bytes.len().min(512 * 1024)]).into_owned();
    Ok(SimpleperfReport {
        task_id: task_id.to_string(),
        csv: content,
        truncated,
    })
}

/// Captures an ART sampling trace with the platform's `am profile` command.
/// The binary trace is deliberately retained unchanged so Android Studio or
/// compatible tooling can inspect method calls without a lossy re-parser.
#[tauri::command]
pub fn start_method_profile_capture(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: MethodProfileCaptureRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let package_name = checked_package(&request.package_name)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    if request.destination.trim().is_empty() {
        return Err("请选择方法采样产物目录。".to_string());
    }
    let duration = request.duration_seconds.clamp(1, 120);
    let interval = request.sampling_interval_us.clamp(100, 100_000);
    fs::create_dir_all(&destination).map_err(|error| format!("无法创建方法采样目录：{error}"))?;
    let manager = state.inner().clone();
    let target = format!("{serial}:{package_name}");
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "method-profile",
        &target,
        "正在准备 ART 方法采样...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let remote = format!("/data/local/tmp/android-tool-method-{task_id_for_thread}.trace");
            let task_directory = destination.join(format!("method-profile-{task_id_for_thread}"));
            fs::create_dir_all(&task_directory)
                .map_err(|error| format!("无法创建方法采样任务目录：{error}"))?;
            let interval_argument = interval.to_string();
            let start_args = [
                "-s",
                serial.as_str(),
                "shell",
                "am",
                "profile",
                "start",
                "--sampling",
                interval_argument.as_str(),
                package_name.as_str(),
                remote.as_str(),
            ];
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在启动 ART 方法采样...",
            )?;
            let started = adb_command_output(&start_args, Duration::from_secs(30))
                .map_err(|error| format!("无法启动方法采样：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "启动 ART 方法采样",
                "adb",
                &start_args,
                &started,
            )?;
            if !started.status.success() {
                return Err(format!("无法启动方法采样，退出码：{}", started.status));
            }
            let started_at = std::time::Instant::now();
            while started_at.elapsed() < Duration::from_secs(duration as u64) {
                if cancellation.load(Ordering::Relaxed) {
                    break;
                }
                let elapsed = started_at.elapsed().as_secs().min(duration as u64) as u32;
                let progress = 12 + (elapsed.saturating_mul(68) / duration) as u8;
                android_tasks::update_task(
                    &task_app,
                    &task_id_for_thread,
                    progress,
                    "正在采集 ART 方法调用...",
                )?;
                std::thread::sleep(Duration::from_millis(200));
            }
            let stop_args = [
                "-s",
                serial.as_str(),
                "shell",
                "am",
                "profile",
                "stop",
                package_name.as_str(),
            ];
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                82,
                "正在停止 ART 方法采样...",
            )?;
            let stopped = adb_command_output(&stop_args, Duration::from_secs(30))
                .map_err(|error| format!("无法停止方法采样：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "停止 ART 方法采样",
                "adb",
                &stop_args,
                &stopped,
            )?;
            if !stopped.status.success() {
                return Err(format!("无法停止方法采样，退出码：{}", stopped.status));
            }
            if cancellation.load(Ordering::Relaxed) {
                let _ = adb_command_output(
                    &["-s", &serial, "shell", "rm", "-f", &remote],
                    Duration::from_secs(30),
                );
                return Err("请求已取消".to_string());
            }
            let trace = task_directory.join("method.trace");
            let trace_argument = trace.to_string_lossy().to_string();
            let pull_args = [
                "-s",
                serial.as_str(),
                "pull",
                remote.as_str(),
                trace_argument.as_str(),
            ];
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                90,
                "正在导出方法 Trace...",
            )?;
            let pulled = adb_command_output(&pull_args, Duration::from_secs(10 * 60))
                .map_err(|error| format!("无法导出方法 Trace：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "导出 ART 方法 Trace",
                "adb",
                &pull_args,
                &pulled,
            )?;
            let _ = adb_command_output(
                &["-s", &serial, "shell", "rm", "-f", &remote],
                Duration::from_secs(30),
            );
            if !pulled.status.success() {
                return Err(format!("无法导出方法 Trace，退出码：{}", pulled.status));
            }
            let size = fs::metadata(&trace)
                .map_err(|error| format!("无法检查方法 Trace：{error}"))?
                .len();
            if size == 0 {
                return Err("设备未返回 ART 方法 Trace 数据。".to_string());
            }
            fs::write(task_directory.join("metadata.txt"), format!("serial={serial}\npackage={package_name}\nduration_seconds={duration}\nsampling_interval_us={interval}\ntrace_size={size}\ncreated_at={}\n", now_millis())).map_err(|error| format!("无法保存方法采样元数据：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                96,
                "正在保存 ART 方法采样产物...",
            )?;
            Ok(task_directory)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}
