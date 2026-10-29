use super::context::{LocalToolContext, MAX_SEARCH_RESULTS, MAX_TREE_DEPTH};
use super::utilities::{
    cancelled, is_allowed_tool_path, is_excluded_path, selected_tool_path, skip_agent_directory,
};
use crate::{strip_windows_extended_prefix, ToolDefinition, ToolHandler};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn directory_tree_value(
    path: &Path,
    depth: usize,
    max_depth: usize,
    remaining: &mut usize,
    excluded_paths: &[String],
    cancellation: &Arc<AtomicBool>,
) -> Result<Value, String> {
    cancelled(cancellation)?;
    if is_excluded_path(path, excluded_paths) {
        return Ok(json!({
            "name": path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
            "path": path.to_string_lossy(),
            "kind": "excluded",
        }));
    }
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法读取路径 {}：{error}", path.display()))?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    if !metadata.is_dir() || depth >= max_depth || *remaining == 0 {
        return Ok(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "kind": if metadata.is_dir() { "directory" } else { "file" },
        }));
    }
    *remaining = remaining.saturating_sub(1);
    let mut children = fs::read_dir(path)
        .map_err(|error| format!("无法读取目录 {}：{error}", path.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|child| !skip_agent_directory(child))
        .filter(|child| !is_excluded_path(child, excluded_paths))
        .collect::<Vec<_>>();
    children.sort_by_key(|child| child.to_string_lossy().to_ascii_lowercase());
    let mut values = Vec::new();
    for child in children {
        if *remaining == 0 {
            break;
        }
        values.push(directory_tree_value(
            &child,
            depth + 1,
            max_depth,
            remaining,
            excluded_paths,
            cancellation,
        )?);
    }
    Ok(json!({
        "name": name,
        "path": path.to_string_lossy(),
        "kind": "directory",
        "children": values,
        "truncated": *remaining == 0,
    }))
}

pub(in crate::local_tools) struct ListLocalFilesTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for ListLocalFilesTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_local_files",
            "List the immediate children of a user-selected local directory, or an explicitly requested external directory when full access is enabled. Use this before reading a file from a directory; do not use ls or dir.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "extension": { "type": "string", "description": "Optional suffix filter such as .md" },
                    "max_entries": { "type": "integer", "maximum": 256 }
                }
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let path = selected_tool_path(&input, &context.roots, context.allow_external_paths)?;
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "无法读取目录元数据：{}：{error}。请使用工作区中实际存在的精确目录路径。",
                path.display()
            )
        })?;
        if !is_allowed_tool_path(
            &path,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err("只能列出用户明确选择的目录内容".to_string());
        }
        if !metadata.is_dir() {
            return Err("list_local_files 只支持目录".to_string());
        }
        let extension = input
            .get("extension")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(128)
            .clamp(1, MAX_SEARCH_RESULTS as u64) as usize;
        let mut entries = Vec::new();
        for child in fs::read_dir(&path).map_err(|error| format!("无法读取目录: {error}"))? {
            cancelled(&context.cancellation)?;
            let child = child.map_err(|error| format!("无法读取目录项: {error}"))?;
            let child_path = strip_windows_extended_prefix(&child.path());
            if is_excluded_path(&child_path, &context.excluded_paths) {
                continue;
            }
            let child_name = child.file_name().to_string_lossy().to_string();
            let child_metadata = child
                .metadata()
                .map_err(|error| format!("无法读取目录项元数据: {error}"))?;
            if let Some(extension) = extension.as_deref() {
                if !child_metadata.is_dir() && !child_name.to_ascii_lowercase().ends_with(extension)
                {
                    continue;
                }
            }
            entries.push(json!({
                "name": child_name,
                "path": child_path.to_string_lossy(),
                "kind": if child_metadata.is_dir() { "directory" } else { "file" },
                "size": if child_metadata.is_file() { child_metadata.len() } else { 0 },
            }));
        }
        entries.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .cmp(
                    &right
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                )
        });
        let truncated = entries.len() > max_entries;
        entries.truncate(max_entries);
        Ok(json!({
            "path": path.to_string_lossy(),
            "entries": entries,
            "truncated": truncated,
        }))
    }
}

pub(in crate::local_tools) struct SearchLocalFilesTool(pub(in crate::local_tools) LocalToolContext);
pub(in crate::local_tools) struct SearchLocalCodeTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for SearchLocalFilesTool {
    fn definition(&self) -> ToolDefinition {
        scanner::definition(false)
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        tokio::task::spawn_blocking(move || execute::execute(&context, input, false))
            .await
            .map_err(|error| format!("检索任务失败：{error}"))?
    }
}

#[async_trait]
impl ToolHandler for SearchLocalCodeTool {
    fn definition(&self) -> ToolDefinition {
        scanner::definition(true)
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        tokio::task::spawn_blocking(move || execute::execute(&context, input, true))
            .await
            .map_err(|error| format!("检索任务失败：{error}"))?
    }
}

pub(in crate::local_tools) struct DirectoryTreeTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for DirectoryTreeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_directory_tree",
            "Return a bounded directory tree for the selected local workspace.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_depth": { "type": "integer", "maximum": MAX_TREE_DEPTH },
                    "max_entries": { "type": "integer", "maximum": MAX_SEARCH_RESULTS }
                }
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let root = selected_tool_path(&input, &context.roots, context.allow_external_paths)?;
        if !is_allowed_tool_path(
            &root,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err("只能读取用户明确选择的目录".to_string());
        }
        let max_depth = input
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .clamp(1, MAX_TREE_DEPTH as u64) as usize;
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(256)
            .clamp(1, MAX_SEARCH_RESULTS as u64) as usize;
        let mut remaining = max_entries;
        let tree = directory_tree_value(
            &root,
            0,
            max_depth,
            &mut remaining,
            &context.excluded_paths,
            &context.cancellation,
        )?;
        Ok(json!({
            "path": root.to_string_lossy(),
            "tree": tree,
            "truncated": remaining == 0,
        }))
    }
}

pub(in crate::local_tools) mod execute;
mod scanner;
mod spec;
