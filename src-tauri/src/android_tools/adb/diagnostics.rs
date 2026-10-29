use super::*;

pub async fn run_adb_diagnostic(serial: String, report: String) -> Result<String, String> {
    let serial = checked_serial(&serial)?.to_string();
    let command = match report.as_str() {
        "battery" => vec!["dumpsys", "battery"],
        "battery-stats" => vec!["dumpsys", "batterystats", "--charged"],
        "thermal" => vec!["dumpsys", "thermalservice"],
        "memory" => vec!["dumpsys", "meminfo"],
        "cpu" => vec!["dumpsys", "cpuinfo"],
        "activities" => vec!["dumpsys", "activity", "activities"],
        "network" => vec!["dumpsys", "netstats"],
        "graphics" => vec!["dumpsys", "gfxinfo"],
        "surfaceflinger" => vec!["dumpsys", "SurfaceFlinger"],
        _ => return Err("不支持的诊断报告。".to_string()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut args = vec!["-s", serial.as_str(), "shell"];
        args.extend(command);
        adb_output(&args)
    })
    .await
    .map_err(|error| format!("诊断任务失败：{error}"))?
}

#[tauri::command]
pub async fn get_adb_diagnostic(
    app: AppHandle,
    serial: String,
    report: String,
) -> Result<String, String> {
    let audit_target = checked_serial(&serial)?.to_string();
    let audit_report = report.clone();
    let result = run_adb_diagnostic(serial, report).await;
    let detail = result
        .as_ref()
        .map(|output| format!("dumpsys {audit_report} 完成，输出 {} 字节", output.len()))
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        &format!("读取设备诊断：{audit_report}"),
        &audit_target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

/// Extracts only documented, human-readable fields that recur across Android
/// versions. Missing fields are intentionally omitted instead of fabricated.
pub fn summarize_adb_diagnostic(report: &str, output: &str) -> AdbDiagnosticSummary {
    let wanted: &[&str] = match report {
        "battery" => &[
            "AC powered",
            "USB powered",
            "Wireless powered",
            "status",
            "health",
            "present",
            "level",
            "scale",
            "voltage",
            "temperature",
            "technology",
        ],
        "memory" => &[
            "Total RAM",
            "Free RAM",
            "Used RAM",
            "Lost RAM",
            "ZRAM",
            "Swap",
        ],
        "graphics" => &[
            "Janky frames",
            "50th percentile",
            "90th percentile",
            "95th percentile",
            "99th percentile",
            "Number Missed Vsync",
            "Number High input latency",
            "Number Slow UI thread",
            "Number Slow bitmap uploads",
            "Number Slow issue draw commands",
        ],
        "surfaceflinger" => &[
            "HWC missed frame count",
            "GPU missed frame count",
            "RenderEngine supports protected context",
            "RenderEngine is in protected context",
            "vulkan_renderengine",
        ],
        _ => &[],
    };
    let mut metrics = BTreeMap::new();
    for line in output.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if !value.is_empty()
            && wanted
                .iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
        {
            metrics
                .entry(key.to_string())
                .or_insert_with(|| value.to_string());
        }
    }
    AdbDiagnosticSummary {
        report: report.to_string(),
        metrics,
        raw_size: output.len(),
    }
}

#[tauri::command]
pub async fn get_adb_diagnostic_summary(
    app: AppHandle,
    serial: String,
    report: String,
) -> Result<AdbDiagnosticSummary, String> {
    let output = get_adb_diagnostic(app, serial, report.clone()).await?;
    Ok(summarize_adb_diagnostic(&report, &output))
}

pub async fn probe_android_performance_support_matrix(
    serial: String,
) -> Result<AndroidPerformanceSupportMatrix, String> {
    let serial = checked_serial(&serial)?.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let adb = find_adb()?;
        let checks = [
            ("Perfetto", vec!["shell", "command", "-v", "perfetto"]),
            ("Simpleperf", vec!["shell", "command", "-v", "simpleperf"]),
            ("Atrace", vec!["shell", "command", "-v", "atrace"]),
            ("Root adbd", vec!["shell", "id"]),
            ("渲染诊断", vec!["shell", "dumpsys", "gfxinfo"]),
            (
                "SurfaceFlinger 合成诊断",
                vec!["shell", "dumpsys", "SurfaceFlinger"],
            ),
        ];
        let mut capabilities = Vec::with_capacity(checks.len());
        for (name, command) in checks {
            let mut args = vec!["-s", serial.as_str()];
            args.extend(command);
            let mut process = Command::new(&adb);
            process.args(&args);
            let output =
                command_output_with_timeout_limit(process, Duration::from_secs(30), 64 * 1024)
                    .map_err(|error| format!("无法检查 {name}：{error}"))?;
            // A capability probe only needs enough evidence to explain availability.
            // Keep the potentially large device-wide gfxinfo response out of IPC.
            let text = bounded_performance_support_output(
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            );
            let available = if name == "Root adbd" {
                output.status.success() && text.contains("uid=0")
            } else {
                output.status.success() && !text.trim().is_empty()
            };
            capabilities.push(AndroidPerformanceCapability {
                name: name.to_string(),
                available,
                detail: text,
            });
        }
        Ok(AndroidPerformanceSupportMatrix {
            serial,
            capabilities,
        })
    })
    .await
    .map_err(|error| format!("性能支持探测任务失败：{error}"))?
}

