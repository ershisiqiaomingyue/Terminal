use super::*;

pub(super) fn number_field(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(serde_json::Value::as_f64)
}
pub(super) fn parse_android_instrumentations(output: &str) -> Vec<AndroidInstrumentation> {
    output
        .lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("instrumentation:")?;
            let (component, target) = value.split_once(" (target=")?;
            let target_package = target.strip_suffix(')')?.trim();
            if component.contains('/')
                && !component.contains(char::is_whitespace)
                && !target_package.is_empty()
            {
                Some(AndroidInstrumentation {
                    component: component.to_string(),
                    target_package: target_package.to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}
pub(super) fn benchmark_result_remote_path(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("INSTRUMENTATION_RESULT: additionalTestOutputFile_")
        })
        .filter_map(|line| line.split_once('=').map(|(_, path)| path.trim()))
        .find(|path| {
            path.starts_with('/')
                && path.ends_with("benchmarkData.json")
                && path.len() <= 1024
                && !path.contains("..")
                && !path.contains('\0')
        })
        .map(str::to_string)
}
#[tauri::command]
pub fn list_android_instrumentations(
    serial: String,
) -> Result<Vec<AndroidInstrumentation>, String> {
    let serial = checked_serial(&serial)?;
    let output = adb_command_output(
        &["-s", serial, "shell", "pm", "list", "instrumentation"],
        Duration::from_secs(30),
    )
    .map_err(|error| format!("无法读取设备 instrumentation：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取设备 instrumentation：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_android_instrumentations(&String::from_utf8_lossy(
        &output.stdout,
    )))
}
/// Parses AndroidX BenchmarkData JSON without accepting arbitrary nested data.
/// It keeps only stable context and metric summary fields, not raw per-run samples.
pub fn summarize_android_benchmark_json(
    source_path: String,
    content: &[u8],
) -> Result<AndroidBenchmarkSummary, String> {
    if content.len() > 8 * 1024 * 1024 {
        return Err("Benchmark JSON 超过 8 MB。".to_string());
    }
    let root: serde_json::Value = serde_json::from_slice(content)
        .map_err(|error| format!("无法解析 AndroidX Benchmark JSON：{error}"))?;
    let benchmarks = root
        .get("benchmarks")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "不是 AndroidX BenchmarkData：缺少 benchmarks 数组。".to_string())?;
    let context = root.get("context");
    let build = context.and_then(|value| value.get("build"));
    let mut truncated = benchmarks.len() > 100;
    let mut cases = Vec::with_capacity(benchmarks.len().min(100));
    for item in benchmarks.iter().take(100) {
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("未命名基准")
            .to_string();
        let class_name = item
            .get("className")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut metrics = Vec::new();
        for (metric_name, data) in item
            .get("metrics")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|map| map.iter())
            .take(100)
        {
            metrics.push(AndroidBenchmarkMetric {
                name: metric_name.clone(),
                median: number_field(data, "median"),
                minimum: number_field(data, "minimum"),
                maximum: number_field(data, "maximum"),
                p50: None,
                p90: None,
                p95: None,
                p99: None,
            });
        }
        for (metric_name, data) in item
            .get("sampledMetrics")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|map| map.iter())
            .take(100)
        {
            metrics.push(AndroidBenchmarkMetric {
                name: metric_name.clone(),
                median: None,
                minimum: None,
                maximum: None,
                p50: number_field(data, "P50"),
                p90: number_field(data, "P90"),
                p95: number_field(data, "P95"),
                p99: number_field(data, "P99"),
            });
        }
        if metrics.len() >= 200 {
            truncated = true;
        }
        cases.push(AndroidBenchmarkCase {
            name,
            class_name,
            metrics,
        });
    }
    Ok(AndroidBenchmarkSummary {
        source_path,
        device_model: build
            .and_then(|value| value.get("model"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        sdk: build
            .and_then(|value| value.get("version"))
            .and_then(|value| value.get("sdk"))
            .and_then(serde_json::Value::as_u64),
        build_type: build
            .and_then(|value| value.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        benchmarks: cases,
        truncated,
    })
}

#[tauri::command]
pub fn import_android_benchmark_result(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    path: String,
) -> Result<String, String> {
    let source = fs::canonicalize(path.trim())
        .map_err(|error| format!("无法读取 Benchmark JSON：{error}"))?;
    if !source.is_file()
        || source
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("json"))
    {
        return Err("请选择 AndroidX BenchmarkData JSON 文件。".to_string());
    }
    let manager = state.inner().clone();
    let source_label = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("benchmarkData.json")
        .to_string();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "benchmark-import",
        &source_label,
        "正在导入 AndroidX Benchmark 结果...",
    )?;
    let task_app = app.clone();
    let id = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            let bytes =
                fs::read(&source).map_err(|error| format!("无法读取 Benchmark JSON：{error}"))?;
            let summary =
                summarize_android_benchmark_json(source.to_string_lossy().to_string(), &bytes)?;
            let root = task_app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法定位任务目录：{error}"))?
                .join("android-tasks")
                .join("benchmark")
                .join(&id);
            fs::create_dir_all(&root)
                .map_err(|error| format!("无法创建 Benchmark 任务目录：{error}"))?;
            let report = root.join("benchmark-summary.json");
            fs::write(
                &report,
                serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("无法保存 Benchmark 摘要：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &id,
                96,
                "Benchmark 结果已解析，正在保存摘要...",
            )?;
            Ok(report)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(&task_app, &manager, &id, artifact, error);
    });
    Ok(task_id)
}

