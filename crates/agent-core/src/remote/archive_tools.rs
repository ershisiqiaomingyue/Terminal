//! Model-facing adapters for the file browser's disk-backed archive transfers.
//! Keep these separate from directory tools, whose destinations merge contents.
use super::*;
use crate::transfer_archive::safe_transfer_name;
use std::sync::atomic::AtomicU64;

static TRANSFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TransferDirectory(PathBuf);

impl TransferDirectory {
    fn create(parent: &Path) -> Result<Self, String> {
        for _ in 0..100 {
            let sequence = TRANSFER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".android-tool-agent-transfer-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("无法创建传输临时目录 {}：{error}", path.display()))
                }
            }
        }
        Err("无法分配传输临时目录".into())
    }
}

impl Drop for TransferDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TransferConnection(SshConnectionConfig);

impl TransferConnection {
    fn create(config: &SshConnectionConfig) -> Self {
        let sequence = TRANSFER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self(transfer_connection(
            config,
            &format!(":agent-transfer-{}-{sequence}", std::process::id()),
        ))
    }
}

impl Drop for TransferConnection {
    fn drop(&mut self) {
        disconnect_ssh_connection(&self.0);
    }
}

fn selected_local_path(context: &SshToolContext, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() || raw.starts_with("attachment://") || raw.starts_with("browser://") {
        return Err("传输需要用户明确选择的真实本地文件或目录".into());
    }
    let path = resolve_selected_path(Path::new(raw), &context.local_transfer_roots);
    if !is_allowed_agent_path(
        &path,
        &context.local_transfer_roots,
        context.allow_external_paths,
    ) {
        return Err("本地传输路径必须是用户明确选择的文件或目录".into());
    }
    Ok(path)
}

fn upload_sources(context: &SshToolContext, input: &Value) -> Result<Vec<String>, String> {
    let paths = input
        .get("local_paths")
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty() && paths.len() <= 128)
        .ok_or("local_paths 需要 1 到 128 个本地文件或目录路径")?;
    paths
        .iter()
        .map(|path| {
            let raw = path.as_str().ok_or("local_paths 每一项必须是路径字符串")?;
            selected_local_path(context, raw)?
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| "上传路径不是 UTF-8".into())
        })
        .collect()
}

fn upload_policy(input: &Value) -> Result<UploadPolicy, String> {
    match input.get("conflict_policy").and_then(Value::as_str) {
        None if input.get("conflict_policy").is_none() => Ok(UploadPolicy::Reject),
        Some("reject") => Ok(UploadPolicy::Reject),
        Some("rename") => Ok(UploadPolicy::Rename),
        Some("overwrite") => Ok(UploadPolicy::Overwrite),
        _ => Err("conflict_policy 必须是 reject、rename 或 overwrite".into()),
    }
}

fn archive_progress(sink: Option<CommandOutputSink>, path: String) -> TransferProgress {
    let reported = Mutex::new((String::new(), Instant::now()));
    Arc::new(move |stage, completed, total| {
        let Some(sink) = &sink else { return };
        if let Ok(mut last) = reported.lock() {
            if last.0 == stage && last.1.elapsed() < Duration::from_millis(200) {
                return;
            }
            *last = (stage.into(), Instant::now());
        }
        let message = match stage {
            "checking" => "正在检查传输文件",
            "compressing" => "正在压缩打包",
            "transferring" => "正在传输压缩包",
            "extracting" => "正在解压并保存",
            "completed" => "传输完成",
            _ => "正在传输",
        };
        let message = if total > 0 {
            format!("{message} · {completed} / {total} 字节")
        } else {
            message.to_string()
        };
        sink.emit_remote_progress(json!({
            "stage": stage, "path": path, "completed": completed,
            "total": total, "message": message,
        }));
    })
}

