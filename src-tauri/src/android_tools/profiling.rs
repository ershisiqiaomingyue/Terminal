use super::*;

/// ART trace files consist of a binary event stream followed by a small UTF-8
/// key section. This only reads that key section and deliberately does not
/// infer timings or call stacks from the binary stream.
pub fn summarize_method_trace(path: &Path) -> Result<MethodTraceSummary, String> {
    const MAX_METHOD_TRACE_BYTES: u64 = 512 * 1024 * 1024;
    let size = fs::metadata(path)
        .map_err(|error| format!("无法检查方法 Trace：{error}"))?
        .len();
    if size > MAX_METHOD_TRACE_BYTES {
        return Err("方法 Trace 超过 512 MB，无法安全读取键区。".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("无法读取方法 Trace：{error}"))?;
    let Some(key_offset) = bytes
        .windows(b"*version".len())
        .rposition(|window| window == b"*version")
    else {
        return Err(
            "ART 方法 Trace 未包含可读的键区。请使用 Android Studio 打开原始 Trace。".to_string(),
        );
    };
    let key = std::str::from_utf8(&bytes[key_offset..])
        .map_err(|_| "ART 方法 Trace 键区不是 UTF-8。".to_string())?;
    let mut version = None;
    let mut section = "";
    let mut thread_count = 0usize;
    let mut method_count = 0usize;
    let mut classes = std::collections::BTreeSet::new();
    let mut data_file_overflow = None;
    for (index, line) in key.lines().enumerate() {
        let line = line.trim();
        if line == "*threads" {
            section = "threads";
            continue;
        }
        if line == "*methods" {
            section = "methods";
            continue;
        }
        if line == "*end" {
            break;
        }
        if line.starts_with('*') {
            section = "";
            continue;
        }
        if index == 1 && !line.is_empty() {
            version = Some(line.to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("data-file-overflow=") {
            data_file_overflow = match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            continue;
        }
        match section {
            "threads" if !line.is_empty() => thread_count += 1,
            "methods" if !line.is_empty() => {
                let mut fields = line.split_whitespace();
                let _id = fields.next();
                let _flags = fields.next();
                if let Some(class_name) = fields.next() {
                    classes.insert(class_name.to_string());
                }
                method_count += 1;
            }
            _ => {}
        }
    }
    Ok(MethodTraceSummary {
        version,
        thread_count,
        method_count,
        class_count: classes.len(),
        data_file_overflow,
        limitation: "此处只读取 ART Trace 键区的线程和方法表，不解析二进制事件，不能据此判断方法耗时、调用次数或热点。请使用 Android Studio 打开原始 Trace 做完整分析。".to_string(),
    })
}

#[tauri::command]
pub fn read_method_profile_report(
    app: AppHandle,
    task_id: String,
) -> Result<MethodProfileReport, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("请提供方法采样任务 ID。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='method-profile' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 ART 方法采样任务。".to_string())?;
    let root =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取方法采样产物：{error}"))?;
    let trace = root.join("method.trace");
    let metadata = root.join("metadata.txt");
    let trace_size = fs::metadata(&trace)
        .map_err(|error| format!("无法读取方法 Trace：{error}"))?
        .len();
    let metadata =
        fs::read_to_string(metadata).map_err(|error| format!("无法读取方法采样元数据：{error}"))?;
    let summary = summarize_method_trace(&trace)?;
    Ok(MethodProfileReport {
        task_id: task_id.to_string(),
        trace_path: trace.to_string_lossy().to_string(),
        trace_size,
        metadata,
        summary,
    })
}

#[tauri::command]
pub fn start_heap_dump(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: HeapDumpRequest,
) -> Result<String, String> {
    let serial = checked_serial(&request.serial)?.to_string();
    let package_name = checked_package(&request.package_name)?.to_string();
    let destination = PathBuf::from(request.destination.trim());
    if request.destination.trim().is_empty() {
        return Err("请选择堆转储产物目录。".to_string());
    }
    fs::create_dir_all(&destination).map_err(|error| format!("无法创建堆转储目录：{error}"))?;
    let manager = state.inner().clone();
    let target = format!("{serial}:{package_name}");
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "heap-dump",
        &target,
        "正在准备 Java 堆转储...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let adb = find_adb()?;
            let converter = find_hprof_converter().ok_or_else(|| {
                "未找到 SDK hprof-conv。请安装 Android SDK Platform-Tools。".to_string()
            })?;
            let remote = format!("/data/local/tmp/android-tool-heap-{task_id_for_thread}.hprof");
            let root = destination.join(format!("heap-dump-{task_id_for_thread}"));
            fs::create_dir_all(&root)
                .map_err(|error| format!("无法创建堆转储任务目录：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在请求设备导出 Java 堆...",
            )?;
            let dump_args = [
                "-s",
                serial.as_str(),
                "shell",
                "am",
                "dumpheap",
                package_name.as_str(),
                remote.as_str(),
            ];
            let mut dump_command = Command::new(&adb);
            dump_command.args(dump_args);
            let mut dump_child = CapturedCommandOutput::spawn(dump_command)
                .map_err(|error| error.replacen("命令", "Java 堆转储", 1))?;
            let dump_started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    dump_child.terminate();
                    let _ = adb_command_output(
                        &["-s", &serial, "shell", "rm", "-f", &remote],
                        Duration::from_secs(30),
                    );
                    return Err("请求已取消".to_string());
                }
                if dump_started.elapsed() >= Duration::from_secs(10 * 60) {
                    dump_child.terminate();
                    let _ = adb_command_output(
                        &["-s", &serial, "shell", "rm", "-f", &remote],
                        Duration::from_secs(30),
                    );
                    return Err("Java 堆转储超过 10 分钟未完成。".to_string());
                }
                if let Some(status) = dump_child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Java 堆转储：{error}"))?
                {
                    let dumped = dump_child
                        .finish(status)
                        .map_err(|error| format!("无法读取 Java 堆转储结果：{error}"))?;
                    android_tasks::record_command_audit(
                        &task_app,
                        &task_id_for_thread,
                        "导出 Android HPROF",
                        "adb",
                        &dump_args,
                        &dumped,
                    )?;
                    if !dumped.status.success() {
                        let stderr = String::from_utf8_lossy(&dumped.stderr).trim().to_string();
                        let stdout = String::from_utf8_lossy(&dumped.stdout).trim().to_string();
                        let detail = if stderr.is_empty() { stdout } else { stderr };
                        return Err(format!(
                            "Java 堆转储失败：{}",
                            if detail.is_empty() {
                                format!("退出码：{}", dumped.status)
                            } else {
                                detail
                            }
                        ));
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            if cancellation.load(Ordering::Relaxed) {
                let _ = adb_command_output(
                    &["-s", &serial, "shell", "rm", "-f", &remote],
                    Duration::from_secs(30),
                );
                return Err("请求已取消".to_string());
            }
            let android_hprof = root.join("android.hprof");
            let android_argument = android_hprof.to_string_lossy().to_string();
            let pull_args = [
                "-s",
                serial.as_str(),
                "pull",
                remote.as_str(),
                android_argument.as_str(),
            ];
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                62,
                "正在拉取 Android HPROF...",
            )?;
            let pulled = adb_command_output(&pull_args, Duration::from_secs(10 * 60))
                .map_err(|error| format!("无法拉取 Android HPROF：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "拉取 Android HPROF",
                "adb",
                &pull_args,
                &pulled,
            )?;
            let _ = adb_command_output(
                &["-s", &serial, "shell", "rm", "-f", &remote],
                Duration::from_secs(30),
            );
            if !pulled.status.success() {
                let stderr = String::from_utf8_lossy(&pulled.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&pulled.stdout).trim().to_string();
                let detail = if stderr.is_empty() { stdout } else { stderr };
                return Err(format!(
                    "无法拉取 Android HPROF：{}",
                    if detail.is_empty() {
                        format!("退出码：{}", pulled.status)
                    } else {
                        detail
                    }
                ));
            }
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            let standard_hprof = root.join("standard.hprof");
            let input = android_hprof.to_string_lossy().to_string();
            let output = standard_hprof.to_string_lossy().to_string();
            let mut convert_args = Vec::new();
            if request.exclude_zygote {
                convert_args.push("-z".to_string());
            }
            convert_args.extend([input, output]);
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                82,
                "正在转换为标准 HPROF...",
            )?;
            let mut convert_command = Command::new(&converter);
            convert_command.args(&convert_args);
            let converted =
                command_output_with_timeout(convert_command, Duration::from_secs(10 * 60))
                    .map_err(|error| format!("无法转换 HPROF：{error}"))?;
            let convert_refs = convert_args.iter().map(String::as_str).collect::<Vec<_>>();
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "转换标准 HPROF",
                "hprof-conv",
                &convert_refs,
                &converted,
            )?;
            if !converted.status.success() {
                let stderr = String::from_utf8_lossy(&converted.stderr)
                    .trim()
                    .to_string();
                let stdout = String::from_utf8_lossy(&converted.stdout)
                    .trim()
                    .to_string();
                let detail = if stderr.is_empty() { stdout } else { stderr };
                return Err(format!(
                    "HPROF 转换失败：{}",
                    if detail.is_empty() {
                        format!("退出码：{}", converted.status)
                    } else {
                        detail
                    }
                ));
            }
            let android_size = fs::metadata(&android_hprof)
                .map_err(|error| format!("无法检查 Android HPROF：{error}"))?
                .len();
            let standard_size = fs::metadata(&standard_hprof)
                .map_err(|error| format!("无法检查标准 HPROF：{error}"))?
                .len();
            fs::write(root.join("metadata.txt"), format!("serial={serial}\npackage={package_name}\nexclude_zygote={}\nandroid_hprof_size={android_size}\nstandard_hprof_size={standard_size}\ncreated_at={}\n", request.exclude_zygote, now_millis())).map_err(|error| format!("无法保存堆转储元数据：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                96,
                "正在保存堆转储产物...",
            )?;
            Ok(root)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_heap_dump_report(app: AppHandle, task_id: String) -> Result<HeapDumpReport, String> {
    let task_id = task_id.trim();
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='heap-dump' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Java 堆转储任务。".to_string())?;
    let root =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取堆转储产物：{error}"))?;
    let android = root.join("android.hprof");
    let standard = root.join("standard.hprof");
    let metadata = fs::read_to_string(root.join("metadata.txt"))
        .map_err(|error| format!("无法读取堆转储元数据：{error}"))?;
    Ok(HeapDumpReport {
        task_id: task_id.to_string(),
        android_hprof_path: android.to_string_lossy().to_string(),
        standard_hprof_path: standard.to_string_lossy().to_string(),
        android_hprof_size: fs::metadata(&android)
            .map_err(|error| error.to_string())?
            .len(),
        standard_hprof_size: fs::metadata(&standard)
            .map_err(|error| error.to_string())?
            .len(),
        metadata,
    })
}

pub(super) fn read_hprof_u32(reader: &mut impl Read) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| format!("HPROF 记录不完整：{error}"))?;
    Ok(u32::from_be_bytes(bytes))
}

pub(super) fn read_hprof_u64(reader: &mut impl Read) -> Result<u64, String> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| format!("HPROF 记录不完整：{error}"))?;
    Ok(u64::from_be_bytes(bytes))
}

