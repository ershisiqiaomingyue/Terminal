use super::*;

pub(super) fn local_transfer_path(
    context: &SshToolContext,
    input: &Value,
    key: &str,
) -> Result<PathBuf, String> {
    let raw = input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} 需要本地路径"))?;
    if raw.starts_with("attachment://") || raw.starts_with("browser://") {
        return Err("文件只有浏览器内嵌内容，没有可用于上传的本地路径；请先保存到磁盘".to_string());
    }
    let path = resolve_selected_path(Path::new(raw), &context.local_transfer_roots);
    ensure_local_transfer_path(context, &path)?;
    Ok(path)
}

pub(super) fn ensure_local_transfer_path(
    context: &SshToolContext,
    path: &Path,
) -> Result<(), String> {
    if !is_allowed_agent_path(
        path,
        &context.local_transfer_roots,
        context.allow_external_paths,
    ) {
        return Err("本地传输路径必须是用户明确选择的文件或目录".to_string());
    }
    if let Some(parent) = path.parent() {
        if !is_allowed_agent_path(
            parent,
            &context.local_transfer_roots,
            context.allow_external_paths,
        ) {
            return Err("本地传输目标的父目录不在用户选择范围内".to_string());
        }
    }
    Ok(())
}

pub(super) fn collect_local_directory_files(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, PathBuf, u64)>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if entries.len() >= MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "目录内容超过 {} 项限制",
            MAX_REMOTE_DIRECTORY_ENTRIES
        ));
    }
    let metadata = fs::symlink_metadata(current)
        .map_err(|error| format!("无法读取本地目录项 {}：{error}", current.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("目录传输不跟随符号链接：{}", current.display()));
    }
    if metadata.is_file() {
        let relative = current
            .strip_prefix(root)
            .map_err(|_| "无法计算本地目录相对路径".to_string())?
            .to_path_buf();
        let size = metadata.len();
        *total_bytes = total_bytes.saturating_add(size);
        if *total_bytes > MAX_REMOTE_TRANSFER_BYTES {
            return Err(format!(
                "目录内容超过 {} MB 限制",
                MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
            ));
        }
        entries.push((current.to_path_buf(), relative, size));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!("不支持的本地目录项：{}", current.display()));
    }
    let mut children = fs::read_dir(current)
        .map_err(|error| format!("无法读取本地目录 {}：{error}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取本地目录项失败：{error}"))?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        collect_local_directory_files(root, &child.path(), entries, total_bytes)?;
    }
    Ok(())
}

pub(super) fn ensure_remote_directory_all(
    sftp: &ssh2::Sftp,
    path: &str,
    cancellation: &Arc<AtomicBool>,
) -> Result<(), String> {
    let normalized = normalize_remote_path(path);
    let mut current = String::from("/");
    for segment in normalized.trim_start_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        if current != "/" {
            current.push('/');
        }
        current.push_str(segment);
        if let Some(stat) =
            remote_sftp_lstat_if_exists(sftp, &current, cancellation, "检查远程目录")?
        {
            if !matches!(stat.file_type(), ssh2::FileType::Directory) {
                return Err(format!("远程路径不是目录：{current}"));
            }
        } else {
            retry_nonblocking_with_cancel(
                "创建远程目录",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                cancellation,
                || sftp.mkdir(Path::new(&current), 0o755),
            )?;
        }
    }
    Ok(())
}

pub(super) fn remote_relative_path(base: &str, relative: &Path) -> Result<String, String> {
    let relative = relative
        .to_str()
        .ok_or_else(|| "目录包含无法转换为 UTF-8 的文件名".to_string())?
        .replace('\\', "/");
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.split('/').any(|part| part == "..")
    {
        return Err("目录相对路径无效".to_string());
    }
    Ok(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        relative.trim_start_matches('/')
    ))
}

