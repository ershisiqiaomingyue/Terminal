use super::*;

pub(super) struct RemoteSearchFilesTool(pub(super) SshToolContext);
pub(super) struct RemoteSearchCodeTool(pub(super) SshToolContext);

pub(super) fn remote_search_command(
    path: &str,
    query: &str,
    _code_only: bool,
    budget_seconds: u64,
) -> String {
    let exclusions = [
        ".git",
        ".repo",
        "out",
        "node_modules",
        ".gradle",
        ".cache",
        ".venv",
        "venv",
        "__pycache__",
        "target",
        "dist",
        "build",
    ];
    let prune = exclusions
        .iter()
        .map(|name| format!("-name {}", shell_escape(name)))
        .collect::<Vec<_>>()
        .join(" -o ");
    // No extension whitelist: source languages and extensionless text are searchable.
    let code_filter = "";
    let find = format!(
        "find {} -type d \\( {prune} \\) -prune -o",
        shell_escape(path)
    );
    let rg_excludes = exclusions
        .iter()
        .map(|name| format!("--glob {}", shell_escape(&format!("!**/{name}/**"))))
        .collect::<Vec<_>>()
        .join(" ");
    let rg_code = "";
    let name_pattern = format!(
        "*{}*",
        query.chars().fold(String::new(), |mut result, character| {
            if matches!(character, '*' | '?' | '[' | ']' | '\\') {
                result.push('\\');
            }
            result.push(character);
            result
        })
    );
    let fallback_grep = shell_escape("query=$1; shift; grep -IlF -- \"$query\" \"$@\"; code=$?; if [ \"$code\" -le 1 ]; then exit 0; else exit \"$code\"; fi");
    // The extra result proves truncation. The remote timeout bounds scans even
    // when a producer has no further matches after the reader closes its pipe.
    let scan = format!(
        "{{ {find} \\( -type d -o \\( -type f{code_filter} \\) \\) -ipath {name_pattern} -printf '%y\\t%p\\n' 2>/dev/null; code=$?; if [ \"$code\" -ne 0 ]; then printf '\\036android-tool-search-error\\tfind:%s\\n' \"$code\"; fi; {{ if command -v rg >/dev/null 2>&1; then rg --line-buffered --hidden --no-ignore -lF --max-filesize 2M {rg_code} {rg_excludes} -- {query} {path} 2>/dev/null; code=$?; if [ \"$code\" -gt 1 ]; then printf '\\036android-tool-search-error\\trg:%s\\n' \"$code\"; fi; else {find} -type f -size -2049k{code_filter} -exec sh -c {fallback_grep} _ {query} {{}} + 2>/dev/null; code=$?; if [ \"$code\" -ne 0 ]; then printf '\\036android-tool-search-error\\tgrep:%s\\n' \"$code\"; fi; fi; }} | awk 'substr($0,1,1)==sprintf(\"%c\",30) {{print; fflush(); next}} {{printf \"f\\t%s\\n\",$0; fflush()}}'; }} | awk 'substr($0,1,1)==sprintf(\"%c\",30) {{print; fflush(); next}} !seen[substr($0,3)]++ {{ print; fflush(); if (++n >= {cap}) exit }}'",
        query = shell_escape(query), path = shell_escape(path), cap = MAX_REMOTE_SEARCH_RESULTS + 1,
        name_pattern = shell_escape(&name_pattern),
    );
    format!(
        "if command -v timeout >/dev/null 2>&1; then timeout --signal=TERM --kill-after=2s {budget_seconds}s sh -c {scan}; search_status=$?; elif command -v setsid >/dev/null 2>&1; then sh -c {scan}; search_status=$?; else printf '\\036android-tool-search-error\\tmissing-timeout-and-setsid\\n'; search_status=2; fi; printf '\\036android-tool-search-status\\t%s\\n' \"$search_status\"",
        scan = shell_escape(&scan),
    )
}

