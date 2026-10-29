use super::*;

pub fn register_ssh_tools(
    registry: &mut ToolRegistry,
    context: SshToolContext,
) -> Result<(), String> {
    let handlers: Vec<Box<dyn ToolHandler>> = vec![
        Box::new(RemotePathAccessProbeTool(
            context.with_tool("__agent_path_access_probe"),
        )),
        Box::new(RemoteCurrentTimeTool(context.with_tool("get_current_time"))),
        Box::new(RemoteWorkingDirectoryTool(
            context.with_tool("get_working_directory"),
        )),
        Box::new(RemoteFileInfoTool(context.with_tool("get_file_info"))),
        Box::new(RemoteListFilesTool(context.with_tool("list_local_files"))),
        Box::new(RemoteSearchFilesTool(
            context.with_tool("search_local_files"),
        )),
        Box::new(RemoteSearchCodeTool(context.with_tool("search_local_code"))),
        Box::new(RemoteReadFileTool(context.with_tool("read_local_file"))),
        Box::new(RemoteWriteFileTool(context.with_tool("write_local_file"))),
        Box::new(RemoteCreateDirectoryTool(
            context.with_tool("create_directory"),
        )),
        Box::new(RemoteCopyFileTool(context.with_tool("copy_file"))),
        Box::new(RemoteMoveFileTool(context.with_tool("move_file"))),
        Box::new(RemoteCopyDirectoryTool(context.with_tool("copy_directory"))),
        Box::new(RemoteMoveDirectoryTool(context.with_tool("move_directory"))),
        Box::new(RemoteDeleteFileTool(context.with_tool("delete_file"))),
        Box::new(RemoteUploadFileTool(context.with_tool("upload_file"))),
        Box::new(RemoteDownloadFileTool(context.with_tool("download_file"))),
        Box::new(RemoteUploadPathsTool(context.with_tool("upload_paths"))),
        Box::new(RemoteDownloadPathTool(context.with_tool("download_path"))),
        Box::new(RemoteUploadDirectoryTool(
            context.with_tool("upload_directory"),
        )),
        Box::new(RemoteDownloadDirectoryTool(
            context.with_tool("download_directory"),
        )),
        Box::new(RemoteSetFilePermissionsTool(
            context.with_tool("set_file_permissions"),
        )),
        Box::new(RemoteApplyPatchTool(context.with_tool("apply_patch"))),
        Box::new(RemoteCommandTool(context.with_tool("run_local_command"))),
        Box::new(RemoteShellTool(context.with_tool("run_local_shell"))),
        Box::new(RemoteInteractiveShellTool(
            context.with_tool("run_interactive_shell"),
        )),
        Box::new(RemoteProjectCheckTool(
            context.with_tool("run_project_check"),
        )),
        Box::new(RemoteGitTool(context.with_tool("get_git_status"), false)),
        Box::new(RemoteGitTool(context.with_tool("get_git_diff"), true)),
        Box::new(RemoteTreeTool(context.with_tool("get_directory_tree"))),
    ];
    for handler in handlers {
        registry
            .register_handler_boxed(handler)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) struct RemotePathAccessProbeTool(pub(super) SshToolContext);

pub(super) struct RemoteCurrentTimeTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteCurrentTimeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_current_time",
            "读取当前 SSH 主机的日期、时间和时区。",
            json!({ "type": "object", "properties": {} }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "message": "正在读取远程主机时间",
            }));
        }
        let operation_sink = sink.clone();
        let command = "printf '__AT_LOCAL__%s\\n' \"$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || true)\"; printf '__AT_ISO__%s\\n' \"$(date -Iseconds 2>/dev/null || date '+%Y-%m-%dT%H:%M:%S%z' 2>/dev/null || true)\"; printf '__AT_TZ__%s\\n' \"$(date '+%Z %z' 2>/dev/null || true)\"".to_string();
        if uses_openssh_fallback(&context.config) {
            let output = run_openssh_context_command(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 120)),
                None,
            )?;
            if !output.status.success() {
                return Err(format!(
                    "读取远程时间失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let mut local = String::new();
            let mut iso = String::new();
            let mut timezone = String::new();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(value) = line.strip_prefix("__AT_LOCAL__") {
                    local = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("__AT_ISO__") {
                    iso = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("__AT_TZ__") {
                    timezone = value.trim().to_string();
                }
            }
            let value = json!({ "local": local, "iso": iso, "timezone": timezone, "target": "ssh", "backend": "openssh" });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            return Ok(value);
        }
        let result = finish_remote_operation(
            sink.as_ref(),
            await_remote(spawn_remote(context.clone(), move |session, context| {
                let channel = retry_nonblocking_with_cancel(
                    "打开 SSH 时间通道",
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
                Ok(result)
            }))
            .await,
        )?;
        if result.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(result
                .get("stderr")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("读取远程时间失败：{}", value.trim()))
                .unwrap_or_else(|| "读取远程时间失败".to_string()));
        }
        let mut local = String::new();
        let mut iso = String::new();
        let mut timezone = String::new();
        for line in result
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .lines()
        {
            if let Some(value) = line.strip_prefix("__AT_LOCAL__") {
                local = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("__AT_ISO__") {
                iso = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("__AT_TZ__") {
                timezone = value.trim().to_string();
            }
        }
        if local.is_empty() {
            return Err("远程主机未返回有效时间".to_string());
        }
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "message": format!("远程时间：{local}"),
            }));
        }
        Ok(json!({
            "local": local,
            "iso": iso,
            "timezone": timezone,
            "target": "ssh",
        }))
    }
}

#[async_trait]
impl ToolHandler for RemotePathAccessProbeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "__agent_path_access_probe",
            "Internal SSH path permission preflight.",
            json!({ "type": "object", "properties": { "path": { "type": "string" }, "action": { "type": "string" } }, "required": ["path"] }),
        )
        .requiring_permission()
        .internal()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let path = remote_path(&self.0, &input)?;
        Ok(json!({
            "preflight": true,
            "path": path,
            "action": input.get("action").and_then(Value::as_str).unwrap_or_default(),
            "target": "ssh",
        }))
    }
}