/// Resolve real paths before calling the browser API, which has no agent scope.
/// Download refuses links in a tree so tar cannot dereference outside the scope.
fn checked_remote_path(
    context: &SshToolContext,
    path: &str,
    download: bool,
) -> Result<String, String> {
    if path.contains('\0') {
        return Err("远程传输路径包含无效字符".into());
    }
    let guard = if context.allow_external_paths {
        String::new()
    } else {
        format!(
            "root_real=$(realpath -e -- {root}) || exit 1; case \"$target_real\" in \"$root_real\"|\"${{root_real%/}}\"/*) ;; *) printf '传输路径越出 SSH 工作目录\\n' >&2; exit 1 ;; esac; ",
            root = shell_escape(&context.root),
        )
    };
    let kind_check = if download {
        // Check find's exit code so unreadable subdirectories fail closed.
        // -printf emits a fixed marker independent of unusual filenames.
        "if [ ! -f \"$target_real\" ] && [ ! -d \"$target_real\" ]; then exit 1; fi; bad=$(find \"$target_real\" \\( -type l -o ! \\( -type f -o -type d \\) \\) -printf x -quit) || exit 1; if [ -n \"$bad\" ]; then printf '目录下载不支持符号链接或特殊文件\\n' >&2; exit 1; fi; "
    } else {
        "test -d \"$target_real\" || { printf '上传目标必须是已有目录\\n' >&2; exit 1; }; "
    };
    let command = format!(
        "test ! -L {path} || {{ printf '传输目标不能是符号链接\\n' >&2; exit 1; }}; target_real=$(realpath -e -- {path}) || exit 1; {guard}{kind_check}printf '%s' \"$target_real\"",
        path = shell_escape(path),
    );
    let result = run_ssh_workspace_command(
        &context.config,
        context.password.as_deref(),
        "/",
        &command,
        context.cancellation.clone(),
        Duration::from_secs(120),
    )?;
    checked_remote_path_result(result)
}

fn checked_remote_path_result(result: SshCommandResult) -> Result<String, String> {
    // OpenSSH host-key notices are not a failed scope/path check. The shell
    // guards above already fail explicitly for inaccessible or unsafe paths.
    if result.exit_code != 0 {
        return Err(format!("无法检查传输路径：{}", result.stderr.trim()));
    }
    if result.output_truncated {
        return Err("服务器返回的传输路径不完整".into());
    }
    let resolved = result.stdout;
    if !resolved.starts_with('/') || resolved.contains('\0') {
        return Err("服务器返回的传输路径无效".into());
    }
    Ok(resolved)
}

pub(super) struct RemoteUploadPathsTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteUploadPathsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "upload_paths",
            "批量上传本地文件或目录到 SSH 已有目录，复用文件预览的压缩打包→一次传输→远程解包流程。目录或多个文件优先用此工具，不要逐文件上传或手写文件内容。每个所选项保留原名称放在 remote_path 下。conflict_policy 默认 reject；rename 保留两份；只有用户明确同意替换整个同名项才用 overwrite（目录整体替换，不合并）。",
            json!({
                "type": "object",
                "properties": {
                    "local_paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 128 },
                    "remote_path": { "type": "string", "description": "SSH 上已有的接收目录，所选文件和目录放在此目录下" },
                    "conflict_policy": { "type": "string", "enum": ["reject", "rename", "overwrite"], "default": "reject" }
                },
                "required": ["local_paths", "remote_path"]
            }),
        ).requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let mut context = self.0.clone();
        let paths = upload_sources(&context, &input)?;
        let remote = remote_mutation_path(&context, &input, "remote_path")?;
        let policy = upload_policy(&input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        let progress = archive_progress(sink.clone(), remote.clone());
        let result = tokio::task::spawn_blocking(move || {
            cancelled(&context.cancellation)?;
            let connection = TransferConnection::create(&context.config);
            context.config = connection.0.clone();
            progress("checking", 0, 0);
            let destination = checked_remote_path(&context, &remote, false)?;
            let temp = TransferDirectory::create(&std::env::temp_dir())?;
            let conflicts = inspect_ssh_browser_upload(
                &context.config, context.password.as_deref(), &paths, &destination,
            )?;
            upload_ssh_browser_paths(
                &context.config, context.password.as_deref(), &paths, &destination,
                &temp.0.join("payload.tar.gz"), policy, &conflicts,
                context.cancellation.clone(), progress.clone(),
            )?;
            progress("completed", 0, 0);
            Ok(json!({
                "localPaths": paths, "remotePath": destination,
                "items": paths.len(), "target": "ssh", "transport": "archive",
                "archiveFormat": "tar.gz", "conflictPolicy": input.get("conflict_policy").and_then(Value::as_str).unwrap_or("reject"),
            }))
        }).await.map_err(|error| format!("批量上传任务异常：{error}"))
            .and_then(|result| result);
        finish_remote_operation(sink.as_ref(), result)
    }
}