/// Streams a local file into a remote command's stdin. Unlike the SFTP path this
/// never waits for an SFTP window adjust between chunks, which is what made
/// uploads crawl on the libssh2 engine.
pub(super) fn stream_local_file_to_remote(
    session: &ssh2::Session,
    local_path: &Path,
    remote_path: &str,
    overwrite: bool,
    cancellation: &Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
    total: u64,
) -> Result<u64, String> {
    let mut source =
        fs::File::open(local_path).map_err(|error| format!("无法打开待上传文件：{error}"))?;
    if total > MAX_REMOTE_TRANSFER_BYTES {
        return Err(format!(
            "文件超过 {} MB 的上传上限",
            MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
        ));
    }
    let target = shell_escape(remote_path);
    let guard = if overwrite {
        String::new()
    } else {
        format!("test -e {target} && {{ printf '目标文件已存在\\n' >&2; exit 2; }}; ")
    };
    let command = format!(
        "{guard}parent=$(dirname -- {target}); test -d \"$parent\" || {{ printf '远程目录不存在\\n' >&2; exit 3; }}; exec cat > {target}"
    );
    // Blocking mode keeps libssh2 from returning EAGAIN on every window update.
    session.set_blocking(true);
    let mut written = 0u64;
    let result = (|| {
        let mut channel = session
            .channel_session()
            .map_err(|error| format!("打开上传通道失败：{error}"))?;
        channel
            .exec(&command)
            .map_err(|error| format!("启动远程写入命令失败：{error}"))?;
        let mut buffer = vec![0u8; REMOTE_SFTP_CHUNK_BYTES.max(64 * 1024)];
        let mut completed = 0u64;
        let mut reported = Instant::now();
        loop {
            cancelled(cancellation)?;
            let length = source
                .read(&mut buffer)
                .map_err(|error| format!("读取本地上传文件失败：{error}"))?;
            if length == 0 {
                break;
            }
            channel
                .write_all(&buffer[..length])
                .map_err(|error| format!("写入远程文件失败：{error}"))?;
            completed = completed.saturating_add(length as u64);
            written = completed;
            if let Some(sink) = sink.filter(|_| reported.elapsed() >= REMOTE_PROGRESS_INTERVAL) {
                sink.emit_remote_progress(json!({
                    "kind": "remote_transfer",
                    "direction": "upload",
                    "target": "ssh",
                    "message": format!("已上传 {completed} / {total} 字节"),
                }));
                reported = Instant::now();
            }
        }
        channel
            .send_eof()
            .map_err(|error| format!("结束上传通道失败：{error}"))?;
        let mut stderr = String::new();
        let _ = (&mut channel).take(8192).read_to_string(&mut String::new());
        channel
            .stderr()
            .take(8192)
            .read_to_string(&mut stderr)
            .map_err(|error| format!("读取上传错误输出失败：{error}"))?;
        channel
            .wait_close()
            .map_err(|error| format!("等待远程写入完成失败：{error}"))?;
        let code = channel
            .exit_status()
            .map_err(|error| format!("读取远程写入状态失败：{error}"))?;
        if code != 0 {
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                format!("远程写入失败（退出码 {code}）")
            } else {
                format!("远程写入失败：{detail}")
            });
        }
        if completed != total {
            return Err(format!("上传文件大小校验失败：{remote_path}"));
        }
        Ok(completed)
    })();
    session.set_blocking(false);
    // `cat > target` leaves a truncated file behind when the transfer fails, so
    // drop it instead of leaving a half-written file on the remote host. Only do
    // that when this call actually opened the file: with `overwrite=false` a
    // refusal ("目标文件已存在") must not delete the user's existing file.
    if let Err(error) = &result {
        if written > 0 {
            if let Err(cleanup) = retry_nonblocking_with_cancel(
                "清理未完成的上传文件",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                cancellation,
                || {
                    let mut channel = session.channel_session()?;
                    channel.exec(&format!("rm -f -- {target}"))?;
                    channel.wait_close()?;
                    Ok(())
                },
            ) {
                eprintln!("清理未完成的上传文件失败：{cleanup}");
            }
        }
        let _ = error;
    }
    result
}

