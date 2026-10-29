use super::*;

pub(super) struct RemoteWorkingDirectoryTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteWorkingDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_working_directory",
            "连接 SSH 主机并确认当前远程工作区目录。",
            json!({ "type": "object", "properties": {} }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let configured_path = context.root.clone();
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": configured_path,
                "itemPath": configured_path,
                "message": "正在连接 SSH 并确认远程工作目录",
            }));
        }
        let operation_sink = sink.clone();
        let command = format!("cd {} && pwd", shell_escape(&configured_path));
        if uses_openssh_fallback(&context.config) {
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[configured_path.as_str()],
                "确认远程工作目录",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "远程工作目录不可用：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let remote_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let value = json!({
                "path": configured_path,
                "remotePath": if remote_path.is_empty() { context.root.clone() } else { remote_path },
                "selected": true,
                "target": "ssh",
                "backend": "openssh"
            });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let result = match await_remote(spawn_remote(context.clone(), move |session, context| {
            let channel = retry_nonblocking_with_cancel(
                "打开 SSH 工作目录通道",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                &context.cancellation,
                || session.channel_session(),
            )?;
            let result = execute_remote_command(
                channel,
                &session,
                &command,
                context,
                operation_sink.as_ref(),
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                None,
            )?;
            finish_remote_sink(operation_sink.as_ref(), &result);
            Ok(result)
        }))
        .await
        {
            Ok(result) => result,
            Err(error) => {
                if let Some(sink) = &sink {
                    sink.emit_remote_finished(
                        Some("error"),
                        json!({ "success": false }),
                        json!({ "error": error }),
                    );
                }
                return Err(error);
            }
        };
        if !result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(result
                .get("stderr")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("远程工作目录不可用：{}", value.trim()))
                .unwrap_or_else(|| "远程工作目录不可用".to_string()));
        }
        let remote_path = result
            .get("stdout")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&configured_path)
            .to_string();
        if let Some(sink) = remote_output_sink_for_input(&self.0, &input) {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": configured_path,
                "itemPath": remote_path,
                "message": "已确认远程工作目录",
            }));
        }
        Ok(json!({
            "path": configured_path,
            "remotePath": remote_path,
            "selected": true,
            "target": "ssh",
        }))
    }
}

pub(super) struct RemoteListFilesTool(pub(super) SshToolContext);

pub(super) struct RemoteFileInfoTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteFileInfoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_file_info",
            "读取 SSH 工作区中一个明确路径的文件或目录元信息，不读取文件正文。",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "if [ -L {0} ]; then kind=symlink; elif [ -d {0} ]; then kind=directory; elif [ -f {0} ]; then kind=file; else kind=other; fi; size=$(wc -c < {0} 2>/dev/null || echo 0); perm=$(stat -c '%a' {0} 2>/dev/null || stat -f '%Lp' {0} 2>/dev/null || true); printf '%s\\t%s\\t%s\\n' \"$kind\" \"$size\" \"$perm\"",
                shell_escape(&path)
            );
            let result = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "读取远程路径元信息",
            )?;
            if !result.status.success() {
                return Err(format!(
                    "读取远程路径元信息失败：{}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            let fields = String::from_utf8_lossy(&result.stdout)
                .trim()
                .split('\t')
                .map(str::to_string)
                .collect::<Vec<_>>();
            let value = json!({
                "path": path,
                "kind": fields.first().cloned().unwrap_or_else(|| "other".to_string()),
                "size": fields.get(1).and_then(|value| value.parse::<u64>().ok()),
                "permissions": fields.get(2).and_then(|value| u32::from_str_radix(value.trim(), 8).ok()),
                "target": "ssh",
                "backend": "openssh"
            });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_path = path.clone();
        let operation_sink = sink.clone();
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context, move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程 SFTP",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({
                        "stage": "scanning",
                        "path": operation_path,
                        "itemPath": operation_path,
                        "message": "正在读取远程路径元信息",
                    }));
                }
                let stat = retry_nonblocking_with_cancel(
                    "读取远程路径元信息",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.lstat(Path::new(&operation_path)),
                )?;
                let kind = match stat.file_type() {
                    ssh2::FileType::Directory => "directory",
                    ssh2::FileType::RegularFile => "file",
                    ssh2::FileType::Symlink => "symlink",
                    _ => "other",
                };
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({
                        "stage": "completed",
                        "path": operation_path,
                        "itemPath": operation_path,
                        "message": format!("远程路径元信息读取完成：{kind}"),
                    }));
                }
                Ok(json!({
                    "path": operation_path,
                    "kind": kind,
                    "size": stat.size,
                    "permissions": stat.perm.map(|value| value & 0o7777),
                    "userId": stat.uid,
                    "groupId": stat.gid,
                    "modifiedAt": stat.mtime,
                    "accessedAt": stat.atime,
                    "target": "ssh",
                }))
            }))
            .await,
        )?;
        Ok(result)
    }
}

