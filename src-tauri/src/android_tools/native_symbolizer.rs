use super::*;

pub(super) fn tombstone_frames(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let frame = fields.next()?;
            (frame.starts_with('#') && fields.next()? == "pc").then_some(())?;
            let address = fields.next()?.trim_start_matches("0x");
            let path = fields.next()?.trim_end_matches([')', ':']);
            let relative = path.trim_start_matches('/');
            (!address.is_empty()
                && address
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                && path.starts_with('/')
                && !relative
                    .split('/')
                    .any(|component| component.is_empty() || component == "." || component == ".."))
            .then(|| (address.to_string(), path.to_string()))
        })
        .collect()
}

pub(super) fn native_command_output(
    command: Command,
    cancellation: Option<&AtomicBool>,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<std::process::Output, String> {
    if cancellation.is_some_and(|signal| signal.load(Ordering::Relaxed)) {
        return Err("请求已取消".to_string());
    }
    let mut child = CapturedCommandOutput::spawn_with_output_limit(command, max_output_bytes)?;
    let started = std::time::Instant::now();
    loop {
        if cancellation.is_some_and(|signal| signal.load(Ordering::Relaxed)) {
            child.terminate();
            return Err("请求已取消".to_string());
        }
        if let Some(status) = child.try_wait()? {
            return child.finish(status);
        }
        if started.elapsed() >= timeout {
            child.terminate();
            return Err(format!("命令执行超时（{} 秒）。", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn record_native_command_audit(
    audit: Option<(&AppHandle, &str)>,
    action: &str,
    executable: &Path,
    args: &[String],
    output: &std::process::Output,
) -> Result<(), String> {
    if let Some((app, task_id)) = audit {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        android_tasks::record_command_audit(
            app,
            task_id,
            action,
            &executable.to_string_lossy(),
            &refs,
            output,
        )?;
    }
    Ok(())
}

pub(super) fn addr2line_fallback_with_context(
    symbols: &Path,
    tombstone: &str,
    cancellation: Option<&AtomicBool>,
    audit: Option<(&AppHandle, &str)>,
) -> Result<Vec<String>, String> {
    let Some(tool) = find_llvm_addr2line() else {
        return Ok(Vec::new());
    };
    let mut resolved = Vec::new();
    for (address, remote_path) in tombstone_frames(tombstone).into_iter().take(100) {
        if cancellation.is_some_and(|signal| signal.load(Ordering::Relaxed)) {
            return Err("请求已取消".to_string());
        }
        let relative = remote_path.trim_start_matches('/');
        let binary = symbols.join(relative);
        if !binary.is_file() {
            continue;
        }
        let mut command = Command::new(&tool);
        let args = vec![
            "-f".to_string(),
            "-C".to_string(),
            "-e".to_string(),
            binary.to_string_lossy().to_string(),
            format!("0x{address}"),
        ];
        command.args(&args);
        let output = native_command_output(
            command,
            cancellation,
            Duration::from_secs(10),
            MAX_ADDR2LINE_OUTPUT_BYTES,
        )
        .map_err(|error| format!("无法执行 llvm-addr2line：{error}"))?;
        record_native_command_audit(audit, "补充 Native 地址解析", &tool, &args, &output)?;
        if !output.status.success() {
            continue;
        }
        let rendered = String::from_utf8_lossy(&output.stdout);
        let mut lines = rendered.lines();
        let function = lines.next().unwrap_or("??").trim();
        let source = lines.next().unwrap_or("??:0").trim();
        if function != "??" && !function.is_empty() {
            resolved.push(format!(
                "{remote_path}+0x{address} -> {function} ({source})"
            ));
        }
    }
    Ok(resolved)
}

#[cfg(test)]
pub(super) fn addr2line_fallback(symbols: &Path, tombstone: &str) -> Result<Vec<String>, String> {
    addr2line_fallback_with_context(symbols, tombstone, None, None)
}

pub fn symbolize_native_tombstone(
    request: NativeSymbolizeRequest,
) -> Result<NativeSymbolizeResult, String> {
    symbolize_native_tombstone_with_context(request, None, None)
}

pub(super) fn symbolize_native_tombstone_with_context(
    request: NativeSymbolizeRequest,
    cancellation: Option<&AtomicBool>,
    audit: Option<(&AppHandle, &str)>,
) -> Result<NativeSymbolizeResult, String> {
    let tombstone = fs::canonicalize(request.tombstone_path.trim())
        .map_err(|error| format!("无法读取 tombstone：{error}"))?;
    let symbols = fs::canonicalize(request.symbols_path.trim())
        .map_err(|error| format!("无法读取符号目录：{error}"))?;
    if !tombstone.is_file() || !symbols.is_dir() {
        return Err("请选择 tombstone 文件和符号目录。".to_string());
    }
    let tombstone_size = fs::metadata(&tombstone)
        .map_err(|error| format!("无法读取 tombstone 属性：{error}"))?
        .len();
    if tombstone_size > MAX_NATIVE_TOMBSTONE_BYTES {
        return Err(format!(
            "tombstone 超过 {} MB，无法安全加载。",
            MAX_NATIVE_TOMBSTONE_BYTES / 1024 / 1024
        ));
    }
    let tool = find_ndk_stack().ok_or_else(|| {
        "未找到 NDK ndk-stack。请安装 Android NDK 或设置 ANDROID_TOOL_NDK_STACK。".to_string()
    })?;
    let mut command = Command::new(&tool);
    let args = vec![
        "-sym".to_string(),
        symbols.to_string_lossy().to_string(),
        "-i".to_string(),
        tombstone.to_string_lossy().to_string(),
    ];
    command.args(&args);
    let output = native_command_output(
        command,
        cancellation,
        Duration::from_secs(60),
        MAX_NATIVE_SYMBOLIZER_OUTPUT_BYTES,
    )
    .map_err(|error| format!("无法执行 ndk-stack：{error}"))?;
    record_native_command_audit(audit, "执行 ndk-stack 符号化", &tool, &args, &output)?;
    let output = if output.status.success() {
        output
    } else {
        let mut fallback = Command::new(&tool);
        let fallback_args = vec![
            "-sym".to_string(),
            symbols.to_string_lossy().to_string(),
            "-dump".to_string(),
            tombstone.to_string_lossy().to_string(),
        ];
        fallback.args(&fallback_args);
        let output = native_command_output(
            fallback,
            cancellation,
            Duration::from_secs(60),
            MAX_NATIVE_SYMBOLIZER_OUTPUT_BYTES,
        )
        .map_err(|error| format!("无法执行 ndk-stack：{error}"))?;
        record_native_command_audit(
            audit,
            "执行 ndk-stack 兼容模式",
            &tool,
            &fallback_args,
            &output,
        )?;
        output
    };
    if !output.status.success() {
        return Err(format!(
            "Native 符号化失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut rendered = String::from_utf8_lossy(&output.stdout).to_string();
    let ndk_stack_symbol_count = rendered
        .lines()
        .filter(|line| line.contains(" #") && !line.contains("???") && !line.contains("<unknown>"))
        .count();
    let tombstone_text =
        fs::read_to_string(&tombstone).map_err(|error| format!("无法读取 tombstone：{error}"))?;
    let fallback = addr2line_fallback_with_context(&symbols, &tombstone_text, cancellation, audit)?;
    let addr2line_symbol_count = fallback.len();
    if addr2line_symbol_count > 0 {
        rendered.push_str("\n地址补充解析（llvm-addr2line）：\n");
        rendered.push_str(&fallback.join("\n"));
        rendered.push('\n');
    }
    if ndk_stack_symbol_count == 0 && addr2line_symbol_count == 0 {
        return Err("未在所选符号目录中找到可解析的 Native 帧。请确认 tombstone 的 ABI、Build ID 和未剥离符号目录来自同一次构建。".to_string());
    }
    Ok(NativeSymbolizeResult {
        output: rendered,
        ndk_stack_symbol_count,
        addr2line_symbol_count,
    })
}

pub(super) fn native_symbolization_task_root(
    app: &AppHandle,
    task_id: &str,
) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("android-tasks")
        .join("native-symbolizer")
        .join(task_id);
    fs::create_dir_all(&root)
        .map_err(|error| format!("无法创建 Native 符号化任务目录：{error}"))?;
    Ok(root)
}

#[tauri::command]
pub fn start_native_tombstone_symbolization(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: NativeSymbolizeRequest,
) -> Result<String, String> {
    let tombstone = fs::canonicalize(request.tombstone_path.trim())
        .map_err(|error| format!("无法读取 tombstone：{error}"))?;
    let symbols = fs::canonicalize(request.symbols_path.trim())
        .map_err(|error| format!("无法读取符号目录：{error}"))?;
    if !tombstone.is_file() || !symbols.is_dir() {
        return Err("请选择 tombstone 文件和符号目录。".to_string());
    }
    let manager = state.inner().clone();
    let target = tombstone.to_string_lossy().to_string();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "native-symbolizer",
        &target,
        "正在准备 Native 符号化...",
    )?;
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "symbolization-plan",
        &format!(
            "tombstone={}\nsymbols={}",
            tombstone.display(),
            symbols.display()
        ),
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            let output_dir = native_symbolization_task_root(&task_app, &task_id_for_thread)?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                12,
                "正在运行 ndk-stack 符号化...",
            )?;
            let report = symbolize_native_tombstone_with_context(
                NativeSymbolizeRequest {
                    tombstone_path: tombstone.to_string_lossy().to_string(),
                    symbols_path: symbols.to_string_lossy().to_string(),
                },
                Some(&cancellation),
                Some((&task_app, &task_id_for_thread)),
            )?;
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                88,
                "正在保存符号化结果...",
            )?;
            let output_path = output_dir.join("symbolized.txt");
            fs::write(&output_path, &report.output)
                .map_err(|error| format!("无法保存符号化输出：{error}"))?;
            fs::write(
                output_dir.join("metadata.txt"),
                format!(
                    "tombstone={}\nsymbols={}\nndk_stack_symbol_count={}\naddr2line_symbol_count={}\n",
                    tombstone.display(), symbols.display(), report.ndk_stack_symbol_count, report.addr2line_symbol_count,
                ),
            ).map_err(|error| format!("无法保存符号化元数据：{error}"))?;
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "symbolization-result",
                &format!(
                    "ndk_stack_symbol_count={}\naddr2line_symbol_count={}\noutput={}",
                    report.ndk_stack_symbol_count,
                    report.addr2line_symbol_count,
                    output_path.display(),
                ),
            )?;
            Ok(output_dir)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_native_symbolization_report(
    app: AppHandle,
    task_id: String,
) -> Result<NativeSymbolizeResult, String> {
    let task_id = task_id.trim();
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row(
        "SELECT artifact_path FROM tasks WHERE id=?1 AND kind='native-symbolizer' AND status='done' AND artifact_path IS NOT NULL",
        params![task_id],
        |row| row.get(0),
    ).map_err(|_| "未找到已完成的 Native 符号化任务。".to_string())?;
    let artifact = fs::canonicalize(artifact)
        .map_err(|error| format!("无法读取 Native 符号化产物：{error}"))?;
    let expected = fs::canonicalize(native_symbolization_task_root(&app, task_id)?)
        .map_err(|error| format!("无法定位 Native 符号化任务目录：{error}"))?;
    if artifact != expected {
        return Err("Native 符号化任务产物路径无效。".to_string());
    }
    let output = fs::read_to_string(artifact.join("symbolized.txt"))
        .map_err(|error| format!("无法读取符号化输出：{error}"))?;
    let metadata = fs::read_to_string(artifact.join("metadata.txt"))
        .map_err(|error| format!("无法读取符号化元数据：{error}"))?;
    let count = |key: &str| {
        metadata
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or_default()
    };
    Ok(NativeSymbolizeResult {
        output,
        ndk_stack_symbol_count: count("ndk_stack_symbol_count="),
        addr2line_symbol_count: count("addr2line_symbol_count="),
    })
}