pub(super) fn remote_search_result(
    path: &str,
    query: &str,
    stdout: &str,
    backend: &str,
    output_truncated: bool,
) -> Value {
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    let mut status = None;
    let mut scan_errors = Vec::new();
    let mut truncated = output_truncated;
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix(REMOTE_SEARCH_STATUS_PREFIX) {
            status = value.parse::<i32>().ok();
            continue;
        }
        if let Some(value) = line.strip_prefix(REMOTE_SEARCH_ERROR_PREFIX) {
            scan_errors.push(value.to_string());
            continue;
        }
        let (kind, path) = match line.split_once('\t') {
            Some(("d", path)) => ("directory", path),
            Some(("f", path)) => ("file", path),
            _ => ("unknown", line),
        };
        if path.is_empty() || !seen.insert(path.to_string()) {
            continue;
        }
        if results.len() >= MAX_REMOTE_SEARCH_RESULTS {
            truncated = true;
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or_default();
        results.push(
            json!({"path": path, "name": name, "kind": kind, "matchType": "name_or_content"}),
        );
    }
    let timed_out = matches!(status, Some(124 | 137));
    let partial = timed_out
        || truncated
        || !scan_errors.is_empty()
        || status.is_none()
        || status.is_some_and(|code| code != 0);
    json!({
        "path": path, "query": query, "results": results, "truncated": truncated,
        "partial": partial, "timedOut": timed_out,
        "scanErrors": scan_errors,
        "searchBudgetSeconds": REMOTE_SEARCH_BUDGET_SECONDS,
        "status": if partial { "partial" } else { "completed" },
        "nextStep": if !scan_errors.is_empty() {
            Some("部分目录或文件扫描失败，请检查权限和搜索器能力后重试；不能据此判定无匹配。")
        } else if partial {
            Some("请在返回的候选目录内继续搜索，或缩小 path 范围；当前结果不代表完整源码树。")
        } else { None },
        "target": "ssh", "backend": backend,
    })
}

