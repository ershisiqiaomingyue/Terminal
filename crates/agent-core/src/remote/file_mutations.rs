use super::*;

pub(super) struct RemoteReadFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("read_local_file", "读取 SSH 工作区中的文件预览。", json!({ "type": "object", "properties": { "path": { "type": "string" }, "max_bytes": { "type": "integer" } }, "required": ["path"] })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": path,
                "itemPath": path,
                "message": "正在读取远程文件",
            }));
        }
        let max_bytes = input
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_REMOTE_FILE_BYTES as u64)
            .min(MAX_REMOTE_FILE_BYTES as u64) as usize;
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ -L {1} ]; then exit 2; fi; head -c {0} {1} 2>/dev/null | base64 | tr -d '\\n'",
                max_bytes,
                shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "读取远程文件",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "读取远程文件失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let encoded = String::from_utf8_lossy(&output.stdout)
                .chars()
                .filter(|value| !value.is_whitespace())
                .collect::<String>();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|error| format!("远程文件解码失败：{error}"))?;
            let mut value = remote_text(&bytes);
            if let Value::Object(object) = &mut value {
                object.insert("path".to_string(), json!(path));
                object.insert("truncated".to_string(), json!(bytes.len() >= max_bytes));
                object.insert("backend".to_string(), json!("openssh"));
            }
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        // SFTP reads on a non-blocking libssh2 channel can remain in EAGAIN
        // for the full operation timeout even when the same SSH connection
        // can execute commands normally. Read the bounded payload through an
        // SSH command instead. The shell-side realpath check preserves the
        // workspace boundary and rejects symlink escapes before head runs.
        let command_path = path.clone();
        let command_sink = sink.clone();
        let command = format!(
            "root_real=$(realpath -e -- {root} 2>/dev/null) || {{ printf '无法解析 SSH 工作目录\\n' >&2; exit 20; }}; target_real=$(realpath -e -- {path} 2>/dev/null) || {{ printf '无法解析远程文件\\n' >&2; exit 21; }}; case \"$target_real\" in \"$root_real\"|\"$root_real\"/*) ;; *) printf '远程文件越出工作目录\\n' >&2; exit 22 ;; esac; if [ -L {path} ] || [ ! -f {path} ]; then printf '目标不是普通文件\\n' >&2; exit 23; fi; head -c {max_bytes} {path} 2>/dev/null | base64 2>/dev/null | tr -d '\\n'",
            root = shell_escape(&context.root),
            path = shell_escape(&command_path),
            max_bytes = max_bytes,
        );
        let command_timeout =
            Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120));
        let output = match await_remote(spawn_remote(context, move |session, context| {
            let channel = retry_nonblocking_with_cancel(
                "打开 SSH 文件读取通道",
                command_timeout,
                &context.cancellation,
                || session.channel_session(),
            )?;
            execute_remote_command(
                channel,
                &session,
                &command,
                context,
                command_sink.as_ref(),
                command_timeout,
                None,
            )
        }))
        .await
        {
            Ok(output) => output,
            Err(error) => {
                finish_remote_operation(sink.as_ref(), Err(error.clone()))?;
                return Err(error);
            }
        };
        if !output
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let error = format!(
                "读取远程文件失败：{}",
                output
                    .get("stderr")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
            );
            finish_remote_operation(sink.as_ref(), Err(error.clone()))?;
            return Err(error);
        }
        let encoded = output
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .filter(|value| !value.is_whitespace())
            .collect::<String>();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|error| format!("远程文件解码失败：{error}"))?;
        let mut result = remote_text(&bytes);
        if let Value::Object(object) = &mut result {
            object.insert("path".to_string(), json!(path));
            object.insert("truncated".to_string(), json!(bytes.len() >= max_bytes));
            object.insert("backend".to_string(), json!("ssh2-command"));
        }
        finish_remote_operation(sink.as_ref(), Ok(result.clone()))?;
        if let Some(sink) = &sink {
            let encoding = result
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("未知编码");
            let content_length = result
                .get("content")
                .and_then(Value::as_str)
                .map(|value| value.chars().count())
                .unwrap_or(0);
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": path,
                "itemPath": path,
                "kind": encoding,
                "completed": content_length,
                "message": format!("文件读取完成，编码 {encoding}，内容约 {content_length} 字符"),
            }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteWriteFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteWriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("write_local_file", "写入 SSH 工作区中的文本文件。", json!({ "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] })).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "write_local_file 需要 content".to_string())?
            .to_string();
        if content.len() > 256 * 1024 {
            return Err("单次写入不能超过 256 KB".to_string());
        }
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": path,
                "itemPath": path,
                "message": "正在准备写入远程文件",
            }));
        }
        if uses_openssh_fallback(&context.config) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
            let command = format!(
                "if [ -L {0} ]; then exit 2; fi; (base64 -d 2>/dev/null || base64 -D 2>/dev/null) > {0}",
                shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                Some(encoded.as_bytes()),
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "写入远程文件",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "写入远程文件失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let value = json!({ "path": path, "bytes": content.len(), "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let result_path = path.clone();
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
                    &result_path,
                    &context.root,
                    &context.cancellation,
                    "写入远程文件",
                )?;
                let mut file = retry_nonblocking_with_cancel(
                    "创建远程文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.create(Path::new(&result_path)),
                )?;
                write_remote_bytes(&mut file, content.as_bytes(), &context.cancellation)?;
                Ok(json!({ "path": result_path, "bytes": content.len(), "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": path,
                "itemPath": path,
                "completed": result.get("bytes").and_then(Value::as_u64).unwrap_or_default(),
                "message": format!("远程文件写入完成，共 {} 字节", result.get("bytes").and_then(Value::as_u64).unwrap_or_default()),
            }));
        }
        Ok(result)
    }
}