/// Reads only the HPROF container framing. This is deliberately not an object
/// graph parser: class/object records require a complete semantic decoder to
/// make any retained-size or leak claim safely.
pub fn summarize_hprof_container(path: &Path) -> Result<HeapDumpContainerSummary, String> {
    let file = File::open(path).map_err(|error| format!("无法打开标准 HPROF：{error}"))?;
    let mut reader = BufReader::new(file);
    let mut header = Vec::with_capacity(32);
    loop {
        if header.len() >= 256 {
            return Err("HPROF 文件头超过允许长度。".to_string());
        }
        let mut byte = [0u8; 1];
        reader
            .read_exact(&mut byte)
            .map_err(|error| format!("无法读取 HPROF 文件头：{error}"))?;
        if byte[0] == 0 {
            break;
        }
        header.push(byte[0]);
    }
    let format =
        String::from_utf8(header).map_err(|_| "HPROF 文件头不是 UTF-8 格式标识。".to_string())?;
    if !format.starts_with("JAVA PROFILE") {
        return Err(format!("不是受支持的标准 HPROF：{format}"));
    }
    let identifier_size = read_hprof_u32(&mut reader)?;
    if !(1..=16).contains(&identifier_size) {
        return Err(format!("HPROF 标识符宽度异常：{identifier_size}"));
    }
    let timestamp_millis = read_hprof_u64(&mut reader)?;
    let mut record_count = 0u64;
    let mut heap_segment_count = 0u64;
    let mut heap_segment_bytes = 0u64;
    let mut record_types = BTreeMap::new();
    loop {
        let mut tag = [0u8; 1];
        match reader.read(&mut tag) {
            Ok(0) => break,
            Ok(1) => {}
            Ok(_) => return Err("无法读取完整 HPROF 记录类型。".to_string()),
            Err(error) => return Err(format!("无法读取 HPROF 记录：{error}")),
        }
        let _time_offset = read_hprof_u32(&mut reader)?;
        let length = read_hprof_u32(&mut reader)? as u64;
        record_count += 1;
        let label = match tag[0] {
            0x01 => "UTF8 字符串",
            0x02 => "加载类",
            0x04 => "调用栈帧",
            0x05 => "调用栈追踪",
            0x0c => "Heap Dump",
            0x1c => "Heap Dump 段",
            0x2c => "Heap Dump 结束",
            _ => "其他记录",
        };
        *record_types.entry(label.to_string()).or_insert(0) += 1;
        if matches!(tag[0], 0x0c | 0x1c) {
            heap_segment_count += 1;
            heap_segment_bytes = heap_segment_bytes.saturating_add(length);
        }
        let mut remaining = length;
        let mut buffer = [0u8; 8192];
        while remaining > 0 {
            let amount = remaining.min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..amount])
                .map_err(|error| format!("HPROF 记录长度与文件内容不匹配：{error}"))?;
            remaining -= amount as u64;
        }
    }
    Ok(HeapDumpContainerSummary {
        format,
        identifier_size,
        timestamp_millis,
        record_count,
        heap_segment_count,
        heap_segment_bytes,
        record_types,
        limitation: "当前摘要仅验证 HPROF 容器和记录分布，未解析对象图、支配树或泄漏路径；请使用 Shark、Android Studio 或 MAT 做对象级分析。".to_string(),
    })
}