pub(super) async fn remote_search(
    context: SshToolContext,
    input: Value,
    code_only: bool,
) -> Result<Value, String> {
    let path = remote_path(&context, &input)?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if query.trim().is_empty() {
        return Err("搜索需要 query".to_string());
    }
    let sink = remote_output_sink_for_input(&context, &input);
    let operation_path = path.clone();
    let operation_query = query.clone();
    let operation_sink = sink.clone();
    if uses_openssh_fallback(&context.config) {
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": operation_path,
                "message": format!("开始搜索：{operation_query}"),
            }));
        }
        let command = remote_search_command(&path, &query, code_only, REMOTE_SEARCH_BUDGET_SECONDS);
        let output = run_openssh_context_command_checked(
            &context,
            &command,
            None,
            Duration::from_secs(REMOTE_SEARCH_BUDGET_SECONDS + 6),
            sink.as_ref(),
            &[operation_path.as_str()],
            "搜索远程目录",
        )?;
        if !output.status.success() {
            return Err(format!(
                "搜索远程目录失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let value = remote_search_result(
            &operation_path,
            &operation_query,
            &String::from_utf8_lossy(&output.stdout),
            "openssh",
            false,
        );
        let results = value["results"].as_array().expect("search result array");
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": operation_path,
                "completed": results.len(),
                "total": results.len(),
                "message": format!("{}，找到 {} 个结果", if value["partial"] == true { "搜索仅取得部分结果，请查看范围或扫描错误" } else { "搜索完成" }, results.len()),
            }));
        }
        finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
        return Ok(value);
    }
    // Searching a large Android checkout through SFTP means opening and
    // downloading files one at a time. Keep the SFTP boundary check, then
    // run the read-only search on the remote host so matches stream back as
    // they are found.
    if !uses_openssh_fallback(&context.config) {
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": operation_path,
                "message": format!("开始搜索：{operation_query}"),
            }));
        }
        let command = remote_search_command(&path, &query, code_only, REMOTE_SEARCH_BUDGET_SECONDS);
        let command_path = operation_path.clone();
        let command_sink = operation_sink.clone();
        let output = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                ensure_remote_path_safe(
                    &sftp,
                    &command_path,
                    &context.root,
                    &context.cancellation,
                    "搜索远程目录",
                )?;
                let channel = retry_nonblocking_with_cancel(
                    "打开 SSH 搜索通道",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.channel_session(),
                )?;
                let (_, wrapped_command) = remote_command_scripts(
                    &command_path,
                    &format!("sh -c {}", shell_escape(&command)),
                );
                let process_id = Arc::new(Mutex::new(None));
                let result = execute_remote_command(
                    channel,
                    &session,
                    &wrapped_command,
                    context,
                    command_sink.as_ref(),
                    Duration::from_secs(REMOTE_SEARCH_BUDGET_SECONDS + 6),
                    Some(process_id),
                )?;
                finish_remote_sink(command_sink.as_ref(), &result);
                Ok(result)
            }))
            .await,
        )?;
        if !output
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(format!(
                "搜索远程目录失败：{}",
                output
                    .get("stderr")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
            ));
        }
        let value = remote_search_result(
            &operation_path,
            &operation_query,
            output
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "ssh2-command",
            output
                .get("outputTruncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        let results = value["results"].as_array().expect("search result array");
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": operation_path,
                "completed": results.len(),
                "total": results.len(),
                "message": format!("{}，找到 {} 个结果", if value["partial"] == true { "搜索仅取得部分结果，请查看范围或扫描错误" } else { "搜索完成" }, results.len()),
            }));
        }
        return Ok(value);
    }
    let result = finish_remote_operation(
        sink.as_ref(),
        await_remote(spawn_remote(context, move |session, context| {
        let sftp = retry_nonblocking_with_cancel(
            "初始化远程 SFTP",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            &context.cancellation,
            || session.sftp(),
        )?;
        ensure_remote_path_safe(
            &sftp,
            &operation_path,
            &context.root,
            &context.cancellation,
            "搜索远程目录",
        )?;
        let mut results = Vec::new();
        let mut progress_events = 0usize;
        let mut visited_directories = 0usize;
        fn visit(
            sftp: &ssh2::Sftp,
            current: &str,
            query: &str,
            code_only: bool,
            results: &mut Vec<Value>,
            sink: Option<&CommandOutputSink>,
            progress_events: &mut usize,
            visited_directories: &mut usize,
            cancellation: &Arc<AtomicBool>,
            is_root: bool,
        ) -> Result<(), String> {
            cancelled(cancellation)?;
            if results.len() >= MAX_REMOTE_SEARCH_RESULTS { return Ok(()); }
            *visited_directories = visited_directories.saturating_add(1);
            if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                sink.emit_remote_progress(json!({
                    "stage": "scanning",
                    "path": current,
                    "itemPath": current,
                    "completed": *visited_directories,
                    "message": format!("正在检查目录：{current}"),
                }));
                *progress_events += 1;
            }
            let entries = match retry_nonblocking_with_cancel(
                "读取远程搜索目录",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                cancellation,
                || sftp.readdir(Path::new(current)),
            ) {
                Ok(entries) => entries,
                Err(error) => {
                    if is_root {
                        return Err(error);
                    }
                    if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                        sink.emit_remote_progress(json!({
                            "stage": "skipped",
                            "path": current,
                            "itemPath": current,
                            "message": format!("跳过无法读取的目录：{current}（{error}）"),
                        }));
                        *progress_events += 1;
                    }
                    return Ok(());
                }
            };
            for (entry, stat) in entries {
                cancelled(cancellation)?;
                if results.len() >= MAX_REMOTE_SEARCH_RESULTS { break; }
                let child = remote_directory_entry_path(current, &entry);
                let name = entry.file_name().and_then(|value| value.to_str()).unwrap_or_default();
                if matches!(name, ".git" | ".repo" | "out" | "node_modules" | ".cache" | ".venv" | "venv" | "__pycache__" | "target" | "dist" | "build" | ".gradle") { continue; }
                if matches!(stat.file_type(), ssh2::FileType::Symlink) {
                    continue;
                }
                let is_directory = matches!(stat.file_type(), ssh2::FileType::Directory);
                if is_directory {
                    if let Some(match_type) = search_match_kind(query, name, &child, None) {
                        results.push(json!({
                            "path": child,
                            "name": name,
                            "kind": "directory",
                            "matchType": match_type,
                        }));
                        if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                            sink.emit_remote_progress(json!({
                                "stage": "found",
                                "path": current,
                                "item": name,
                                "itemPath": child,
                                "kind": "directory",
                                "matchType": match_type,
                                "completed": results.len(),
                                "message": format!("找到匹配目录：{child}"),
                            }));
                            *progress_events += 1;
                        }
                    }
                    visit(
                        sftp,
                        &child,
                        query,
                        code_only,
                        results,
                        sink,
                        progress_events,
                        visited_directories,
                        cancellation,
                        false,
                    )?;
                    continue;
                }
                if !matches!(stat.file_type(), ssh2::FileType::RegularFile) {
                    continue;
                }
                let mut match_type = search_match_kind(query, name, &child, None);
                if match_type.is_none() && stat.size.unwrap_or(0) <= MAX_REMOTE_SEARCH_FILE_BYTES as u64 {
                    let mut file = match retry_nonblocking_with_cancel(
                        "打开远程搜索文件",
                        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                        cancellation,
                        || sftp.open(Path::new(&child)),
                    ) {
                        Ok(file) => file,
                        Err(error) => {
                            if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                                sink.emit_remote_progress(json!({
                                    "stage": "skipped",
                                    "path": current,
                                    "itemPath": child,
                                    "message": format!("跳过无法读取的文件：{child}（{error}）"),
                                }));
                                *progress_events += 1;
                            }
                            continue;
                        }
                    };
                    let bytes = match read_remote_bytes(
                        &mut file,
                        MAX_REMOTE_SEARCH_FILE_BYTES,
                        cancellation,
                    ) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                        if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                            sink.emit_remote_progress(json!({
                                "stage": "skipped",
                                "path": current,
                                "itemPath": child,
                                "message": format!("跳过无法读取的文件：{child}（{error}）"),
                            }));
                            *progress_events += 1;
                        }
                        continue;
                        }
                    };
                    match_type = search_match_kind(query, name, &child, Some(&String::from_utf8_lossy(&bytes)));
                }
                if let Some(match_type) = match_type {
                    results.push(json!({ "path": child, "name": name, "kind": "file", "matchType": match_type }));
                    if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
                        sink.emit_remote_progress(json!({
                            "stage": "found",
                            "path": current,
                            "item": name,
                            "itemPath": child,
                            "kind": "file",
                            "matchType": match_type,
                            "completed": results.len(),
                            "message": format!("找到匹配文件：{child}"),
                        }));
                        *progress_events += 1;
                    }
                }
            }
            Ok(())
        }
        if let Some(sink) = &operation_sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": operation_path,
                "message": format!("开始搜索：{operation_query}"),
            }));
            progress_events += 1;
        }
        visit(
            &sftp,
            &operation_path,
            &operation_query,
            code_only,
            &mut results,
            operation_sink.as_ref(),
            &mut progress_events,
            &mut visited_directories,
            &context.cancellation,
            true,
        )?;
        if let Some(sink) = &operation_sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": operation_path,
                "completed": results.len(),
                "total": results.len(),
                "message": format!("搜索完成，找到 {} 个结果", results.len()),
            }));
        }
        Ok(results)
        }))
        .await,
    )?;
    let truncated = result.len() >= MAX_REMOTE_SEARCH_RESULTS;
    Ok(
        json!({ "path": path, "query": query, "results": result, "truncated": truncated, "target": "ssh" }),
    )
}

