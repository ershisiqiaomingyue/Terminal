use super::super::context::{LocalToolContext, MAX_FILE_WRITE_BYTES};
use super::super::utilities::{cancelled, is_allowed_tool_path};
use crate::{is_explicitly_selected_root, resolve_selected_path, ToolDefinition, ToolHandler};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

pub(in crate::local_tools) struct WriteLocalFileTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for WriteLocalFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_local_file",
            "Write bounded text to a user-selected local file, or an explicitly requested external file when full access is enabled. Requires permission unless full access is active.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string", "maxLength": MAX_FILE_WRITE_BYTES }
                },
                "required": ["path", "content"]
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
            .map(PathBuf::from)
            .map(|path| resolve_selected_path(&path, &context.roots))
            .ok_or_else(|| "write_local_file 需要 path".to_string())?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "write_local_file 需要 content".to_string())?;
        if content.len() > MAX_FILE_WRITE_BYTES {
            return Err(format!("单次写入不能超过 {} 字节", MAX_FILE_WRITE_BYTES));
        }
        if !is_allowed_tool_path(
            &path,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err("只能修改用户明确选择的文件或目录".to_string());
        }
        if let Some(parent) = path.parent() {
            if !is_allowed_tool_path(
                parent,
                &context.roots,
                &context.excluded_paths,
                context.allow_external_paths,
            ) && !is_explicitly_selected_root(&path, &context.roots)
            {
                return Err("文件父目录不在用户选择范围内".to_string());
            }
        }
        fs::write(&path, content).map_err(|error| format!("写入文件失败: {error}"))?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": content.len() }))
    }
}
