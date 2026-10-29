use super::*;

pub(super) fn remote_patch_hunk_start(header: &str, fallback: usize) -> Result<usize, String> {
    if header.trim() == "@@" {
        return Ok(fallback);
    }
    header
        .split_whitespace()
        .find(|part| part.starts_with('-') && part.len() > 1)
        .ok_or_else(|| format!("无法解析补丁 hunk 头：{header}"))?
        .trim_start_matches('-')
        .split(',')
        .next()
        .ok_or_else(|| format!("无法解析补丁 hunk 起始行：{header}"))?
        .parse::<usize>()
        .map_err(|_| format!("无法解析补丁 hunk 起始行：{header}"))
}

pub(super) fn apply_remote_patch_text(
    content: &str,
    patch: &str,
    cancellation: &Arc<AtomicBool>,
) -> Result<(String, usize), String> {
    let source = content.split('\n').collect::<Vec<_>>();
    let mut result = Vec::with_capacity(source.len());
    let mut source_cursor = 0usize;
    let mut hunk_count = 0usize;
    let mut in_hunk = false;
    let normalized_patch = patch.replace("\r\n", "\n");
    for raw_line in normalized_patch.lines() {
        cancelled(cancellation)?;
        if raw_line.starts_with("@@") {
            let start = remote_patch_hunk_start(raw_line, source_cursor + 1)?.saturating_sub(1);
            if start < source_cursor || start > source.len() {
                return Err(format!("补丁 hunk 起始行超出文件范围：{}", start + 1));
            }
            result.extend(source[source_cursor..start].iter().copied());
            source_cursor = start;
            in_hunk = true;
            hunk_count += 1;
            continue;
        }
        if raw_line.starts_with("*** Begin Patch")
            || raw_line.starts_with("*** Update File")
            || raw_line.starts_with("*** End Patch")
            || raw_line.starts_with("--- ")
            || raw_line.starts_with("+++ ")
        {
            continue;
        }
        if !in_hunk || raw_line.starts_with("\\ No newline") {
            continue;
        }
        let (kind, value) = raw_line.split_at(1);
        match kind {
            " " => {
                let expected = source
                    .get(source_cursor)
                    .copied()
                    .ok_or_else(|| "补丁上下文超出文件范围".to_string())?;
                if expected != value {
                    return Err(format!("补丁上下文不匹配，期望：{expected}"));
                }
                result.push(expected);
                source_cursor += 1;
            }
            "-" => {
                let expected = source
                    .get(source_cursor)
                    .copied()
                    .ok_or_else(|| "补丁删除行超出文件范围".to_string())?;
                if expected != value {
                    return Err(format!("补丁删除行不匹配，期望：{expected}"));
                }
                source_cursor += 1;
            }
            "+" => result.push(value),
            _ => return Err(format!("补丁包含无法识别的行：{raw_line}")),
        }
    }
    if hunk_count == 0 {
        return Err("apply_patch 需要至少一个 @@ hunk".to_string());
    }
    result.extend(source[source_cursor..].iter().copied());
    Ok((result.join("\n"), hunk_count))
}