pub(super) fn remote_mutation_path(
    context: &SshToolContext,
    input: &Value,
    key: &str,
) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| remote_path(context, &json!({ "path": value })))
        .transpose()?
        .ok_or_else(|| format!("{key} 需要路径"))
}

pub(super) fn remote_parent_path(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
}

pub(super) fn remote_sftp_lstat(
    sftp: &ssh2::Sftp,
    path: &str,
    cancellation: &Arc<AtomicBool>,
    operation: &str,
) -> Result<ssh2::FileStat, String> {
    retry_nonblocking_with_cancel(
        operation,
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.lstat(Path::new(path)),
    )
}

pub(super) fn remote_sftp_lstat_if_exists(
    sftp: &ssh2::Sftp,
    path: &str,
    cancellation: &Arc<AtomicBool>,
    operation: &str,
) -> Result<Option<ssh2::FileStat>, String> {
    match remote_sftp_lstat(sftp, path, cancellation, operation) {
        Ok(stat) => Ok(Some(stat)),
        Err(error) if is_remote_not_found_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn is_remote_not_found_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such") || error.contains("not found")
}

pub(super) fn remote_sftp_parent_is_directory(
    sftp: &ssh2::Sftp,
    path: &str,
    cancellation: &Arc<AtomicBool>,
) -> Result<(), String> {
    let parent = remote_sftp_lstat(
        sftp,
        remote_parent_path(path),
        cancellation,
        "读取远程目标父目录",
    )
    .map_err(|error| format!("无法读取远程目标父目录：{error}"))?;
    if !matches!(parent.file_type(), ssh2::FileType::Directory) {
        return Err("远程目标父路径不是目录".to_string());
    }
    Ok(())
}

pub(super) fn remote_copy_file(
    sftp: &ssh2::Sftp,
    source: &str,
    destination: &str,
    overwrite: bool,
    cancellation: &Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
) -> Result<u64, String> {
    let mut source_file = retry_nonblocking_with_cancel(
        "打开远程复制源文件",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.open(Path::new(source)),
    )?;
    let mut destination_file = retry_nonblocking_with_cancel(
        "创建远程复制目标文件",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || {
            if overwrite {
                sftp.create(Path::new(destination))
            } else {
                sftp.open_mode(
                    Path::new(destination),
                    ssh2::OpenFlags::WRITE | ssh2::OpenFlags::EXCLUSIVE,
                    0o644,
                    ssh2::OpenType::File,
                )
            }
        },
    )?;
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        cancelled(cancellation)?;
        let length = retry_io_with_cancel(
            "读取远程复制源文件",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            cancellation,
            || source_file.read(&mut buffer),
        )?;
        if length == 0 {
            break;
        }
        total = total.saturating_add(length as u64);
        if total > MAX_REMOTE_TRANSFER_BYTES {
            return Err(format!(
                "远程复制文件超过 {} MB 限制",
                MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
            ));
        }
        write_remote_bytes(&mut destination_file, &buffer[..length], cancellation)?;
        if let Some(sink) = sink {
            sink.emit_remote_progress(json!({
                "stage": "progress",
                "completed": total,
                "message": format!("已复制 {} 字节", total),
            }));
        }
    }
    Ok(total)
}

