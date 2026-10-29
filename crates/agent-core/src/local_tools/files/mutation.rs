use super::super::context::LocalToolContext;
use super::super::utilities::{cancelled, is_allowed_tool_path};
use super::super::wsl::{execute_wsl_shell, WslPathMapper};
use crate::{
    normalized_policy_path, policy_path_key, resolve_selected_path, ToolDefinition, ToolHandler,
    WslRuntimeConfig,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn mutation_path(input: &Value, key: &str, roots: &[String]) -> Result<PathBuf, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| resolve_selected_path(Path::new(value), roots))
        .ok_or_else(|| format!("{key} 需要 path"))
}

fn ensure_mutation_path_allowed(
    path: &Path,
    context: &LocalToolContext,
    label: &str,
) -> Result<(), String> {
    if !is_allowed_tool_path(
        path,
        &context.roots,
        &context.excluded_paths,
        context.allow_external_paths,
    ) {
        return Err(format!("只能在用户明确选择的范围内{label}"));
    }
    if let Some(parent) = path.parent() {
        if !is_allowed_tool_path(
            parent,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err(format!("{label}的父目录不在用户选择范围内"));
        }
    }
    Ok(())
}

fn same_policy_path(left: &Path, right: &Path) -> bool {
    policy_path_key(&normalized_policy_path(left))
        == policy_path_key(&normalized_policy_path(right))
}

pub(in crate::local_tools) struct CreateDirectoryTool(pub(in crate::local_tools) LocalToolContext);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AllowPermission, ToolExecutionError, ToolRegistry};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[tokio::test]
    async fn file_management_tools_are_scoped_and_non_recursive() {
        let root = super::super::super::tests::temporary_root();
        fs::create_dir_all(&root).expect("test root should be created");
        let source = root.join("source.txt");
        let destination = root.join("nested").join("copy.txt");
        fs::write(&source, "remote-ready").expect("source should be written");
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        crate::register_local_tools(
            &mut registry,
            LocalToolContext::new(
                vec![root.to_string_lossy().to_string()],
                Vec::new(),
                false,
                cancellation,
                None,
            ),
        )
        .expect("file tools should register");

        registry
            .execute(
                "create_directory",
                json!({ "path": "nested" }),
                &AllowPermission,
            )
            .await
            .expect("directory should be created");
        registry
            .execute(
                "copy_file",
                json!({ "source": "source.txt", "destination": "nested/copy.txt" }),
                &AllowPermission,
            )
            .await
            .expect("file should be copied");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "remote-ready");

        registry
            .execute(
                "move_file",
                json!({ "source": "nested/copy.txt", "destination": "moved.txt" }),
                &AllowPermission,
            )
            .await
            .expect("file should be moved");
        assert!(root.join("moved.txt").is_file());
        assert!(!destination.exists());

        registry
            .execute(
                "delete_file",
                json!({ "path": "moved.txt" }),
                &AllowPermission,
            )
            .await
            .expect("file should be deleted");
        assert!(!root.join("moved.txt").exists());
        let error = registry
            .execute("delete_file", json!({ "path": "nested" }), &AllowPermission)
            .await
            .expect_err("directory deletion must be rejected");
        assert!(
            matches!(error, ToolExecutionError::Handler(message) if message.contains("不支持删除目录"))
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[async_trait]
impl ToolHandler for CreateDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "create_directory",
            "Create one directory inside the selected local workspace. Do not create parent trees implicitly.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let path = mutation_path(&input, "path", &context.roots)?;
        ensure_mutation_path_allowed(&path, context, "创建目录")?;
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                return Ok(
                    json!({ "path": path.to_string_lossy(), "created": false, "alreadyExists": true }),
                );
            }
            Ok(_) => return Err("目标路径已经是文件，不能创建目录".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法检查目录：{error}")),
        }
        let parent = path.parent().ok_or_else(|| "目录父路径无效".to_string())?;
        if !parent.is_dir() {
            return Err("目录父路径不存在，请先创建上一级目录".to_string());
        }
        fs::create_dir(&path).map_err(|error| format!("创建目录失败：{error}"))?;
        Ok(json!({ "path": path.to_string_lossy(), "created": true, "alreadyExists": false }))
    }
}