/// Runs one already-installed instrumentation and only accepts the AndroidX
/// BenchmarkData file explicitly reported by AndroidX in its run-end result.
#[tauri::command]
pub fn start_android_benchmark_run(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: AndroidBenchmarkRunRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let instrumentation = request.instrumentation.trim();
    if instrumentation.len() > 240
        || !instrumentation.contains('/')
        || instrumentation.contains(char::is_whitespace)
    {
        return Err("Instrumentation 组件格式无效。".to_string());
    }
    let available = list_android_instrumentations(serial.clone())?;
    if !available
        .iter()
        .any(|item| item.component == instrumentation)
    {
        return Err("该 instrumentation 未在当前设备上发现，请先刷新列表。".to_string());
    }
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "benchmark-run",
        &format!("{serial}:{instrumentation}"),
        "正在启动 AndroidX Benchmark instrumentation...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    let instrumentation = instrumentation.to_string();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let root = task_app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法定位任务目录：{error}"))?
                .join("android-tasks")
                .join("benchmark")
                .join(&task_id_for_thread);
            fs::create_dir_all(&root)
                .map_err(|error| format!("无法创建 Benchmark 任务目录：{error}"))?;
            let args = [
                "-s",
                serial.as_str(),
                "shell",
                "am",
                "instrument",
                "-w",
                "-r",
                "-e",
                "androidx.benchmark.output.enable",
                "true",
                instrumentation.as_str(),
            ];
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在运行 instrumentation；Benchmark 可能需要数分钟...",
            )?;
            let adb = find_adb()?;
            let mut command = Command::new(&adb);
            command.args(args);
            let mut child = CapturedCommandOutput::spawn(command)
                .map_err(|error| format!("无法启动 Benchmark instrumentation：{error}"))?;
            let started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    return Err("请求已取消".to_string());
                }
                if started.elapsed() > Duration::from_secs(30 * 60) {
                    child.terminate();
                    return Err("Benchmark instrumentation 超过 30 分钟未完成。".to_string());
                }
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Benchmark instrumentation：{error}"))?
                {
                    let output = child.finish(status).map_err(|error| {
                        format!("无法读取 Benchmark instrumentation 输出：{error}")
                    })?;
                    android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        "运行 Benchmark instrumentation",
                        "adb",
                        &args,
                        &output,
                    )?;
                    if !output.status.success() {
                        return Err(format!(
                            "Benchmark instrumentation 失败：{}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        ));
                    }
                    let combined = format!(
                        "{}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let remote = benchmark_result_remote_path(&combined).ok_or_else(|| "该 instrumentation 没有报告 AndroidX BenchmarkData JSON；它可能是普通测试，或 Benchmark 输出被禁用。".to_string())?;
                    android_tasks::update_task(
                        &task_app,
                        &task_id_for_thread,
                        91,
                        "正在拉取 AndroidX BenchmarkData JSON...",
                    )?;
                    let local = root.join("benchmarkData.json");
                    let local_string = local.to_string_lossy().to_string();
                    let pull_args = [
                        "-s",
                        serial.as_str(),
                        "pull",
                        remote.as_str(),
                        local_string.as_str(),
                    ];
                    let pulled = adb_command_output(&pull_args, Duration::from_secs(5 * 60))
                        .map_err(|error| format!("无法拉取 BenchmarkData：{error}"))?;
                    android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        "拉取 BenchmarkData",
                        "adb",
                        &pull_args,
                        &pulled,
                    )?;
                    if !pulled.status.success() {
                        return Err(format!(
                            "无法拉取 BenchmarkData：{}",
                            String::from_utf8_lossy(&pulled.stderr).trim()
                        ));
                    }
                    let bytes = fs::read(&local)
                        .map_err(|error| format!("无法读取拉取的 BenchmarkData：{error}"))?;
                    let summary = summarize_android_benchmark_json(remote, &bytes)?;
                    let report = root.join("benchmark-summary.json");
                    fs::write(
                        &report,
                        serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| format!("无法保存 Benchmark 摘要：{error}"))?;
                    android_tasks::update_task(
                        &task_app,
                        &task_id_for_thread,
                        97,
                        "BenchmarkData 已解析，正在保存摘要...",
                    )?;
                    return Ok(report);
                }
                let progress = (12
                    + (started.elapsed().as_secs().min(25 * 60) * 76 / (25 * 60)) as u8)
                    .min(88);
                android_tasks::update_task(
                    &task_app,
                    &task_id_for_thread,
                    progress,
                    "正在运行 AndroidX Benchmark instrumentation...",
                )?;
                std::thread::sleep(Duration::from_millis(500));
            }
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_android_benchmark_result(
    app: AppHandle,
    task_id: String,
) -> Result<AndroidBenchmarkSummary, String> {
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind IN ('benchmark-import','benchmark-run') AND status='done' AND artifact_path IS NOT NULL", [task_id.trim()], |row| row.get(0)).map_err(|_| "未找到已完成的 Benchmark 任务。".to_string())?;
    let report =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?;
    let managed_root = fs::canonicalize(
        app.path()
            .app_data_dir()
            .map_err(|error| format!("无法定位任务目录：{error}"))?
            .join("android-tasks")
            .join("benchmark"),
    )
    .map_err(|error| format!("无法定位 Benchmark 任务目录：{error}"))?;
    if !report.starts_with(managed_root)
        || report.file_name().and_then(|name| name.to_str()) != Some("benchmark-summary.json")
        || fs::metadata(&report)
            .map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?
            .len()
            > 256 * 1024
    {
        return Err("Benchmark 摘要路径无效。".to_string());
    }
    serde_json::from_slice(
        &fs::read(report).map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?,
    )
    .map_err(|error| format!("无法解析 Benchmark 摘要：{error}"))
}

