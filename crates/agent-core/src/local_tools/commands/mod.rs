use super::context::LocalToolContext;
use super::utilities::{command_uses_excluded_path, is_allowed_tool_path, is_excluded_path};
use super::wsl::execute_wsl_process;
use crate::{
    command_result_value, execute_process, is_allowed_command, is_forbidden_destructive_command,
    is_windows_shell_command, resolve_command_working_directory, CommandOutputSink, ToolDefinition,
    ToolHandler, MAX_COMMAND_TIMEOUT_MS,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub(in crate::local_tools) struct RunLocalCommandTool(pub(in crate::local_tools) LocalToolContext);

pub(super) fn output_sink_for_input(
    sink: Option<&CommandOutputSink>,
    input: &Value,
) -> Option<CommandOutputSink> {
    let call_id = input
        .get("__agent_call_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    sink.map(|value| value.with_call_id(call_id.to_string()))
}

fn command_candidates(command: &Path) -> Vec<PathBuf> {
    let mut names = vec![command.to_path_buf()];
    #[cfg(windows)]
    if command.extension().is_none() {
        let extensions = std::env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|extension| !extension.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);
        for extension in extensions {
            names.push(PathBuf::from(format!(
                "{}{}",
                command.to_string_lossy(),
                extension
            )));
        }
    }
    names
}

pub(super) fn resolve_command_executable(
    command: &str,
    cwd: &Path,
    roots: &[String],
    excluded_paths: &[String],
    allow_external_paths: bool,
) -> Result<PathBuf, String> {
    let command_path = Path::new(command);
    let has_path = command_path.is_absolute() || command.contains('\\') || command.contains('/');
    if has_path {
        if command_path.is_absolute() && !allow_external_paths {
            return Err(
                "命令路径必须是当前工作目录内的相对路径；外部程序请使用白名单中的裸命令名"
                    .to_string(),
            );
        }
        for candidate in command_candidates(command_path) {
            let candidate = cwd.join(candidate);
            if !is_allowed_tool_path(&candidate, roots, excluded_paths, allow_external_paths) {
                continue;
            }
            if fs::metadata(&candidate)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                return Ok(candidate);
            }
        }
        return Err(format!(
            "命令启动失败: 找不到可执行程序 `{command}`，不会通过 shell 执行"
        ));
    }
    let names = command_candidates(command_path);
    if command_path
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("gradlew"))
    {
        for name in &names {
            let candidate = cwd.join(name);
            if is_allowed_tool_path(&candidate, roots, excluded_paths, allow_external_paths)
                && fs::metadata(&candidate)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
            {
                return Ok(candidate);
            }
        }
    }
    // Tools shipped with the application take precedence over the machine PATH
    // so allowlisted bare names such as `adb` and `java` resolve on a clean
    // Windows install that never had an Android SDK.
    for directory in crate::network::child_environment_path_entries() {
        for name in &names {
            let candidate = directory.join(name);
            if fs::metadata(&candidate)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                return Ok(candidate);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in &names {
                let candidate = directory.join(name);
                if fs::metadata(&candidate)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(format!(
        "命令启动失败: 找不到可执行程序 `{command}`，不会通过 shell 执行"
    ))
}

#[async_trait]
impl ToolHandler for RunLocalCommandTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_local_command",
            "Run one allowlisted local engineering executable with explicit arguments. Do not use a shell or shell aliases; use the controlled file tools or get_working_directory instead.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "maximum": MAX_COMMAND_TIMEOUT_MS }
                },
                "required": ["command"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "run_local_command 需要 command".to_string())?;
        if is_forbidden_destructive_command(&input) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
        }
        if let Some(tool) = crate::controlled_tool_for_command(command) {
            return Err(format!(
                "不能通过 run_local_command 执行 `{command}`，请改用 {tool}"
            ));
        }
        if is_windows_shell_command(command) {
            return Err(format!(
                "不能通过 run_local_command 启动 shell `{command}`，请改用 run_local_shell"
            ));
        }
        if !is_allowed_command(command) {
            return Err("命令不在 Agent 允许列表中，且不会通过 shell 执行".to_string());
        }
        let args = input
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cwd = resolve_command_working_directory(
            input.get("cwd").and_then(Value::as_str),
            &context.roots,
            context.allow_external_paths,
        )?;
        if is_excluded_path(&cwd, &context.excluded_paths) {
            return Err("命令工作目录位于 SELinux 配置的排除目录内".to_string());
        }
        if command_uses_excluded_path(
            command,
            &args,
            &cwd,
            &context.roots,
            &context.excluded_paths,
        ) {
            return Err("命令参数引用了 SELinux 配置的排除目录".to_string());
        }
        let executable = if context.wsl.is_some() {
            None
        } else {
            Some(resolve_command_executable(
                command,
                &cwd,
                &context.roots,
                &context.excluded_paths,
                context.allow_external_paths,
            )?)
        };
        let output_sink = output_sink_for_input(context.output_sink.as_ref(), &input);
        if let Some(sink) = output_sink.as_ref() {
            sink.emit_started(command, &args, &cwd);
        }
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000)
            .clamp(1_000, context.command_timeout_limit);
        let output = if context.wsl.is_some() {
            execute_wsl_process(
                context,
                command,
                &args,
                &cwd,
                timeout_ms,
                output_sink.as_ref(),
            )
            .await?
        } else {
            execute_process(
                executable.ok_or_else(|| "WSL 命令执行器未初始化".to_string())?,
                &args,
                &cwd,
                timeout_ms,
                context.cancellation.clone(),
                output_sink.as_ref(),
            )
            .await?
        };
        Ok(command_result_value(
            json!({"command": command, "args": args, "cwd": cwd.to_string_lossy()}),
            &output,
        ))
    }
}

pub(in crate::local_tools) use checks::RunProjectCheckTool;
pub(in crate::local_tools) use git::{GetGitDiffTool, GetGitStatusTool};
pub(in crate::local_tools) use shell::RunLocalShellTool;

mod checks;
mod git;
mod shell;