#[tauri::command]
pub fn analyze_heap_dump(app: AppHandle, task_id: String) -> Result<HeapDumpAnalysis, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("请提供堆转储任务 ID。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='heap-dump' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Java 堆转储任务。".to_string())?;
    let root =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取堆转储产物：{error}"))?;
    let standard = fs::canonicalize(root.join("standard.hprof"))
        .map_err(|error| format!("无法读取标准 HPROF：{error}"))?;
    if !standard.starts_with(&root) {
        return Err("标准 HPROF 不在受管任务目录中。".to_string());
    }
    let standard_hprof_size = fs::metadata(&standard)
        .map_err(|error| format!("无法检查标准 HPROF：{error}"))?
        .len();
    let summary = summarize_hprof_container(&standard)?;
    Ok(HeapDumpAnalysis {
        task_id: task_id.to_string(),
        standard_hprof_path: standard.to_string_lossy().to_string(),
        standard_hprof_size,
        summary,
    })
}

#[tauri::command]
pub fn start_heap_shark_analysis(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    task_id: String,
) -> Result<String, String> {
    let source_task_id = task_id.trim().to_string();
    if source_task_id.is_empty() {
        return Err("请提供已完成的 Java 堆转储任务 ID。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let (target, artifact): (String, String) = connection.query_row(
        "SELECT target,artifact_path FROM tasks WHERE id=?1 AND kind='heap-dump' AND status='done' AND artifact_path IS NOT NULL",
        [&source_task_id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).map_err(|_| "未找到已完成的 Java 堆转储任务。".to_string())?;
    let root =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取堆转储产物：{error}"))?;
    let standard = fs::canonicalize(root.join("standard.hprof"))
        .map_err(|error| format!("无法读取标准 HPROF：{error}"))?;
    if !standard.starts_with(&root) {
        return Err("标准 HPROF 不在受管任务目录中。".to_string());
    }
    if fs::metadata(&standard)
        .map_err(|error| format!("无法检查标准 HPROF：{error}"))?
        .len()
        > 1024 * 1024 * 1024
    {
        return Err("标准 HPROF 超过 1 GB，当前 Shark 分析上限为 1 GB。请使用 Android Studio 或 MAT 离线分析。".to_string());
    }
    let manager = state.inner().clone();
    let (analysis_task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "heap-shark",
        &target,
        "正在准备 Shark 对象图分析...",
    )?;
    let task_app = app.clone();
    let analysis_id_for_thread = analysis_task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let (java, helper, dependencies) = find_shark_runtime(&task_app)?;
            let separator = if cfg!(target_os = "windows") {
                ";"
            } else {
                ":"
            };
            let mut classpath = vec![helper.to_string_lossy().to_string()];
            classpath.extend(
                dependencies
                    .iter()
                    .map(|path| path.to_string_lossy().to_string()),
            );
            let report = root.join("shark-analysis.json");
            fs::write(root.join("shark-source-task.txt"), source_task_id)
                .map_err(|error| format!("无法保存 Shark 来源任务：{error}"))?;
            let standard_argument = standard.to_string_lossy().to_string();
            android_tasks::update_task(
                &task_app,
                &analysis_id_for_thread,
                8,
                "正在启动 Shark 对象图分析...",
            )?;
            let classpath_argument = classpath.join(separator);
            let mut command = Command::new(&java);
            command.args([
                "-Xmx1536m",
                "-cp",
                &classpath_argument,
                "AndroidToolSharkKt",
                &standard_argument,
            ]);
            let mut child = CapturedCommandOutput::spawn(command)
                .map_err(|error| format!("无法启动 Shark 分析：{error}"))?;
            let started = std::time::Instant::now();
            loop {
                if cancellation.load(Ordering::Relaxed) {
                    child.terminate();
                    return Err("请求已取消".to_string());
                }
                if started.elapsed() >= Duration::from_secs(15 * 60) {
                    child.terminate();
                    return Err("Shark 对象图分析超过 15 分钟未完成。".to_string());
                }
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| format!("无法等待 Shark 分析：{error}"))?
                {
                    let output = child
                        .finish(status)
                        .map_err(|error| format!("无法读取 Shark 分析结果：{error}"))?;
                    android_tasks::record_command_audit(
                        &task_app,
                        &analysis_id_for_thread,
                        "Shark 对象图分析",
                        &java.to_string_lossy(),
                        &[
                            "-Xmx1536m",
                            "-cp",
                            "[Shark classpath]",
                            "AndroidToolSharkKt",
                            &standard_argument,
                        ],
                        &output,
                    )?;
                    if !output.status.success() {
                        return Err(format!(
                            "Shark 分析失败：{}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        ));
                    }
                    let text = String::from_utf8(output.stdout)
                        .map_err(|_| "Shark 分析返回了非 UTF-8 数据。".to_string())?;
                    let parsed: HeapSharkAnalysis = serde_json::from_str(text.trim())
                        .map_err(|error| format!("无法解析 Shark 分析结果：{error}"))?;
                    if parsed.status != "ok" {
                        return Err("Shark 未完成对象图分析。".to_string());
                    }
                    fs::write(
                        &report,
                        serde_json::to_vec_pretty(&parsed)
                            .map_err(|error| format!("无法序列化 Shark 分析结果：{error}"))?,
                    )
                    .map_err(|error| format!("无法保存 Shark 分析结果：{error}"))?;
                    android_tasks::update_task(
                        &task_app,
                        &analysis_id_for_thread,
                        96,
                        "Shark 对象图分析完成，正在保存摘要...",
                    )?;
                    return Ok(report);
                }
                let progress = (10
                    + (started.elapsed().as_secs().min(12 * 60) * 78 / (12 * 60)) as u8)
                    .min(88);
                android_tasks::update_task(
                    &task_app,
                    &analysis_id_for_thread,
                    progress,
                    "正在扫描 HPROF 对象图...",
                )?;
                std::thread::sleep(Duration::from_millis(500));
            }
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &analysis_id_for_thread,
            artifact,
            error,
        );
    });
    Ok(analysis_task_id)
}

#[tauri::command]
pub fn read_heap_shark_analysis(
    app: AppHandle,
    task_id: String,
) -> Result<HeapSharkAnalysis, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("请提供 Shark 分析任务 ID。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='heap-shark' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Shark 对象图分析任务。".to_string())?;
    let report =
        fs::canonicalize(artifact).map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?;
    if report.file_name().and_then(|name| name.to_str()) != Some("shark-analysis.json")
        || fs::metadata(&report)
            .map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?
            .len()
            > 256 * 1024
    {
        return Err("Shark 分析产物无效。".to_string());
    }
    serde_json::from_slice(
        &fs::read(report).map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?,
    )
    .map_err(|error| format!("无法解析 Shark 分析产物：{error}"))
}