pub(super) struct RemoteApplyPatchTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "apply_patch",
            "对 SSH 工作区中的一个已有文本文件应用经过上下文校验的小范围补丁；支持 patch，或唯一匹配的 old_text/new_text。",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "patch": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" }
                },
                "required": ["path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let path = remote_path(&context, &input)?;
        let sink = remote_output_sink_for_input(&context, &input);
        let patch = input
            .get("patch")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        let old_text = input
            .get("old_text")
            .and_then(Value::as_str)
            .map(str::to_string);
        let new_text = input
            .get("new_text")
            .and_then(Value::as_str)
            .map(str::to_string);
        if patch.as_ref().is_some_and(|value| value.len() > 512 * 1024) {
            return Err("单次远程补丁不能超过 512 KB".to_string());
        }
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": path,
                "itemPath": path,
                "message": "正在读取远程文件并校验补丁",
            }));
        }
        if uses_openssh_fallback(&context.config) {
            let read_command = format!(
                "if [ ! -f {path} ] || [ -L {path} ]; then exit 2; fi; base64 < {path} 2>/dev/null | tr -d '\\n'",
                path = shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &read_command,
                None,
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[path.as_str()],
                "读取远程补丁文件",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "读取远程补丁文件失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let encoded = String::from_utf8_lossy(&output.stdout)
                .chars()
                .filter(|value| !value.is_whitespace())
                .collect::<String>();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|error| format!("远程补丁文件解码失败：{error}"))?;
            if bytes.len() > MAX_REMOTE_PATCH_FILE_BYTES {
                return Err(format!(
                    "apply_patch 不支持超过 {} MB 的远程文件",
                    MAX_REMOTE_PATCH_FILE_BYTES / (1024 * 1024)
                ));
            }
            let content = String::from_utf8(bytes)
                .map_err(|_| "无法读取远程文本文件：文件不是 UTF-8 文本".to_string())?;
            let (next, applied) = if let Some(patch) = patch.as_deref() {
                apply_remote_patch_text(&content, patch, &context.cancellation)?
            } else {
                let old_text = old_text.as_deref().ok_or_else(|| {
                    "apply_patch 需要 patch，或同时提供 old_text 和 new_text".to_string()
                })?;
                let new_text = new_text.as_deref().ok_or_else(|| {
                    "apply_patch 需要 new_text，或同时提供 old_text 和 patch".to_string()
                })?;
                if old_text.is_empty() {
                    return Err("apply_patch 不接受空 old_text".to_string());
                }
                if content.match_indices(old_text).count() != 1 {
                    return Err("apply_patch 要求 old_text 恰好匹配一次".to_string());
                }
                (content.replacen(old_text, new_text, 1), 1)
            };
            let encoded_next = base64::engine::general_purpose::STANDARD.encode(next.as_bytes());
            let write_command = format!(
                "if [ ! -f {path} ] || [ -L {path} ]; then exit 2; fi; (base64 -d 2>/dev/null || base64 -D 2>/dev/null) > {path}",
                path = shell_escape(&path)
            );
            let output = run_openssh_context_command_checked(
                &context,
                &write_command,
                Some(encoded_next.as_bytes()),
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800)),
                None,
                &[path.as_str()],
                "写入远程补丁文件",
            )?;
            if !output.status.success() {
                return Err(format!(
                    "写入远程补丁失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let value = json!({
                "path": path,
                "appliedHunks": applied,
                "oldBytes": content.len(),
                "newBytes": next.len(),
                "target": "ssh",
                "backend": "openssh",
            });
            finish_remote_operation(sink.as_ref(), Ok(value.clone()))?;
            if let Some(sink) = &sink {
                sink.emit_remote_progress(json!({
                    "stage": "completed",
                    "path": path,
                    "itemPath": path,
                    "message": "远程补丁应用完成",
                }));
            }
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
                let metadata = retry_nonblocking_with_cancel(
                    "读取远程补丁文件属性",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.stat(Path::new(&result_path)),
                )?;
                if metadata.size.unwrap_or(0) > MAX_REMOTE_PATCH_FILE_BYTES as u64 {
                    return Err(format!(
                        "apply_patch 不支持超过 {} MB 的远程文件",
                        MAX_REMOTE_PATCH_FILE_BYTES / (1024 * 1024)
                    ));
                }
                let mut file = retry_nonblocking_with_cancel(
                    "打开远程文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.open(Path::new(&result_path)),
                )?;
                let bytes = read_remote_bytes(
                    &mut file,
                    MAX_REMOTE_PATCH_FILE_BYTES,
                    &context.cancellation,
                )?;
                let content = String::from_utf8(bytes)
                    .map_err(|_| "无法读取远程文本文件：文件不是 UTF-8 文本".to_string())?;
                let (next, applied) = if let Some(patch) = patch {
                    apply_remote_patch_text(&content, &patch, &context.cancellation)?
                } else {
                    let old_text = old_text.ok_or_else(|| {
                        "apply_patch 需要 patch，或同时提供 old_text 和 new_text".to_string()
                    })?;
                    let new_text = new_text.ok_or_else(|| {
                        "apply_patch 需要 new_text，或同时提供 old_text 和 patch".to_string()
                    })?;
                    if old_text.is_empty() {
                        return Err("apply_patch 不接受空 old_text".to_string());
                    }
                    if content.match_indices(&old_text).count() != 1 {
                        return Err("apply_patch 要求 old_text 恰好匹配一次".to_string());
                    }
                    (content.replacen(&old_text, &new_text, 1), 1)
                };
                cancelled(&context.cancellation)?;
                let mut output = retry_nonblocking_with_cancel(
                    "创建远程补丁文件",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || sftp.create(Path::new(&result_path)),
                )?;
                write_remote_bytes(&mut output, next.as_bytes(), &context.cancellation)
                    .map_err(|error| format!("写入远程补丁失败：{error}"))?;
                Ok(json!({
                    "path": result_path,
                    "appliedHunks": applied,
                    "oldBytes": content.len(),
                    "newBytes": next.len(),
                    "target": "ssh",
                }))
            }))
            .await,
        )?;
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "completed",
                "path": path,
                "itemPath": path,
                "message": "远程补丁应用完成",
            }));
        }
        Ok(result)
    }
}