pub(super) fn remote_copy_directory(
    sftp: &ssh2::Sftp,
    source: &str,
    destination: &str,
    overwrite: bool,
    cancellation: &Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
) -> Result<(usize, u64), String> {
    if remote_path_is_same_or_descendant(&destination, &source) {
        return Err("copy_directory 不允许把目录复制到自身或其子目录中".to_string());
    }
    let source_stat = remote_sftp_lstat(sftp, source, cancellation, "读取远程复制源目录")
        .map_err(|error| format!("无法读取远程复制源目录：{error}"))?;
    if !matches!(source_stat.file_type(), ssh2::FileType::Directory) {
        return Err("copy_directory 的 source 必须是目录".to_string());
    }
    match remote_sftp_lstat_if_exists(sftp, destination, cancellation, "检查远程复制目标目录")?
    {
        Some(stat) if !matches!(stat.file_type(), ssh2::FileType::Directory) => {
            return Err("远程复制目标已存在且不是目录".to_string());
        }
        Some(_) if !overwrite => {
            return Err("远程目标目录已存在；如需合并覆盖请明确传入 overwrite=true".to_string());
        }
        Some(_) => {}
        None => {
            remote_sftp_parent_is_directory(sftp, destination, cancellation)?;
            retry_nonblocking_with_cancel(
                "创建远程复制目标目录",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                cancellation,
                || sftp.mkdir(Path::new(destination), 0o755),
            )?;
        }
    }

    fn copy_directory_contents(
        sftp: &ssh2::Sftp,
        source: &str,
        destination: &str,
        overwrite: bool,
        cancellation: &Arc<AtomicBool>,
        sink: Option<&CommandOutputSink>,
        entry_count: &mut usize,
        total_bytes: &mut u64,
    ) -> Result<(), String> {
        let mut entries = retry_nonblocking_with_cancel(
            "读取远程复制源目录",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            cancellation,
            || sftp.readdir(Path::new(source)),
        )?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (entry, stat) in entries {
            cancelled(cancellation)?;
            *entry_count = entry_count.saturating_add(1);
            if *entry_count > MAX_REMOTE_DIRECTORY_ENTRIES {
                return Err(format!(
                    "远程目录复制超过 {} 项限制",
                    MAX_REMOTE_DIRECTORY_ENTRIES
                ));
            }
            let name = entry
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty() && *value != "." && *value != "..")
                .ok_or_else(|| "远程目录包含无法处理的条目名称".to_string())?;
            let source_path = format!("{}/{}", source.trim_end_matches('/'), name);
            let destination_path = format!("{}/{}", destination.trim_end_matches('/'), name);
            match stat.file_type() {
                ssh2::FileType::Directory => {
                    match remote_sftp_lstat_if_exists(
                        sftp,
                        &destination_path,
                        cancellation,
                        "检查远程复制子目录",
                    )? {
                        Some(destination_stat)
                            if !matches!(
                                destination_stat.file_type(),
                                ssh2::FileType::Directory
                            ) =>
                        {
                            return Err(format!("远程复制目标不是目录：{destination_path}"));
                        }
                        Some(_) if !overwrite => {
                            return Err(format!(
                                "远程复制目标目录已存在：{destination_path}；如需合并覆盖请传 overwrite=true"
                            ));
                        }
                        Some(_) => {}
                        None => {
                            remote_sftp_parent_is_directory(sftp, &destination_path, cancellation)?;
                            retry_nonblocking_with_cancel(
                                "创建远程复制子目录",
                                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                                cancellation,
                                || sftp.mkdir(Path::new(&destination_path), 0o755),
                            )?;
                        }
                    }
                    copy_directory_contents(
                        sftp,
                        &source_path,
                        &destination_path,
                        overwrite,
                        cancellation,
                        sink,
                        entry_count,
                        total_bytes,
                    )?;
                }
                ssh2::FileType::RegularFile => {
                    let size = stat.size.unwrap_or(0);
                    if total_bytes.saturating_add(size) > MAX_REMOTE_TRANSFER_BYTES {
                        return Err(format!(
                            "远程目录复制超过 {} MB 限制",
                            MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
                        ));
                    }
                    remote_sftp_parent_is_directory(sftp, &destination_path, cancellation)?;
                    let copied = remote_copy_file(
                        sftp,
                        &source_path,
                        &destination_path,
                        overwrite,
                        cancellation,
                        None,
                    )?;
                    *total_bytes = total_bytes.saturating_add(copied);
                    if let Some(sink) = sink {
                        sink.emit_remote_progress(json!({
                            "stage": "item",
                            "itemPath": destination_path,
                            "completed": *total_bytes,
                            "message": format!("已复制远程文件：{source_path}"),
                        }));
                    }
                }
                ssh2::FileType::Symlink => {
                    return Err(format!(
                        "copy_directory 不支持符号链接，为避免越界已停止：{source_path}"
                    ));
                }
                _ => {
                    return Err(format!("copy_directory 不支持远程特殊文件：{source_path}"));
                }
            }
        }
        Ok(())
    }

    let mut entry_count = 0usize;
    let mut total_bytes = 0u64;
    copy_directory_contents(
        sftp,
        source,
        destination,
        overwrite,
        cancellation,
        sink,
        &mut entry_count,
        &mut total_bytes,
    )?;
    Ok((entry_count, total_bytes))
}

