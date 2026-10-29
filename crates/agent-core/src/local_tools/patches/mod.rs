use super::context::LocalToolContext;
use super::utilities::{cancelled, is_allowed_tool_path};
use crate::{resolve_selected_path, ToolDefinition, ToolHandler};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(in crate::local_tools) struct ApplyPatchTool(pub(in crate::local_tools) LocalToolContext);

fn patch_hunk_start(header: &str, fallback: usize) -> Result<usize, String> {
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

fn apply_patch_text(
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
            let start = patch_hunk_start(raw_line, source_cursor + 1)?.saturating_sub(1);
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

#[async_trait]
impl ToolHandler for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "apply_patch",
            "Apply a bounded patch to a selected local file. Prefer patch with one or more @@ hunks; old_text/new_text remains available for one exact replacement. The operation validates every context and deletion line before writing.",
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
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .map(|path| resolve_selected_path(&path, &context.roots))
            .ok_or_else(|| "apply_patch 需要 path".to_string())?;
        if !is_allowed_tool_path(
            &path,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err("只能修改用户明确选择的文件或目录".to_string());
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("无法读取待修改文件 {}：{error}", path.display()))?;
        let (next, applied) = if let Some(patch) = input
            .get("patch")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            apply_patch_text(&content, patch, &context.cancellation)?
        } else {
            let old_text = input
                .get("old_text")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "apply_patch 需要 patch，或同时提供 old_text 和 new_text".to_string()
                })?;
            let new_text = input
                .get("new_text")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "apply_patch 需要 new_text，或同时提供 old_text 和 patch".to_string()
                })?;
            if old_text.is_empty() {
                return Err("apply_patch 不接受空 old_text".to_string());
            }
            let occurrences = content.match_indices(old_text).count();
            if occurrences != 1 {
                return Err(format!(
                    "apply_patch 要求 old_text 恰好匹配一次，当前匹配 {occurrences} 次"
                ));
            }
            (content.replacen(old_text, new_text, 1), 1)
        };
        cancelled(&context.cancellation)?;
        fs::write(&path, next.as_bytes()).map_err(|error| format!("写入补丁失败: {error}"))?;
        Ok(json!({
            "path": path.to_string_lossy(),
            "appliedHunks": applied,
            "oldBytes": content.len(),
            "newBytes": next.len(),
        }))
    }
}