#[async_trait]
impl ToolHandler for RemoteListFilesTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_local_files",
            "列出 SSH 工作区目录中的文件和目录。",
            json!({
                "type": "object", "properties": { "path": { "type": "string" } }
            }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "setopt NULL_GLOB 2>/dev/null || true; if [ -L {path} ]; then exit 2; fi; for entry in {path}/* {path}/.[!.]* {path}/..?*; do if [ -e \"$entry\" ] || [ -L \"$entry\" ]; then if [ -L \"$entry\" ]; then kind=symlink; elif [ -d \"$entry\" ]; then kind=directory; elif [ -f \"$entry\" ]; then kind=file; else kind=other; fi; size=$(wc -c < \"$entry\" 2>/dev/null || echo 0); printf '%s\\t%s\\t%s\\n' \"$kind\" \"$entry\" \"$size\"; fi; done",
                path = shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
                &[path.as_str()],
                "读取远程目录",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "读取远程目录失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let entries = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (kind, path, size) = line.split_once('\t').and_then(|(kind, rest)| {
                        let (path, size) = rest.rsplit_once('\t')?;
                        Some((kind, path, size))
                    })?;
                    Some(json!({
                        "name": Path::new(path).file_name().and_then(|value| value.to_str()).unwrap_or_default(),
                        "path": path,
                        "kind": kind,
                        "size": size.parse::<u64>().ok()
                    }))
                })
                .collect::<Vec<_>>();
            if entries.len() > MAX_REMOTE_DIRECTORY_ENTRIES {
                return Err(format!(
                    "读取远程目录超过 {} 项限制",
                    MAX_REMOTE_DIRECTORY_ENTRIES
                ));
            }
            let value =
                json!({ "path": path, "entries": entries, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let operation_path = path.clone();
        let cancellation = context.cancellation.clone();
        let operation_sink = sink.clone();
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
                "读取远程目录",
            )?;
            if let Some(sink) = &operation_sink {
                sink.emit_remote_progress(json!({
                    "stage": "scanning",
                    "path": operation_path,
                    "message": "正在读取远程目录",
                }));
            }
            let mut entries = retry_nonblocking_with_cancel(
                "读取远程目录",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                &cancellation,
                || sftp.readdir(Path::new(&operation_path)),
            )?;
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            if entries.len() > MAX_REMOTE_DIRECTORY_ENTRIES {
                return Err(format!(
                    "读取远程目录超过 {} 项限制",
                    MAX_REMOTE_DIRECTORY_ENTRIES
                ));
            }
            let total = entries.len();
            let mut result = Vec::with_capacity(total);
            for (index, (entry, stat)) in entries.into_iter().enumerate() {
                cancelled(&cancellation)?;
                let name = entry.file_name().and_then(|value| value.to_str()).unwrap_or_default();
                let entry_path = remote_directory_entry_path(&operation_path, &entry);
                let kind = match stat.file_type() {
                    ssh2::FileType::Directory => "directory",
                    ssh2::FileType::Symlink => "symlink",
                    ssh2::FileType::RegularFile => "file",
                    _ => "other",
                };
                result.push(json!({
                    "name": name,
                    "path": entry_path,
                    "kind": kind,
                    "size": stat.size,
                }));
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({
                        "stage": "item",
                        "path": operation_path,
                        "item": name,
                        "itemPath": entry_path,
                        "kind": kind,
                        "completed": index + 1,
                        "total": total,
                        "message": if kind == "directory" { format!("发现目录：{name}") } else { format!("发现文件：{name}") },
                    }));
                }
            }
            if let Some(sink) = &operation_sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": operation_path,
                    "completed": result.len(),
                    "total": total,
                    "message": format!("目录读取完成，共 {} 项", result.len()),
                }));
            }
            Ok(result)
            }))
            .await,
        )?;
        Ok(json!({ "path": path, "entries": result, "target": "ssh" }))
    }
}