pub(super) struct RemoteCreateDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteCreateDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "create_directory",
            "在 SSH 工作区内创建一个目录，不隐式创建多级父目录。",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_mutation_path(&context, &input, "path")?;
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ -d {0} ]; then printf already\\n; elif [ -e {0} ] || [ -L {0} ]; then printf file\\n; elif mkdir {0}; then printf created\\n; else exit 1; fi",
                shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "创建远程目录",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "创建远程目录失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let value = json!({
                "path": path,
                "created": state == "created",
                "alreadyExists": state == "already",
                "target": "ssh",
                "backend": "openssh"
            });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_sink = sink.clone();
        let result_path = path.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                if let Some(stat) = remote_sftp_lstat_if_exists(
                    &sftp,
                    &result_path,
                    &context.cancellation,
                    "检查远程创建目录目标",
                )? {
                    if matches!(stat.file_type(), ssh2::FileType::Directory) {
                        return Ok(json!({ "path": result_path, "created": false, "alreadyExists": true, "target": "ssh" }));
                    }
                    return Err("远程目标路径已经是文件，不能创建目录".to_string());
                }
                remote_sftp_parent_is_directory(&sftp, &result_path, &context.cancellation)?;
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({ "stage": "scanning", "path": result_path, "message": "正在创建远程目录" }));
                }
                retry_nonblocking_with_cancel(
                    "创建远程目录",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.mkdir(Path::new(&result_path), 0o755),
                )?;
                Ok(json!({ "path": result_path, "created": true, "alreadyExists": false, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({ "stage": "completed", "path": path, "message": if result.get("created").and_then(Value::as_bool).unwrap_or(false) { "远程目录创建完成" } else { "远程目录已存在" } }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteCopyFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteCopyFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "copy_file",
            "在 SSH 工作区内复制一个明确指定的普通文件，不支持目录递归复制。",
            json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "destination": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["source", "destination"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let source = remote_mutation_path(&context, &input, "source")?;
        let destination = remote_mutation_path(&context, &input, "destination")?;
        if source == destination {
            return Err("复制源文件和目标文件不能相同".to_string());
        }
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ ! -f {source} ] || [ -L {source} ]; then exit 3; fi; if [ -e {destination} ] || [ -L {destination} ]; then {guard} else :; fi; test -d {parent} || exit 4; cp -p {source} {destination}",
                source = shell_escape(&source),
                destination = shell_escape(&destination),
                guard = if overwrite { ":" } else { "exit 2" },
                parent = shell_escape(remote_parent_path(&destination)),
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[source.as_str(), destination.as_str()],
                "复制远程文件",
            )?;
            if !output.status.success() {
                return Err(match output.status.code() {
                    Some(2) => "远程目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string(),
                    Some(3) => "复制源路径不是普通文件或是符号链接".to_string(),
                    _ => format!(
                        "复制远程文件失败：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            let value = json!({ "source": source, "destination": destination, "overwritten": overwrite, "moved": false, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_sink = sink.clone();
        let source_for_task = source.clone();
        let destination_for_task = destination.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let source_stat = remote_sftp_lstat(
                    &sftp,
                    &source_for_task,
                    &context.cancellation,
                    "读取远程复制源文件",
                )
                .map_err(|error| format!("无法读取远程复制源文件：{error}"))?;
                if !matches!(source_stat.file_type(), ssh2::FileType::RegularFile) {
                    return Err("copy_file 只支持普通文件，不支持目录".to_string());
                }
                if remote_sftp_lstat_if_exists(
                    &sftp,
                    &destination_for_task,
                    &context.cancellation,
                    "检查远程复制目标文件",
                )?
                .is_some()
                    && !overwrite
                {
                    return Err("远程目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string());
                }
                remote_sftp_parent_is_directory(&sftp, &destination_for_task, &context.cancellation)?;
                let bytes = remote_copy_file(&sftp, &source_for_task, &destination_for_task, overwrite, &context.cancellation, operation_sink.as_ref())?;
                Ok(json!({ "source": source_for_task, "destination": destination_for_task, "bytes": bytes, "overwritten": overwrite, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({ "stage": "completed", "path": destination, "message": format!("远程文件复制完成，共 {} 字节", result.get("bytes").and_then(Value::as_u64).unwrap_or_default()) }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteMoveFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteMoveFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "move_file",
            "在 SSH 工作区内移动一个明确指定的普通文件，不支持目录递归移动。",
            json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "destination": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["source", "destination"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let source = remote_mutation_path(&context, &input, "source")?;
        let destination = remote_mutation_path(&context, &input, "destination")?;
        if source == destination {
            return Err("移动源文件和目标文件不能相同".to_string());
        }
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ ! -f {source} ] || [ -L {source} ]; then exit 3; fi; if [ -e {destination} ] || [ -L {destination} ]; then {guard} else :; fi; test -d {parent} || exit 4; mv {source} {destination}",
                source = shell_escape(&source),
                destination = shell_escape(&destination),
                guard = if overwrite { ":" } else { "exit 2" },
                parent = shell_escape(remote_parent_path(&destination)),
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[source.as_str(), destination.as_str()],
                "移动远程文件",
            )?;
            if !output.status.success() {
                return Err(match output.status.code() {
                    Some(2) => "远程目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string(),
                    Some(3) => "移动源路径不是普通文件或是符号链接".to_string(),
                    _ => format!(
                        "移动远程文件失败：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            let value = json!({ "source": source, "destination": destination, "overwritten": overwrite, "moved": true, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let source_for_task = source.clone();
        let destination_for_task = destination.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let source_stat = remote_sftp_lstat(
                    &sftp,
                    &source_for_task,
                    &context.cancellation,
                    "读取远程移动源文件",
                )
                .map_err(|error| format!("无法读取远程移动源文件：{error}"))?;
                if !matches!(source_stat.file_type(), ssh2::FileType::RegularFile) {
                    return Err("move_file 只支持普通文件，不支持目录".to_string());
                }
                if remote_sftp_lstat_if_exists(
                    &sftp,
                    &destination_for_task,
                    &context.cancellation,
                    "检查远程移动目标文件",
                )?
                .is_some()
                    && !overwrite
                {
                    return Err("远程目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string());
                }
                remote_sftp_parent_is_directory(&sftp, &destination_for_task, &context.cancellation)?;
                retry_nonblocking_with_cancel(
                    "移动远程文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.rename(
                        Path::new(&source_for_task),
                        Path::new(&destination_for_task),
                        Some(if overwrite {
                            ssh2::RenameFlags::ATOMIC
                                | ssh2::RenameFlags::OVERWRITE
                                | ssh2::RenameFlags::NATIVE
                        } else {
                            ssh2::RenameFlags::ATOMIC | ssh2::RenameFlags::NATIVE
                        }),
                    ),
                )?;
                Ok(json!({ "source": source_for_task, "destination": destination_for_task, "moved": true, "overwritten": overwrite, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(
                json!({ "stage": "completed", "path": destination, "message": "远程文件移动完成" }),
            );
        }
        Ok(result)
    }
}

pub(super) struct RemoteDeleteFileTool(pub(super) SshToolContext);

pub(super) struct RemoteCopyDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteCopyDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "copy_directory",
            "递归复制 SSH 工作区内的一个目录。只复制普通文件和目录，不复制符号链接；最多 4096 项、128 MB。",
            json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "destination": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["source", "destination"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let source = remote_mutation_path(&context, &input, "source")?;
        let destination = remote_mutation_path(&context, &input, "destination")?;
        if source == destination || destination.starts_with(&format!("{source}/")) {
            return Err("copy_directory 不允许把目录复制到自身或其子目录中".to_string());
        }
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": source,
                "itemPath": destination,
                "message": "正在准备复制远程目录",
            }));
        }
        if uses_openssh_fallback(&context.config) {
            let result = run_openssh_context_command_checked(
                &context,
                &openssh_directory_copy_command(&source, &destination, overwrite),
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[source.as_str(), destination.as_str()],
                "复制远程目录",
            )
            .and_then(|output| parse_openssh_directory_result(&output, "复制远程目录"));
            let (entries, bytes) = finish_remote_operation(sink.as_ref(), result)?;
            let value = json!({
                "source": source,
                "destination": destination,
                "entries": entries,
                "bytes": bytes,
                "copied": true,
                "overwritten": overwrite,
                "target": "ssh",
                "backend": "openssh",
            });
            if let Some(sink) = &sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": destination,
                    "completed": entries,
                    "total": entries,
                    "message": format!("远程目录复制完成，共 {entries} 项"),
                }));
            }
            return Ok(value);
        }
        let source_for_task = source.clone();
        let destination_for_task = destination.clone();
        let operation_sink = sink.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程目录复制",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let (entries, bytes) = remote_copy_directory(
                    &sftp,
                    &source_for_task,
                    &destination_for_task,
                    overwrite,
                    &context.cancellation,
                    operation_sink.as_ref(),
                )?;
                Ok(json!({
                    "source": source_for_task,
                    "destination": destination_for_task,
                    "entries": entries,
                    "bytes": bytes,
                    "copied": true,
                    "overwritten": overwrite,
                    "target": "ssh",
                }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": destination,
                "message": format!("远程目录复制完成，共 {} 项", result.get("entries").and_then(Value::as_u64).unwrap_or_default()),
            }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteMoveDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteMoveDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "move_directory",
            "移动 SSH 工作区内的一个目录。只允许明确目录路径，不允许移动到自身或其子目录。",
            json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "destination": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["source", "destination"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let source = remote_mutation_path(&context, &input, "source")?;
        let destination = remote_mutation_path(&context, &input, "destination")?;
        if remote_path_is_same_or_descendant(&destination, &source) {
            return Err("move_directory 不允许把目录移动到自身或其子目录中".to_string());
        }
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": source,
                "itemPath": destination,
                "message": "正在准备移动远程目录",
            }));
        }
        if uses_openssh_fallback(&context.config) {
            let result = run_openssh_context_command_checked(
                &context,
                &openssh_directory_move_command(&source, &destination, overwrite),
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[source.as_str(), destination.as_str()],
                "移动远程目录",
            )
            .and_then(|output| parse_openssh_directory_result(&output, "移动远程目录"));
            let (entries, bytes) = finish_remote_operation(sink.as_ref(), result)?;
            let value = json!({
                "source": source,
                "destination": destination,
                "entries": entries,
                "bytes": bytes,
                "moved": true,
                "overwritten": overwrite,
                "target": "ssh",
                "backend": "openssh",
            });
            if let Some(sink) = &sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": destination,
                    "completed": entries,
                    "total": entries,
                    "message": format!("远程目录移动完成，共 {entries} 项"),
                }));
            }
            return Ok(value);
        }
        let source_for_task = source.clone();
        let destination_for_task = destination.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程目录移动",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let source_stat = remote_sftp_lstat(
                    &sftp,
                    &source_for_task,
                    &context.cancellation,
                    "读取远程移动源目录",
                )
                .map_err(|error| format!("无法读取远程移动源目录：{error}"))?;
                if !matches!(source_stat.file_type(), ssh2::FileType::Directory) {
                    return Err("move_directory 的 source 必须是目录".to_string());
                }
                if let Some(destination_stat) = remote_sftp_lstat_if_exists(
                    &sftp,
                    &destination_for_task,
                    &context.cancellation,
                    "检查远程移动目标目录",
                )? {
                    if !matches!(destination_stat.file_type(), ssh2::FileType::Directory) {
                        return Err("远程目录移动目标已存在且不是目录".to_string());
                    }
                    if !overwrite {
                        return Err(
                            "远程目标目录已存在；如需覆盖请明确传入 overwrite=true".to_string()
                        );
                    }
                }
                remote_sftp_parent_is_directory(
                    &sftp,
                    &destination_for_task,
                    &context.cancellation,
                )?;
                retry_nonblocking_with_cancel(
                    "移动远程目录",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || {
                        sftp.rename(
                            Path::new(&source_for_task),
                            Path::new(&destination_for_task),
                            Some(if overwrite {
                                ssh2::RenameFlags::ATOMIC
                                    | ssh2::RenameFlags::OVERWRITE
                                    | ssh2::RenameFlags::NATIVE
                            } else {
                                ssh2::RenameFlags::ATOMIC | ssh2::RenameFlags::NATIVE
                            }),
                        )
                    },
                )?;
                Ok(json!({
                    "source": source_for_task,
                    "destination": destination_for_task,
                    "moved": true,
                    "overwritten": overwrite,
                    "target": "ssh",
                }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": destination,
                "message": "远程目录移动完成",
            }));
        }
        Ok(result)
    }
}