pub(in crate::local_tools) struct CopyFileTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for CopyFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "copy_file",
            "Copy one explicitly selected local file to another explicitly selected path. Directories and implicit recursive copies are not supported.",
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
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let source = mutation_path(&input, "source", &context.roots)?;
        let destination = mutation_path(&input, "destination", &context.roots)?;
        ensure_mutation_path_allowed(&source, context, "复制文件")?;
        ensure_mutation_path_allowed(&destination, context, "复制文件")?;
        if same_policy_path(&source, &destination) {
            return Err("复制源文件和目标文件不能相同".to_string());
        }
        if !fs::metadata(&source)
            .map_err(|error| format!("无法读取复制源文件：{error}"))?
            .is_file()
        {
            return Err("copy_file 只支持普通文件，不支持目录".to_string());
        }
        if destination.exists()
            && !input
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Err("目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| "目标父路径无效".to_string())?;
        if !parent.is_dir() {
            return Err("目标父目录不存在，请先使用 create_directory".to_string());
        }
        let bytes =
            fs::copy(&source, &destination).map_err(|error| format!("复制文件失败：{error}"))?;
        Ok(
            json!({ "source": source.to_string_lossy(), "destination": destination.to_string_lossy(), "bytes": bytes, "overwritten": input.get("overwrite").and_then(Value::as_bool).unwrap_or(false) }),
        )
    }
}

pub(in crate::local_tools) struct MoveFileTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for MoveFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "move_file",
            "Move one explicitly selected local file to another explicitly selected path. Directories and implicit recursive moves are not supported.",
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
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let source = mutation_path(&input, "source", &context.roots)?;
        let destination = mutation_path(&input, "destination", &context.roots)?;
        ensure_mutation_path_allowed(&source, context, "移动文件")?;
        ensure_mutation_path_allowed(&destination, context, "移动文件")?;
        if same_policy_path(&source, &destination) {
            return Err("移动源文件和目标文件不能相同".to_string());
        }
        if !fs::metadata(&source)
            .map_err(|error| format!("无法读取移动源文件：{error}"))?
            .is_file()
        {
            return Err("move_file 只支持普通文件，不支持目录".to_string());
        }
        if destination.exists()
            && !input
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Err("目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| "目标父路径无效".to_string())?;
        if !parent.is_dir() {
            return Err("目标父目录不存在，请先使用 create_directory".to_string());
        }
        if destination.exists() {
            fs::remove_file(&destination).map_err(|error| format!("覆盖目标文件失败：{error}"))?;
        }
        fs::rename(&source, &destination).map_err(|error| format!("移动文件失败：{error}"))?;
        Ok(
            json!({ "source": source.to_string_lossy(), "destination": destination.to_string_lossy(), "moved": true }),
        )
    }
}

pub(in crate::local_tools) struct DeleteFileTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for DeleteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "delete_file",
            "Delete one explicitly selected local file or symlink. Never delete directories or recursively delete a tree.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let path = mutation_path(&input, "path", &context.roots)?;
        ensure_mutation_path_allowed(&path, context, "删除文件")?;
        if let Some(wsl) = context.wsl.clone() {
            return delete_wsl_file(context, wsl, &path).await;
        }
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| format!("无法读取删除目标：{error}"))?;
        if metadata.is_dir() {
            return Err(
                "delete_file 不支持删除目录或递归删除；请先明确提供目录管理需求".to_string(),
            );
        }
        fs::remove_file(&path).map_err(|error| format!("删除文件失败：{error}"))?;
        Ok(json!({ "path": path.to_string_lossy(), "deleted": true }))
    }
}

/// Delete a WSL target file. Files are normally reached through the
/// `\\wsl.localhost` UNC share, but that share can lose visibility of files
/// after `wsl.exe` processes have run (WSL vserver staleness, os error 3 even
/// though the file still exists in the distribution). When the UNC path is
/// stale or missing, verify through `wsl.exe test -e` and delete through
/// `wsl.exe rm -f` so the operation still succeeds.
async fn delete_wsl_file(
    context: &LocalToolContext,
    wsl: WslRuntimeConfig,
    path: &Path,
) -> Result<Value, String> {
    let unc_usable = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(
                "delete_file 不支持删除目录或递归删除；请先明确提供目录管理需求".to_string(),
            );
        }
        Ok(_) => true,
        Err(_) => false,
    };
    if unc_usable {
        match fs::remove_file(path) {
            Ok(()) => return Ok(json!({ "path": path.to_string_lossy(), "deleted": true })),
            Err(_) => {}
        }
    }
    let mapper = WslPathMapper::new(wsl.clone());
    let linux = mapper.from_unc_path(&path.to_string_lossy());
    let quoted = linux.replace('\'', "'\\''");
    // `test` rejects a `--` option terminator, so only `rm` keeps it.
    let command = format!("test -e '{quoted}' && rm -f -- '{quoted}'");
    let cwd = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let result = execute_wsl_shell(context, "bash", &command, &cwd, 60_000, None).await?;
    if result.status.success() {
        Ok(json!({ "path": path.to_string_lossy(), "deleted": true, "backend": "wsl.exe" }))
    } else {
        Err(format!(
            "删除文件失败：wsl.exe 未找到或无法删除目标（退出码 {}）",
            result.status.code().unwrap_or(-1)
        ))
    }
}