#[tauri::command]
pub fn start_crash_diagnostics(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: CrashDiagnosticsRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "crash-diagnostics",
        &serial,
        "正在准备 ANR 与崩溃诊断包...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            fs::create_dir_all(&destination)
                .map_err(|error| format!("无法创建诊断目录：{error}"))?;
            let capture = |name: &str, args: &[&str]| -> Result<(), String> {
                let mut command_args = vec!["-s", serial.as_str()];
                command_args.extend_from_slice(args);
                let output = adb_command_output(&command_args, Duration::from_secs(60))
                    .map_err(|error| format!("无法采集 {name}：{error}"))?;
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    name,
                    "adb",
                    &command_args,
                    &output,
                )?;
                if !output.status.success() {
                    return Err(format!("采集 {name} 失败，退出码：{}", output.status));
                }
                fs::write(destination.join(name), &output.stdout)
                    .map_err(|error| format!("无法保存 {name}：{error}"))?;
                Ok(())
            };
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                12,
                "正在采集应用退出记录...",
            )?;
            capture(
                "exit-info.txt",
                &["shell", "dumpsys", "activity", "exit-info"],
            )?;
            if cancellation.load(Ordering::Relaxed) {
                return Ok(());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                32,
                "正在采集崩溃日志缓冲区...",
            )?;
            capture(
                "logcat-crash.txt",
                &["logcat", "-d", "-b", "crash", "-v", "threadtime"],
            )?;
            for (index, directory) in ["/data/anr", "/data/tombstones"].iter().enumerate() {
                if cancellation.load(Ordering::Relaxed) {
                    return Ok(());
                }
                android_tasks::update_task(
                    &task_app,
                    &task_id_for_thread,
                    50 + index as u8 * 20,
                    if index == 0 {
                        "正在导出 ANR 文件..."
                    } else {
                        "正在导出 Native tombstone..."
                    },
                )?;
                let list_args = ["-s", serial.as_str(), "shell", "ls", "-t", *directory];
                let listing = adb_command_output(&list_args, Duration::from_secs(30))
                    .map_err(|error| format!("无法读取 {directory}：{error}"))?;
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    &format!("列出 {directory}"),
                    "adb",
                    &list_args,
                    &listing,
                )?;
                if !listing.status.success() {
                    return Err(format!("无法读取 {directory}，退出码：{}", listing.status));
                }
                let names = String::from_utf8_lossy(&listing.stdout)
                    .lines()
                    .filter(|name| {
                        !name.contains('/') && !name.contains("..") && !name.trim().is_empty()
                    })
                    .take(3)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                for name in names {
                    let remote = format!("{directory}/{name}");
                    let local = destination
                        .join(if index == 0 { "anr" } else { "tombstones" })
                        .join(&name);
                    fs::create_dir_all(local.parent().unwrap())
                        .map_err(|error| format!("无法创建诊断子目录：{error}"))?;
                    let local_argument = local.to_string_lossy().to_string();
                    let pull_args = [
                        "-s",
                        serial.as_str(),
                        "pull",
                        remote.as_str(),
                        &local_argument,
                    ];
                    let pulled = adb_command_output(&pull_args, Duration::from_secs(5 * 60))
                        .map_err(|error| format!("无法导出 {remote}：{error}"))?;
                    android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        &format!("导出 {remote}"),
                        "adb",
                        &pull_args,
                        &pulled,
                    )?;
                    if !pulled.status.success() {
                        return Err(format!("无法导出 {remote}，退出码：{}", pulled.status));
                    }
                }
            }
            fs::write(
                destination.join("metadata.txt"),
                format!("serial={serial}\ncreated_at={}\n", now_millis()),
            )
            .map_err(|error| format!("无法保存诊断元数据：{error}"))?;
            Ok(())
        })();
        let artifact = result.as_ref().ok().and_then(|_| destination.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}
