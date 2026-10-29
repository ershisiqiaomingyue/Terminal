use super::*;

pub(super) struct RemoteTreeTool(pub(super) SshToolContext);

pub(super) fn collect_remote_tree(
    sftp: &ssh2::Sftp,
    current: &str,
    depth: usize,
    max_depth: usize,
    excluded_names: &[String],
    entries: &mut Vec<Value>,
    sink: Option<&CommandOutputSink>,
    progress_events: &mut usize,
    cancellation: &Arc<AtomicBool>,
    is_root: bool,
) -> Result<(), String> {
    cancelled(cancellation)?;
    if depth > max_depth || entries.len() >= MAX_REMOTE_TREE_ENTRIES {
        return Ok(());
    }
    let mut children = match retry_nonblocking_with_cancel(
        "读取远程目录树",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.readdir(Path::new(current)),
    ) {
        Ok(children) => children,
        Err(error) if is_root => return Err(error),
        Err(error) => {
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
    children.sort_by(|left, right| left.0.cmp(&right.0));
    for (entry, stat) in children {
        cancelled(cancellation)?;
        if entries.len() >= MAX_REMOTE_TREE_ENTRIES {
            break;
        }
        let name = entry
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() || excluded_names.iter().any(|excluded| excluded == &name) {
            continue;
        }
        let path = remote_directory_entry_path(current, &entry);
        let is_directory = matches!(stat.file_type(), ssh2::FileType::Directory);
        entries.push(json!({
            "name": name,
            "path": path,
            "kind": if is_directory { "directory" } else { "file" },
            "depth": depth,
            "size": stat.size,
        }));
        if let Some(sink) = sink.filter(|_| *progress_events < MAX_REMOTE_PROGRESS_EVENTS) {
            sink.emit_remote_progress(json!({
                "stage": "item",
                "path": current,
                "item": name,
                "itemPath": path,
                "kind": if is_directory { "directory" } else { "file" },
                "completed": entries.len(),
                "message": if is_directory { format!("发现目录：{path}") } else { format!("发现文件：{path}") },
            }));
            *progress_events += 1;
        }
        if is_directory && depth < max_depth {
            collect_remote_tree(
                sftp,
                &path,
                depth + 1,
                max_depth,
                excluded_names,
                entries,
                sink,
                progress_events,
                cancellation,
                false,
            )?;
        }
    }
    Ok(())
}

#[async_trait]
impl ToolHandler for RemoteTreeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("get_directory_tree", "只读递归读取 SSH 工作区目录树，不执行 shell 命令。目录内容使用 list_local_files，普通文件使用 read_local_file。", json!({ "type": "object", "properties": { "path": { "type": "string" }, "depth": { "type": "integer", "maximum": 8 }, "max_depth": { "type": "integer", "maximum": 8 }, "exclude": { "type": "array", "items": { "type": "string" } } } })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        let depth = input
            .get("max_depth")
            .or_else(|| input.get("depth"))
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .min(8) as usize;
        let excluded_names = input
            .get("exclude")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if uses_openssh_fallback(&context.config) {
            let command = format!(
                "setopt NULL_GLOB 2>/dev/null || true; walk() {{ current=\"$1\"; current_depth=\"$2\"; [ \"$current_depth\" -gt {depth} ] && return; for item in \"$current\"/* \"$current\"/.[!.]* \"$current\"/..?*; do if [ -e \"$item\" ] || [ -L \"$item\" ]; then if [ -d \"$item\" ] && [ ! -L \"$item\" ]; then kind=directory; else kind=file; fi; printf '%s\\t%s\\n' \"$kind\" \"$item\"; if [ \"$kind\" = directory ] && [ \"$current_depth\" -lt {depth} ]; then walk \"$item\" $((current_depth + 1)); fi; fi; done; }}; if [ -L {path} ] || [ ! -d {path} ]; then exit 3; fi; walk {path} 1",
                path = shell_escape(&path),
                depth = depth.saturating_add(1)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[path.as_str()],
                "读取远程目录树",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "读取远程目录树失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let entries = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (kind, item_path) = line.split_once('\t')?;
                    if excluded_names.iter().any(|name| Path::new(item_path).components().any(|part| part.as_os_str() == std::ffi::OsStr::new(name))) {
                        return None;
                    }
                    let name = Path::new(item_path).file_name()?.to_str()?.to_string();
                    Some(json!({ "name": name, "path": item_path, "kind": kind, "depth": item_path.matches('/').count().saturating_sub(path.matches('/').count()), "size": Value::Null }))
                })
                .take(MAX_REMOTE_TREE_ENTRIES)
                .collect::<Vec<_>>();
            let value = json!({ "path": path, "entries": entries, "truncated": false, "target": "ssh", "backend": "openssh" });
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
                ensure_remote_path_safe(
                    &sftp,
                    &operation_path,
                    &context.root,
                    &context.cancellation,
                    "读取远程目录树",
                )?;
                let mut entries = Vec::new();
                let mut progress_events = 0usize;
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({
                        "stage": "scanning",
                        "path": operation_path,
                        "message": format!("开始读取目录树，深度 {depth}"),
                    }));
                    progress_events += 1;
                }
                collect_remote_tree(
                    &sftp,
                    &operation_path,
                    0,
                    depth,
                    &excluded_names,
                    &mut entries,
                    operation_sink.as_ref(),
                    &mut progress_events,
                    &context.cancellation,
                    true,
                )?;
                if let Some(sink) = &operation_sink {
                    sink.emit_remote_progress(json!({
                        "stage": "completed",
                        "path": operation_path,
                        "completed": entries.len(),
                        "total": entries.len(),
                        "message": format!("目录树读取完成，共 {} 项", entries.len()),
                    }));
                }
                Ok(entries)
            }))
            .await,
        )?;
        Ok(json!({
            "path": path,
            "entries": result,
            "truncated": result.len() >= MAX_REMOTE_TREE_ENTRIES,
            "target": "ssh",
        }))
    }
}