#[tauri::command]
pub async fn get_android_performance_support_matrix(
    app: AppHandle,
    serial: String,
) -> Result<AndroidPerformanceSupportMatrix, String> {
    let audit_target = checked_serial(&serial)?.to_string();
    let result = probe_android_performance_support_matrix(serial).await;
    let detail = result
        .as_ref()
        .map(|matrix| {
            matrix
                .capabilities
                .iter()
                .map(|item| {
                    format!(
                        "{}={}",
                        item.name,
                        if item.available {
                            "可用"
                        } else {
                            "不可用"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        "读取性能支持矩阵",
        &audit_target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

/// Returns the renderer counters for one application when a package is given,
/// or the device-wide renderer report when it is empty. The package is typed
/// and validated rather than being interpolated into an unrestricted shell.
#[tauri::command]
pub async fn get_adb_graphics_diagnostic(
    app: AppHandle,
    request: AdbGraphicsDiagnosticRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let package_name = request.package_name.trim().to_string();
    if !package_name.is_empty() {
        checked_package(&package_name)?;
    }
    let audit_target = if package_name.is_empty() {
        serial.clone()
    } else {
        format!("{serial}:{package_name}")
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut args = vec!["-s", serial.as_str(), "shell", "dumpsys", "gfxinfo"];
        if !package_name.is_empty() {
            args.push(package_name.as_str());
        }
        adb_output(&args)
    })
    .await
    .map_err(|error| format!("渲染诊断任务失败：{error}"))?;
    let detail = result
        .as_ref()
        .map(|output| format!("gfxinfo 完成，输出 {} 字节", output.len()))
        .unwrap_or_else(|error| error.clone());
    let _ = android_tasks::record_operation_audit(
        &app,
        "读取渲染诊断",
        &audit_target,
        if result.is_ok() { "done" } else { "error" },
        &detail,
    );
    result
}

/// Persists a bounded point-in-time device health snapshot. Individual
/// services vary by Android version, so unsupported reports are retained with
/// their command failure instead of discarding the rest of the snapshot.
#[tauri::command]
pub fn start_device_health_capture(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: DeviceHealthCaptureRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    if request.destination.trim().is_empty() {
        return Err("请选择设备健康快照保存目录。".to_string());
    }
    fs::create_dir_all(&destination)
        .map_err(|error| format!("无法创建设备健康快照目录：{error}"))?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "device-health",
        &serial,
        "正在准备设备健康快照...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let artifact = destination.join(format!("device-health-{task_id_for_thread}"));
            fs::create_dir_all(&artifact)
                .map_err(|error| format!("无法创建设备健康快照产物目录：{error}"))?;
            let adb = find_adb()?;
            let reports = [
                ("battery.txt", "电池", vec!["dumpsys", "battery"]),
                (
                    "batterystats.txt",
                    "耗电统计",
                    vec!["dumpsys", "batterystats", "--charged"],
                ),
                ("thermal.txt", "温度", vec!["dumpsys", "thermalservice"]),
                ("network.txt", "网络", vec!["dumpsys", "netstats"]),
                ("memory.txt", "内存", vec!["dumpsys", "meminfo"]),
                ("cpu.txt", "CPU", vec!["dumpsys", "cpuinfo"]),
                ("graphics.txt", "渲染", vec!["dumpsys", "gfxinfo"]),
            ];
            let mut completed = 0usize;
            for (index, (file_name, label, command)) in reports.iter().enumerate() {
                if cancellation.load(Ordering::Relaxed) {
                    return Err("请求已取消".to_string());
                }
                android_tasks::update_task(
                    &task_app,
                    &task_id_for_thread,
                    8 + (index as u8 * 14),
                    &format!("正在采集{label}信息..."),
                )?;
                let mut args = vec!["-s".to_string(), serial.clone(), "shell".to_string()];
                args.extend(command.iter().map(|part| (*part).to_string()));
                let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                let output = adb_command_output(&refs, Duration::from_secs(60))
                    .map_err(|error| format!("无法采集{label}信息：{error}"))?;
                android_tasks::record_command_audit(
                    &task_app,
                    &task_id_for_thread,
                    &format!("设备健康快照：{label}"),
                    &adb.to_string_lossy(),
                    &refs,
                    &output,
                )?;
                let body = format!(
                    "命令: adb {}\n退出码: {}\n\n标准输出:\n{}\n\n标准错误:\n{}\n",
                    refs.join(" "),
                    output
                        .status
                        .code()
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "信号终止".to_string()),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                fs::write(artifact.join(file_name), body)
                    .map_err(|error| format!("无法保存{label}快照：{error}"))?;
                if output.status.success() {
                    completed += 1;
                }
            }
            fs::write(
                artifact.join("metadata.txt"),
                format!(
                    "serial={serial}\ncreated_at={}\nsuccessful_reports={completed}/{}\n",
                    now_millis(),
                    reports.len()
                ),
            )
            .map_err(|error| format!("无法保存设备健康快照元数据：{error}"))?;
            if completed == 0 {
                return Err("设备未返回任何健康诊断报告，请检查连接与设备权限。".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                95,
                "设备健康快照已保存，正在完成任务...",
            )?;
            Ok(artifact)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}
