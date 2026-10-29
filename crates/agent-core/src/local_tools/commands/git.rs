use super::super::context::LocalToolContext;
use super::super::utilities::{is_allowed_tool_path, is_excluded_path};
use super::super::wsl::{execute_wsl_process, WslPathMapper};
use super::output_sink_for_input;
use super::resolve_command_executable;
use crate::{
    execute_process, resolve_command_working_directory, resolve_selected_path,
    resolve_user_directory_alias, ToolDefinition, ToolHandler,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

pub(in crate::local_tools) struct GetGitStatusTool(pub(in crate::local_tools) LocalToolContext);
pub(in crate::local_tools) struct GetGitDiffTool(pub(in crate::local_tools) LocalToolContext);

pub(super) async fn run_git(
    context: &LocalToolContext,
    input: &Value,
    args: &[String],
) -> Result<crate::CommandExecutionResult, String> {
    let cwd = resolve_command_working_directory(
        input.get("cwd").and_then(Value::as_str),
        &context.roots,
        context.allow_external_paths,
    )?;
    if is_excluded_path(&cwd, &context.excluded_paths) {
        return Err("Git 工作目录位于 SELinux 配置的排除目录内".to_string());
    }
    let output_sink = output_sink_for_input(context.output_sink.as_ref(), input);
    if let Some(sink) = output_sink.as_ref() {
        sink.emit_started("git", args, &cwd);
    }
    if context.wsl.is_some() {
        execute_wsl_process(context, "git", args, &cwd, 30_000, output_sink.as_ref()).await
    } else {
        let executable = resolve_command_executable(
            "git",
            &cwd,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        )?;
        execute_process(
            executable,
            args,
            &cwd,
            30_000,
            context.cancellation.clone(),
            output_sink.as_ref(),
        )
        .await
    }
}

#[async_trait]
impl ToolHandler for GetGitStatusTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("get_git_status", "Return structured Git branch and working tree status for the selected workspace or an explicitly requested cwd in full access.", json!({"type":"object","properties":{"cwd":{"type":"string"}}})).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let args = vec!["status".into(), "--porcelain=v1".into(), "--branch".into()];
        let context = &self.0;
        let cwd = resolve_command_working_directory(
            input.get("cwd").and_then(Value::as_str),
            &context.roots,
            context.allow_external_paths,
        )?;
        let output = run_git(context, &input, &args).await?;
        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let mut lines = raw.lines();
        let branch = lines
            .next()
            .unwrap_or_default()
            .strip_prefix("## ")
            .unwrap_or_default()
            .to_string();
        let files = lines
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let status = line.get(..2).unwrap_or_default().trim().to_string();
                let path = line.get(3..).unwrap_or(line).trim().to_string();
                json!({"status": status, "path": path})
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"cwd": cwd.to_string_lossy(), "branch": branch, "dirty": !files.is_empty(), "files": files, "raw": raw, "exitCode": output.status.code(), "success": output.status.success()}),
        )
    }
}

#[async_trait]
impl ToolHandler for GetGitDiffTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("get_git_diff", "Return a bounded structured Git diff for the selected workspace or an explicitly requested cwd/path in full access.", json!({"type":"object","properties":{"cwd":{"type":"string"},"path":{"type":"string"}}})).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let cwd = resolve_command_working_directory(
            input.get("cwd").and_then(Value::as_str),
            &context.roots,
            context.allow_external_paths,
        )?;
        let mut args = vec!["diff".into(), "--no-ext-diff".into(), "--unified=3".into()];
        if let Some(path) = input
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let path = if Path::new(path).is_absolute()
                || resolve_user_directory_alias(Path::new(path)).is_some()
            {
                resolve_selected_path(Path::new(path), &context.roots)
            } else {
                cwd.join(path)
            };
            if !is_allowed_tool_path(
                &path,
                &context.roots,
                &context.excluded_paths,
                context.allow_external_paths,
            ) {
                return Err("Git diff 路径不在用户选择范围内".to_string());
            }
            let git_path = if let Some(config) = &context.wsl {
                WslPathMapper::new(config.clone()).from_unc_path(&path.to_string_lossy())
            } else {
                path.to_string_lossy().to_string()
            };
            args.extend(["--".into(), git_path]);
        }
        let output = run_git(context, &input, &args).await?;
        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        let files = diff
            .lines()
            .filter_map(|line| line.strip_prefix("diff --git "))
            .map(|line| {
                line.split_once(" b/")
                    .map(|(_, path)| path.to_string())
                    .unwrap_or_else(|| line.to_string())
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"cwd": cwd.to_string_lossy(), "files": files, "diff": diff, "exitCode": output.status.code(), "success": output.status.success(), "truncated": output.ordered_truncated}),
        )
    }
}