#[async_trait]
impl ToolHandler for RemoteSearchFilesTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("search_local_files", "从当前 SSH 工作区递归搜索目录名、文件名、路径片段和文件内容；忽略 .git/.repo/out/node_modules 等生成目录，不跟随符号链接。最多 256 项，单次远端搜索预算 20 秒；partial 为 true 时结果不完整，请在候选目录缩小 path 后继续。未拿到工具返回的精确路径前省略 path。", json!({ "type": "object", "properties": { "path": { "type": "string" }, "query": { "type": "string" } }, "required": ["query"] })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        remote_search(self.0.clone(), input, false).await
    }
}

#[async_trait]
impl ToolHandler for RemoteSearchCodeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("search_local_code", "从当前 SSH 源码工作区（不限语言或扩展名）递归搜索目录名、代码文件名、路径片段和代码内容；忽略 .git/.repo/out/node_modules 等生成目录，不跟随符号链接。最多 256 项，单次远端搜索预算 20 秒；partial 为 true 时结果不完整，请在候选目录缩小 path 后继续。未拿到工具返回的精确路径前省略 path。", json!({ "type": "object", "properties": { "path": { "type": "string" }, "query": { "type": "string" } }, "required": ["query"] })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        remote_search(self.0.clone(), input, true).await
    }
}