#[async_trait]
impl ToolHandler for RemoteDeleteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "delete_file",
            "删除 SSH 工作区内明确指定的单个文件或符号链接，绝不删除目录或递归删除目录树。",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_mutation_path(&context, &input, "path")?;
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ -d {0} ]; then exit 2; elif [ -f {0} ] || [ -L {0} ]; then rm {0}; else exit 3; fi",
                shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[remote_parent_path(&path)],
                "删除远程文件",
            )?;
            if !output.status.success() {
                return Err(if output.status.code() == Some(2) {
                    "delete_file 不支持删除目录或递归删除".to_string()
                } else {
                    format!(
                        "删除远程文件失败：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )
                });
            }
            let value =
                json!({ "path": path, "deleted": true, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let result_path = path.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let stat = remote_sftp_lstat(
                    &sftp,
                    &result_path,
                    &context.cancellation,
                    "读取远程删除目标",
                )
                .map_err(|error| format!("无法读取远程删除目标：{error}"))?;
                if matches!(stat.file_type(), ssh2::FileType::Directory) {
                    return Err(
                        "delete_file 不支持删除目录或递归删除；请先明确提供目录管理需求"
                            .to_string(),
                    );
                }
                retry_nonblocking_with_cancel(
                    "删除远程文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.unlink(Path::new(&result_path)),
                )?;
                Ok(json!({ "path": result_path, "deleted": true, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(
                json!({ "stage": "completed", "path": path, "message": "远程文件删除完成" }),
            );
        }
        Ok(result)
    }
}
