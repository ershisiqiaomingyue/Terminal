use super::super::context::{LocalToolContext, MAX_COMMAND_TIMEOUT_MS};
use super::super::utilities::{command_uses_excluded_path, is_excluded_path};
use super::super::wsl::execute_wsl_process;
use super::output_sink_for_input;
use super::resolve_command_executable;
use crate::{
    command_result_value, execute_process, is_forbidden_destructive_command,
    is_windows_shell_command, resolve_command_working_directory, ToolDefinition, ToolHandler,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;

pub(in crate::local_tools) struct RunProjectCheckTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for RunProjectCheckTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("run_project_check", "Run a project-owned check: .agent-checks.json maps named entries to an executable and argument array for any language. Otherwise detects Cargo, package.json, Go modules or Gradle. Inspect project scripts first; never pass shell text as a command.", json!({"type":"object","properties":{"kind":{"type":"string","description":"Named .agent-checks.json entry, otherwise build/test/check"},"cwd":{"type":"string"},"timeout_ms":{"type":"integer","maximum":MAX_COMMAND_TIMEOUT_MS}},"required":["kind"]})).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "run_project_check 需要检查入口名称".to_string())?;
        let cwd = resolve_command_working_directory(
            input.get("cwd").and_then(Value::as_str),
            &context.roots,
            context.allow_external_paths,
        )?;
        if is_excluded_path(&cwd, &context.excluded_paths) {
            return Err("项目检查工作目录位于 SELinux 配置的排除目录内".to_string());
        }
        let has_cargo = cwd.join("Cargo.toml").is_file();
        let has_package = cwd.join("package.json").is_file();
        let wrapper = if context.wsl.is_some() && cwd.join("gradlew").is_file() {
            Some("gradlew")
        } else if cfg!(windows) && cwd.join("gradlew.bat").is_file() {
            Some("gradlew.bat")
        } else if cwd.join("gradlew").is_file() {
            Some("gradlew")
        } else {
            None
        };
        let configured = crate::project_checks::configured_check(&cwd, kind)?;
        if configured.is_none() && !matches!(kind, "build" | "test" | "check") {
            return Err("未配置该检查入口，请读取项目脚本或 .agent-checks.json".into());
        }
        let (command, args) = if let Some(entry) = configured {
            (entry.command, entry.args)
        } else if has_cargo {
            ("cargo".to_string(), vec![kind.to_string()])
        } else if has_package {
            let package: Value = serde_json::from_str(
                &fs::read_to_string(cwd.join("package.json"))
                    .map_err(|error| format!("无法读取 package.json: {error}"))?,
            )
            .map_err(|error| format!("package.json 格式无效: {error}"))?;
            let scripts = package.get("scripts").and_then(Value::as_object);
            let script = if scripts.and_then(|values| values.get(kind)).is_some() {
                kind.to_string()
            } else if kind == "check" && scripts.and_then(|values| values.get("build")).is_some() {
                "build".into()
            } else {
                return Err(format!("package.json 未定义 {kind} 脚本"));
            };
            ("npm".to_string(), vec!["run".into(), script])
        } else if cwd.join("go.mod").is_file() {
            (
                "go".to_string(),
                vec![
                    if kind == "check" { "vet" } else { kind }.into(),
                    "./...".into(),
                ],
            )
        } else if let Some(wrapper) = wrapper {
            (
                wrapper.to_string(),
                vec![match kind {
                    "build" => "build",
                    "test" => "test",
                    _ => "check",
                }
                .into()],
            )
        } else {
            return Err("未检测到已声明的项目检查入口。请读取项目构建/测试脚本，用受控命令工具执行，或在 .agent-checks.json 配置任意语言的 command/args。".to_string());
        };
        let command = command.as_str();
        if is_windows_shell_command(command)
            || crate::controlled_tool_for_command(command).is_some()
        {
            return Err("项目检查入口应是构建/测试可执行文件；Shell 脚本请使用 run_local_shell 的显式命令与权限流程".into());
        }
        if is_forbidden_destructive_command(&json!({"command":command,"args":args})) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.into());
        }
        if command_uses_excluded_path(
            command,
            &args,
            &cwd,
            &context.roots,
            &context.excluded_paths,
        ) {
            return Err("项目检查参数引用了排除路径".into());
        }
        let output_sink = output_sink_for_input(context.output_sink.as_ref(), &input);
        if let Some(sink) = output_sink.as_ref() {
            sink.emit_started(command, &args, &cwd);
        }
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, context.command_timeout_limit);
        let output = if context.wsl.is_some() {
            let command = if command == "gradlew" {
                "./gradlew"
            } else {
                command
            };
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
            let executable = resolve_command_executable(
                command,
                &cwd,
                &context.roots,
                &context.excluded_paths,
                context.allow_external_paths,
            )?;
            // G3-A：run_project_check 只执行项目自身声明的构建/测试入口，
            // 本机实测受限令牌下 workspace 内的可执行文件会被加载器拒绝启动，
            // 因此该工具不降权运行；降权只作用于 `run_local_shell` 的任意命令。
            execute_process(
                executable,
                &args,
                &cwd,
                timeout_ms,
                context.cancellation.clone(),
                output_sink.as_ref(),
            )
            .await?
        };
        let diagnostics = String::from_utf8_lossy(&output.stderr)
            .lines()
            .chain(String::from_utf8_lossy(&output.stdout).lines())
            .filter(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("error") || line.contains("warning") || line.contains("failed")
            })
            .take(64)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut result = command_result_value(
            json!({"kind": kind, "command": command, "args": args, "cwd": cwd.to_string_lossy()}),
            &output,
        );
        if let Value::Object(object) = &mut result {
            object.insert("diagnostics".into(), json!(diagnostics));
        }
        Ok(result)
    }
}