pub(super) fn download_remote_file_to_local(
    sftp: &ssh2::Sftp,
    remote_path: &str,
    local_path: &Path,
    overwrite: bool,
    cancellation: &Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
    completed: &mut u64,
    total: u64,
) -> Result<u64, String> {
    let metadata = remote_sftp_lstat(sftp, remote_path, cancellation, "读取远程下载源文件")?;
    if !matches!(metadata.file_type(), ssh2::FileType::RegularFile) {
        return Err(format!("远程路径不是普通文件：{remote_path}"));
    }
    let mut source = retry_nonblocking_with_cancel(
        "打开远程下载源文件",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.open(Path::new(remote_path)),
    )?;
    let mut destination = if overwrite {
        fs::File::create(local_path).map_err(|error| format!("创建本地下载文件失败：{error}"))?
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(local_path)
            .map_err(|error| format!("创建本地下载文件失败：{error}"))?
    };
    let mut buffer = vec![0u8; REMOTE_SFTP_CHUNK_BYTES];
    let mut file_bytes = 0u64;
    let mut reported = Instant::now();
    loop {
        cancelled(cancellation)?;
        let length = retry_io_with_cancel(
            "读取远程下载源文件",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            cancellation,
            || source.read(&mut buffer),
        )?;
        if length == 0 {
            break;
        }
        file_bytes = file_bytes.saturating_add(length as u64);
        *completed = completed.saturating_add(length as u64);
        write_remote_bytes(&mut destination, &buffer[..length], cancellation)?;
        if let Some(sink) = sink.filter(|_| reported.elapsed() >= REMOTE_PROGRESS_INTERVAL) {
            sink.emit_remote_progress(json!({
                "stage": "progress",
                "path": remote_path,
                "completed": *completed,
                "total": total,
                "message": format!("已下载 {} / {} 字节", *completed, total),
            }));
            reported = Instant::now();
        }
    }
    if metadata.size.is_some_and(|size| size != file_bytes) {
        return Err(format!("下载文件大小校验失败：{remote_path}"));
    }
    Ok(file_bytes)
}

