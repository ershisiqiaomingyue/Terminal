use super::super::context::{LocalToolContext, MAX_COMMAND_TIMEOUT_MS, MAX_SHELL_COMMAND_CHARS};
use super::super::utilities::{command_uses_excluded_path, is_excluded_path};
use super::super::wsl::execute_wsl_shell;
use super::output_sink_for_input;
use super::resolve_command_executable;
use crate::{
    command_result_value, execute_process, is_allowed_windows_shell,
    is_forbidden_destructive_command_line, resolve_command_working_directory, shell_process_args,
    ToolDefinition, ToolHandler,
};
use async_trait::async_trait;
use serde_json::{json, Value};

pub(in crate::local_tools) struct RunLocalShellTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for RunLocalShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("run_local_shell", "Run one Windows shell command with explicit shell, command text, and selected workspace. Supported shells: powershell, pwsh, cmd, bash, wsl.", json!({"type":"object","properties":{"shell":{"type":"string","enum":["powershell","pwsh","cmd","bash","wsl"]},"command":{"type":"string","maxLength":MAX_SHELL_COMMAND_CHARS},"cwd":{"type":"string"},"timeout_ms":{"type":"integer","maximum":MAX_COMMAND_TIMEOUT_MS}},"required":["shell","command"]})).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let shell = input
            .get("shell")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| {
                if context.wsl.is_some() {
                    matches!(value.as_str(), "bash" | "sh" | "zsh" | "wsl")
                } else {
                    is_allowed_windows_shell(value)
                }
            })
            .ok_or_else(|| "run_local_shell 需要 powershell、pwsh、cmd、bash 或 wsl".to_string())?;
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "run_local_shell 需要 command".to_string())?;
        if is_forbidden_destructive_command_line(command) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
        }
        if command.chars().count() > MAX_SHELL_COMMAND_CHARS {
            return Err(format!(
                "shell 命令不能超过 {} 个字符",
                MAX_SHELL_COMMAND_CHARS
            ));
        }
        let cwd = resolve_command_working_directory(
            input.get("cwd").and_then(Value::as_str),
            &context.roots,
            context.allow_external_paths,
        )?;
        if is_excluded_path(&cwd, &context.excluded_paths) {
            return Err("Shell 工作目录位于 SELinux 配置的排除目录内".to_string());
        }
        if command_uses_excluded_path(command, &[], &cwd, &context.roots, &context.excluded_paths) {
            return Err("Shell 命令引用了 SELinux 配置的排除目录".to_string());
        }
        let executable = if context.wsl.is_some() {
            None
        } else {
            Some(resolve_command_executable(
                &shell,
                &cwd,
                &[],
                &context.excluded_paths,
                false,
            )?)
        };
        let args = if context.wsl.is_some() {
            Vec::new()
        } else {
            shell_process_args(&shell, command)?
        };
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000)
            .clamp(1_000, context.command_timeout_limit);
        let output_sink = output_sink_for_input(context.output_sink.as_ref(), &input);
        if let Some(sink) = output_sink.as_ref() {
            let display_args = if context.wsl.is_some() {
                vec!["-lc".to_string(), command.to_string()]
            } else {
                args.clone()
            };
            sink.emit_started(&shell, &display_args, &cwd);
        }
        let output = if context.wsl.is_some() {
            execute_wsl_shell(
                context,
                &shell,
                command,
                &cwd,
                timeout_ms,
                output_sink.as_ref(),
            )
            .await?
        } else {
            execute_process(
                executable.ok_or_else(|| "Shell 执行器未初始化".to_string())?,
                &args,
                &cwd,
                timeout_ms,
                context.cancellation.clone(),
                output_sink.as_ref(),
            )
            .await?
        };
        Ok(command_result_value(
            json!({"shell": shell, "command": command, "cwd": cwd.to_string_lossy()}),
            &output,
        ))
    }
}