pub(super) struct RemoteDownloadPathTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteDownloadPathTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "download_path",
            "下载 SSH 文件或整个目录到用户选择的本地已有目录，复用文件预览的远程打包→一次传输→本地解包流程，目录优先用此工具。保留远程名称；默认拒绝同名项，rename 保留两份。超过 1 GB 时，只有用户明确确认大文件下载后才设置 confirmed_large=true。",
            json!({
                "type": "object",
                "properties": {
                    "remote_path": { "type": "string" },
                    "local_path": { "type": "string", "description": "用户选择的本地已有接收目录" },
                    "conflict_policy": { "type": "string", "enum": ["reject", "rename"], "default": "reject" },
                    "confirmed_large": { "type": "boolean", "default": false }
                },
                "required": ["remote_path", "local_path"]
            }),
        ).requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let mut context = self.0.clone();
        let remote = remote_mutation_path(&context, &input, "remote_path")?;
        let local = selected_local_path(
            &context,
            input
                .get("local_path")
                .and_then(Value::as_str)
                .ok_or("local_path 需要本地接收目录")?,
        )?;
        if !local.is_dir() {
            return Err("local_path 必须是用户选择的本地已有目录".into());
        }
        let policy = upload_policy(&input)?;
        if matches!(policy, UploadPolicy::Overwrite) {
            return Err("download_path 的冲突策略只支持 reject 或 rename".into());
        }
        let confirmed_large = input
            .get("confirmed_large")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sink = remote_output_sink_for_input(&context, &input);
        let progress = archive_progress(sink.clone(), remote.clone());
        let result = tokio::task::spawn_blocking(move || {
            cancelled(&context.cancellation)?;
            let connection = TransferConnection::create(&context.config);
            context.config = connection.0.clone();
            progress("checking", 0, 0);
            let remote = checked_remote_path(&context, &remote, true)?;
            let name = remote
                .rsplit('/')
                .next()
                .filter(|name| safe_transfer_name(name))
                .ok_or("请选择具体的文件或目录")?;
            let target = local.join(name);
            if matches!(policy, UploadPolicy::Reject) && fs::symlink_metadata(&target).is_ok() {
                return Err(
                    "本地同名文件或目录已存在；可使用 conflict_policy=rename 保留两份".into(),
                );
            }
            let temp = TransferDirectory::create(&local)?;
            download_ssh_browser_path(
                &context.config,
                context.password.as_deref(),
                &remote,
                &temp.0,
                confirmed_large,
                context.cancellation.clone(),
                progress.clone(),
            )?;
            cancelled(&context.cancellation)?;
            let output = if matches!(policy, UploadPolicy::Rename) {
                crate::browser_downloads::publish_browser_download(&temp.0, &local, name)?
            } else {
                crate::browser_mutations::rename_no_replace(&temp.0.join(name), &target)
                    .map_err(|error| format!("保存下载内容失败，未覆盖现有文件：{error}"))?;
                target
            };
            progress("completed", 0, 0);
            Ok(json!({
                "remotePath": remote, "localPath": output.to_string_lossy(),
                "target": "ssh", "transport": "archive", "archiveFormat": "tar.gz",
            }))
        })
        .await
        .map_err(|error| format!("归档下载任务异常：{error}"))
        .and_then(|result| result);
        finish_remote_operation(sink.as_ref(), result)
    }
}

#[cfg(test)]
#[path = "archive_tools_tests.rs"]
mod tests;