pub(super) struct RemoteUploadFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteUploadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "upload_file",
            "将用户明确选择的本地普通文件上传到 SSH 工作区。支持二进制文件，不支持目录上传。",
            json!({
                "type": "object",
                "properties": {
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["local_path", "remote_path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let local_path = local_transfer_path(&context, &input, "local_path")?;
        let remote_path = remote_mutation_path(&context, &input, "remote_path")?;
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let metadata =
            fs::metadata(&local_path).map_err(|error| format!("无法读取待上传文件：{error}"))?;
        if !metadata.is_file() {
            return Err("upload_file 只支持普通文件，不支持目录".to_string());
        }
        if metadata.len() > MAX_REMOTE_TRANSFER_BYTES {
            return Err(format!(
                "上传文件超过 {} MB 限制",
                MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
            ));
        }
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let target = shell_escape(&remote_path);
            let guard = if overwrite {
                String::new()
            } else {
                format!("test -e {target} && {{ printf '目标文件已存在\\n' >&2; exit 2; }}; ")
            };
            let command = format!(
                "{guard}parent=$(dirname -- {target}); test -d \"$parent\" || {{ printf '远程目录不存在\\n' >&2; exit 3; }}; exec cat > {target}"
            );
            let bytes = stream_file_to_openssh(
                &context,
                &command,
                &local_path,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                sink.as_ref(),
                metadata.len(),
            )?;
            if bytes != metadata.len() {
                return Err(format!("上传文件大小校验失败：{remote_path}"));
            }
            let value = json!({ "localPath": local_path.to_string_lossy(), "remotePath": remote_path, "bytes": bytes, "overwritten": overwrite, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_sink = sink.clone();
        let local_for_task = local_path.clone();
        let remote_for_task = remote_path.clone();
        let target_for_task = remote_path.clone();
        let total_bytes = metadata.len();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                stream_local_file_to_remote(
                    &session,
                    &local_for_task,
                    &target_for_task,
                    overwrite,
                    &context.cancellation,
                    operation_sink.as_ref(),
                    total_bytes,
                )?;
                Ok(json!({ "localPath": local_for_task.to_string_lossy(), "remotePath": remote_for_task, "bytes": total_bytes, "overwritten": overwrite, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({ "stage": "completed", "path": remote_path, "message": format!("文件上传完成，共 {} 字节", result.get("bytes").and_then(Value::as_u64).unwrap_or_default()) }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteDownloadFileTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteDownloadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "download_file",
            "将 SSH 工作区内明确指定的普通文件下载到用户明确选择的本地目录或文件路径。支持二进制文件。",
            json!({
                "type": "object",
                "properties": {
                    "remote_path": { "type": "string" },
                    "local_path": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["remote_path", "local_path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let remote_path = remote_mutation_path(&context, &input, "remote_path")?;
        let local_selection = local_transfer_path(&context, &input, "local_path")?;
        let local_path = if local_selection.is_dir() {
            let file_name = Path::new(&remote_path)
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "远程下载路径没有可用文件名".to_string())?;
            local_selection.join(file_name)
        } else {
            local_selection
        };
        ensure_local_transfer_path(&context, &local_path)?;
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if local_path.exists() && !overwrite {
            return Err("本地目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string());
        }
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "base64 < {} 2>/dev/null | tr -d '\\n'",
                shell_escape(&remote_path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[remote_path.as_str()],
                "下载远程文件",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "下载文件失败：{}",
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
            if bytes.len() as u64 > MAX_REMOTE_TRANSFER_BYTES {
                return Err("下载文件超过 128 MB 限制".to_string());
            }
            if overwrite {
                fs::write(&local_path, &bytes)
                    .map_err(|error| format!("写入本地下载文件失败：{error}"))?;
            } else {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&local_path)
                    .and_then(|mut file| file.write_all(&bytes))
                    .map_err(|error| format!("写入本地下载文件失败：{error}"))?;
            }
            let value = json!({ "remotePath": remote_path, "localPath": local_path.to_string_lossy(), "bytes": bytes.len(), "overwritten": overwrite, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_sink = sink.clone();
        let remote_for_task = remote_path.clone();
        let local_for_task = local_path.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let metadata = remote_sftp_lstat(
                    &sftp,
                    &remote_for_task,
                    &context.cancellation,
                    "读取远程下载源文件",
                )
                .map_err(|error| format!("无法读取远程下载源文件：{error}"))?;
                if !matches!(metadata.file_type(), ssh2::FileType::RegularFile) {
                    return Err("download_file 只支持普通文件，不支持目录".to_string());
                }
                if metadata.size.unwrap_or(0) > MAX_REMOTE_TRANSFER_BYTES {
                    return Err(format!(
                        "下载文件超过 {} MB 限制",
                        MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
                    ));
                }
                let mut source = retry_nonblocking_with_cancel(
                    "打开远程下载源文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.open(Path::new(&remote_for_task)),
                )?;
                let mut destination = if overwrite {
                    fs::File::create(&local_for_task)
                        .map_err(|error| format!("创建本地下载文件失败：{error}"))?
                } else {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&local_for_task)
                        .map_err(|error| format!("创建本地下载文件失败：{error}"))?
                };
                let mut buffer = [0u8; 8192];
                let mut total = 0u64;
                loop {
                    cancelled(&context.cancellation)?;
                    let length = retry_io_with_cancel(
                        "读取远程下载源文件",
                        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                        &context.cancellation,
                        || source.read(&mut buffer),
                    )?;
                    if length == 0 {
                        break;
                    }
                    total = total.saturating_add(length as u64);
                    write_remote_bytes(&mut destination, &buffer[..length], &context.cancellation)?;
                    if let Some(sink) = &operation_sink {
                        sink.emit_remote_progress(json!({
                            "stage": "progress",
                            "path": remote_for_task,
                            "completed": total,
                            "total": metadata.size,
                            "message": format!("已下载 {} / {} 字节", total, metadata.size.unwrap_or(0)),
                        }));
                    }
                }
                Ok(json!({ "remotePath": remote_for_task, "localPath": local_for_task.to_string_lossy(), "bytes": total, "overwritten": overwrite, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({ "stage": "completed", "path": remote_path, "message": format!("文件下载完成，共 {} 字节", result.get("bytes").and_then(Value::as_u64).unwrap_or_default()) }));
        }
        Ok(result)
    }
}

pub(super) fn collect_remote_directory_files(
    sftp: &ssh2::Sftp,
    root: &str,
    current: &str,
    relative: &Path,
    entries: &mut Vec<(String, PathBuf, u64)>,
    total_bytes: &mut u64,
    cancellation: &Arc<AtomicBool>,
) -> Result<(), String> {
    if entries.len() >= MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "远程目录内容超过 {} 项限制",
            MAX_REMOTE_DIRECTORY_ENTRIES
        ));
    }
    let mut children = retry_nonblocking_with_cancel(
        "读取远程目录",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.readdir(Path::new(current)),
    )?;
    children.sort_by(|left, right| left.0.cmp(&right.0));
    for (path, stat) in children {
        cancelled(cancellation)?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty() && *value != "." && *value != "..")
            .ok_or_else(|| format!("远程目录包含无法转换为 UTF-8 的文件名：{path:?}"))?;
        let child_remote = format!("{}/{}", current.trim_end_matches('/'), name);
        let child_relative = relative.join(name);
        if matches!(stat.file_type(), ssh2::FileType::Symlink) {
            return Err(format!("目录传输不跟随远程符号链接：{child_remote}"));
        }
        if matches!(stat.file_type(), ssh2::FileType::Directory) {
            collect_remote_directory_files(
                sftp,
                root,
                &child_remote,
                &child_relative,
                entries,
                total_bytes,
                cancellation,
            )?;
        } else if matches!(stat.file_type(), ssh2::FileType::RegularFile) {
            let size = stat.size.unwrap_or(0);
            *total_bytes = total_bytes.saturating_add(size);
            if *total_bytes > MAX_REMOTE_TRANSFER_BYTES {
                return Err(format!(
                    "远程目录内容超过 {} MB 限制",
                    MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
                ));
            }
            entries.push((child_remote, child_relative, size));
        }
    }
    let _ = root;
    Ok(())
}

pub(super) struct RemoteUploadDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteUploadDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "upload_directory",
            "递归上传用户明确选择的本地目录到 SSH 工作区。拒绝符号链接，最多 4096 项和 128 MB。",
            json!({
                "type": "object",
                "properties": {
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["local_path", "remote_path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let local_path = local_transfer_path(&context, &input, "local_path")?;
        let remote_path = remote_mutation_path(&context, &input, "remote_path")?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|error| format!("无法读取待上传目录：{error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("upload_directory 需要普通本地目录，不支持符号链接".to_string());
        }
        let mut entries = Vec::new();
        let mut total = 0u64;
        collect_local_directory_files(&local_path, &local_path, &mut entries, &mut total)?;
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let timeout =
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800));
            let child_context = context.with_output_sink(None);
            let result = async {
                ensure_openssh_remote_directory(&context, &remote_path, timeout)?;
                let upload_tool = RemoteUploadFileTool(child_context.clone());
                let mut completed = 0u64;
                for (local, relative, _) in &entries {
                    let remote = remote_relative_path(&remote_path, relative)?;
                    ensure_openssh_remote_directory(
                        &context,
                        remote_parent_path(&remote),
                        timeout,
                    )?;
                    let value = upload_tool
                        .execute(json!({
                            "local_path": local.to_string_lossy(),
                            "remote_path": remote,
                            "overwrite": overwrite,
                        }))
                        .await?;
                    let bytes = value
                        .get("bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    completed = completed.saturating_add(bytes);
                    if let Some(sink) = &sink {
                        sink.emit_remote_progress(json!({
                            "stage": "progress",
                            "path": remote_path,
                            "itemPath": remote,
                            "completed": completed,
                            "total": total,
                            "message": format!("已上传目录文件：{}", local.display()),
                        }));
                    }
                }
                Ok::<Value, String>(json!({
                    "localPath": local_path.to_string_lossy(),
                    "remotePath": remote_path,
                    "files": entries.len(),
                    "bytes": completed,
                    "totalBytes": total,
                    "overwritten": overwrite,
                    "target": "ssh",
                    "backend": "openssh",
                }))
            }
            .await;
            let result = finish_remote_operation(sink.as_ref(), result)?;
            if let Some(sink) = &sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": remote_path,
                    "message": format!("目录上传完成，共 {} 个文件", result["files"]),
                }));
            }
            return Ok(result);
        }
        let operation_sink = sink.clone();
        let local_for_task = local_path.clone();
        let remote_for_task = remote_path.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程目录上传",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                ensure_remote_directory_all(&sftp, &remote_for_task, &context.cancellation)?;
                let mut completed = 0u64;
                let mut files = 0u64;
                for (local, relative, size) in &entries {
                    let remote = remote_relative_path(&remote_for_task, relative)?;
                    ensure_remote_directory_all(
                        &sftp,
                        Path::new(&remote)
                            .parent()
                            .and_then(|value| value.to_str())
                            .unwrap_or(&remote_for_task),
                        &context.cancellation,
                    )?;
                    stream_local_file_to_remote(
                        &session,
                        local,
                        &remote,
                        overwrite,
                        &context.cancellation,
                        operation_sink.as_ref(),
                        *size,
                    )?;
                    completed = completed.saturating_add(*size);
                    files = files.saturating_add(1);
                }
                Ok(json!({
                    "localPath": local_for_task.to_string_lossy(),
                    "remotePath": remote_for_task,
                    "files": files,
                    "bytes": completed,
                    "totalBytes": total,
                    "overwritten": overwrite,
                    "target": "ssh"
                }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": remote_path,
                "message": format!("目录上传完成，共 {} 个文件", result["files"])
            }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteDownloadDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteDownloadDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "download_directory",
            "递归下载 SSH 工作区内明确指定的目录到用户明确选择的本地目录。拒绝符号链接，最多 4096 项和 128 MB。",
            json!({
                "type": "object",
                "properties": {
                    "remote_path": { "type": "string" },
                    "local_path": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                },
                "required": ["remote_path", "local_path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let remote_path = remote_mutation_path(&context, &input, "remote_path")?;
        let local_selection = local_transfer_path(&context, &input, "local_path")?;
        let overwrite = input
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let timeout =
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800));
            let listing = run_openssh_context_command_checked(
                &context,
                &openssh_directory_listing_command(&remote_path),
                None,
                timeout,
                None,
                &[remote_path.as_str()],
                "读取远程下载目录",
            )?;
            let (_entries, total, files) =
                openssh_directory_file_list(&listing, "读取远程下载目录")?;
            let directory_name = Path::new(&remote_path)
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("download");
            let local_root = if local_selection.is_dir() {
                local_selection.join(directory_name)
            } else {
                local_selection.clone()
            };
            ensure_local_transfer_path(&context, &local_root)?;
            if local_root.exists() && !local_root.is_dir() {
                return Err("本地目录下载目标不是目录".to_string());
            }
            fs::create_dir_all(&local_root)
                .map_err(|error| format!("创建本地下载目录失败：{error}"))?;
            let child_context = context.with_output_sink(None);
            let result = async {
                let download_tool = RemoteDownloadFileTool(child_context.clone());
                let mut completed = 0u64;
                for remote in &files {
                    let relative = remote_relative_path_from_base(&remote_path, remote)?;
                    let local = local_root.join(&relative);
                    if let Some(parent) = local.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|error| format!("创建本地目录失败：{error}"))?;
                    }
                    let value = download_tool
                        .execute(json!({
                            "remote_path": remote,
                            "local_path": local.to_string_lossy(),
                            "overwrite": overwrite,
                        }))
                        .await?;
                    let bytes = value
                        .get("bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    completed = completed.saturating_add(bytes);
                    if let Some(sink) = &sink {
                        sink.emit_remote_progress(json!({
                            "stage": "progress",
                            "path": remote_path,
                            "itemPath": remote,
                            "completed": completed,
                            "total": total,
                            "message": format!("已下载目录文件：{}", relative.display()),
                        }));
                    }
                }
                Ok::<Value, String>(json!({
                    "remotePath": remote_path,
                    "localPath": local_root.to_string_lossy(),
                    "files": files.len(),
                    "bytes": completed,
                    "totalBytes": total,
                    "overwritten": overwrite,
                    "target": "ssh",
                    "backend": "openssh",
                }))
            }
            .await;
            let result = finish_remote_operation(sink.as_ref(), result)?;
            if let Some(sink) = &sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": remote_path,
                    "message": format!("目录下载完成，共 {} 个文件", result["files"]),
                }));
            }
            return Ok(result);
        }
        let operation_sink = sink.clone();
        let remote_for_task = remote_path.clone();
        let local_selection_for_task = local_selection.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程目录下载",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let root_stat = remote_sftp_lstat(
                    &sftp,
                    &remote_for_task,
                    &context.cancellation,
                    "读取远程下载目录",
                )?;
                if !matches!(root_stat.file_type(), ssh2::FileType::Directory) {
                    return Err("download_directory 的 remote_path 必须是目录".to_string());
                }
                let directory_name = Path::new(&remote_for_task)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or("download");
                let local_root = if local_selection_for_task.is_dir() {
                    local_selection_for_task.join(directory_name)
                } else {
                    local_selection_for_task.clone()
                };
                ensure_local_transfer_path(context, &local_root)?;
                if local_root.exists() && !local_root.is_dir() {
                    return Err("本地目录下载目标不是目录".to_string());
                }
                fs::create_dir_all(&local_root)
                    .map_err(|error| format!("创建本地下载目录失败：{error}"))?;
                let mut entries = Vec::new();
                let mut total = 0u64;
                collect_remote_directory_files(
                    &sftp,
                    &remote_for_task,
                    &remote_for_task,
                    Path::new(""),
                    &mut entries,
                    &mut total,
                    &context.cancellation,
                )?;
                let mut completed = 0u64;
                for (remote, relative, _) in &entries {
                    let local = local_root.join(relative);
                    if let Some(parent) = local.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|error| format!("创建本地目录失败：{error}"))?;
                    }
                    download_remote_file_to_local(
                        &sftp,
                        remote,
                        &local,
                        overwrite,
                        &context.cancellation,
                        operation_sink.as_ref(),
                        &mut completed,
                        total,
                    )?;
                }
                Ok(json!({
                    "remotePath": remote_for_task,
                    "localPath": local_root.to_string_lossy(),
                    "files": entries.len(),
                    "bytes": completed,
                    "totalBytes": total,
                    "overwritten": overwrite,
                    "target": "ssh"
                }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": remote_path,
                "message": format!("目录下载完成，共 {} 个文件", result["files"])
            }));
        }
        Ok(result)
    }
}

pub(super) struct RemoteSetFilePermissionsTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteSetFilePermissionsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "set_file_permissions",
            "修改 SSH 工作区内明确文件或目录的 Unix 权限。只接受八进制 mode，例如 644、755 或 0755；不跟随符号链接。",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "mode": { "type": "string" }
                },
                "required": ["path", "mode"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_mutation_path(&context, &input, "path")?;
        let raw_mode = input
            .get("mode")
            .and_then(Value::as_str)
            .map(|value| value.trim().trim_start_matches("0o"))
            .filter(|value| !value.is_empty() && value.len() <= 4)
            .ok_or_else(|| "set_file_permissions 需要 1-4 位八进制 mode".to_string())?;
        if !raw_mode.chars().all(|value| ('0'..='7').contains(&value)) {
            return Err("权限 mode 必须是八进制数字".to_string());
        }
        let mode =
            u32::from_str_radix(raw_mode, 8).map_err(|error| format!("权限 mode 无效：{error}"))?;
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ -L {path} ]; then exit 2; fi; chmod {mode} {path}",
                path = shell_escape(&path),
                mode = raw_mode
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "设置远程文件权限",
            )?;
            if !output.status.success() {
                return Err(if output.status.code() == Some(2) {
                    "set_file_permissions 不跟随符号链接".to_string()
                } else {
                    format!(
                        "修改远程文件权限失败：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )
                });
            }
            let value = json!({ "path": path, "mode": format!("{mode:o}"), "updated": true, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let path_for_task = path.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程权限操作",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let stat = remote_sftp_lstat(
                    &sftp,
                    &path_for_task,
                    &context.cancellation,
                    "读取权限目标",
                )?;
                if matches!(stat.file_type(), ssh2::FileType::Symlink) {
                    return Err("set_file_permissions 不跟随符号链接".to_string());
                }
                retry_nonblocking_with_cancel(
                    "修改远程文件权限",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || {
                        sftp.setstat(
                            Path::new(&path_for_task),
                            ssh2::FileStat {
                                size: None,
                                uid: None,
                                gid: None,
                                perm: Some(mode),
                                atime: None,
                                mtime: None,
                            },
                        )
                    },
                )?;
                Ok(json!({ "path": path_for_task, "mode": format!("{mode:o}"), "updated": true, "target": "ssh" }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({ "stage": "completed", "path": path, "message": format!("远程权限已修改为 {:o}", mode) }));
        }
        Ok(result)
    }
}
