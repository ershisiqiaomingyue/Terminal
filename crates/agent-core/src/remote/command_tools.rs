use super::*;

pub(super) struct RemoteCommandTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteCommandTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("run_local_command", "在 SSH 工作区中执行命令，并实时返回远程 stdout/stderr。默认 120 秒；长时间构建请传 timeout_ms，最多 30 分钟。", json!({ "type": "object", "properties": { "command": { "type": "string" }, "args": { "type": "array", "items": { "type": "string" } }, "cwd": { "type": "string" }, "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 1800000 } }, "required": ["command"] })).requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let requested_command = command_line(&input)?;
        if is_forbidden_destructive_command_line(&requested_command) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
        }
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|value| remote_path(&context, &json!({"path": value})))
            .transpose()?
            .unwrap_or_else(|| context.root.clone());
        let (display_command, wrapped_command) = remote_command_scripts(&cwd, &requested_command);
        let process_id = Arc::new(Mutex::new(None));
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_REMOTE_COMMAND_SECONDS * 1000)
            .clamp(1_000, context.command_timeout_limit);
        let call_id = input
            .get("__agent_call_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let sink = context.output_sink.as_ref().map(|sink| {
            call_id
                .map(|value| sink.with_call_id(value))
                .unwrap_or_else(|| sink.clone())
        });
        if let Some(sink) = &sink {
            sink.emit_started("ssh", &[display_command.clone()], Path::new(&cwd));
        }
        if uses_openssh_fallback(&context.config) {
            let blocking_context = context.clone();
            let blocking_cwd = cwd.clone();
            let blocking_command = requested_command.clone();
            let blocking_display_command = display_command.clone();
            let blocking_sink = sink.clone();
            return tokio::task::spawn_blocking(move || {
                let cli_command =
                    format!("cd {} && {}", shell_escape(&blocking_cwd), blocking_command);
                let result = run_openssh_context_command_checked(
                    &blocking_context,
                    &cli_command,
                    None,
                    Duration::from_millis(timeout_ms),
                    blocking_sink.as_ref(),
                    &[blocking_cwd.as_str()],
                    "执行远程命令",
                );
                match result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout)
                            .chars()
                            .take(MAX_REMOTE_COMMAND_OUTPUT_BYTES)
                            .collect::<String>();
                        let stderr = String::from_utf8_lossy(&output.stderr)
                            .chars()
                            .take(MAX_REMOTE_COMMAND_OUTPUT_BYTES)
                            .collect::<String>();
                        let value = json!({
                            "command": blocking_display_command,
                            "exitCode": output.status.code(),
                            "success": output.status.success(),
                            "stdout": stdout,
                            "stderr": stderr,
                            "target": "ssh",
                            "backend": "openssh"
                        });
                        finish_remote_sink(blocking_sink.as_ref(), &value);
                        Ok(value)
                    }
                    Err(error) => {
                        if let Some(sink) = &blocking_sink {
                            let termination = if error.contains("取消") {
                                "cancelled"
                            } else {
                                "error"
                            };
                            sink.emit_remote_finished(
                                Some(termination),
                                json!({ "success": false }),
                                json!({ "error": error }),
                            );
                        }
                        Err(error)
                    }
                }
            })
            .await
            .map_err(|error| format!("等待系统 SSH 命令失败：{error}"))?;
        }
        let command = wrapped_command;
        let error_sink = sink.clone();
        let result = await_remote(spawn_remote(context.clone(), move |session, context| {
            session.set_blocking(false);
            let channel =
                retry_nonblocking("打开 SSH 命令通道", Duration::from_secs(10), || {
                    session.channel_session()
                })?;
            let result = execute_remote_command(
                channel,
                &session,
                &command,
                context,
                sink.as_ref(),
                Duration::from_millis(timeout_ms),
                Some(process_id.clone()),
            )?;
            let mut result = result;
            if let Value::Object(object) = &mut result {
                object.insert("command".to_string(), json!(display_command));
            }
            finish_remote_sink(sink.as_ref(), &result);
            Ok(result)
        }))
        .await;
        if let Err(error) = &result {
            if let Some(sink) = &error_sink {
                let termination = if error.contains("取消") {
                    "cancelled"
                } else {
                    "error"
                };
                sink.emit_remote_finished(
                    Some(termination),
                    json!({ "success": false }),
                    json!({ "error": error }),
                );
            }
        }
        let result = result?;
        Ok(result)
    }
}

pub(super) struct RemoteShellTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_local_shell",
            "在 SSH 工作区中通过远程 shell 执行一条命令，并实时返回输出。支持 bash、sh 和 zsh；需要初始化构建环境时，把环境初始化和后续命令放在同一次调用中；长时间构建请传 timeout_ms，最多 30 分钟。",
            json!({
                "type": "object",
                "properties": {
                    "shell": { "type": "string", "enum": ["bash", "sh", "zsh"] },
                    "command": { "type": "string" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 1800000 }
                },
                "required": ["shell", "command"]
            }),
        )
        .requiring_permission()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "run_local_shell 需要 command".to_string())?
            .to_string();
        let shell = input
            .get("shell")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| matches!(value.as_str(), "bash" | "sh" | "zsh"))
            .ok_or_else(|| "run_local_shell 的 shell 只支持 bash、sh 或 zsh".to_string())?;
        if is_forbidden_destructive_command_line(&command) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
        }
        let mut normalized = input.clone();
        if let Value::Object(object) = &mut normalized {
            object.insert("command".to_string(), json!(shell));
            object.insert("args".to_string(), json!(["-lc", command]));
        }
        RemoteCommandTool(self.0.clone()).execute(normalized).await
    }
}

pub(super) struct RemoteInteractiveShellTool(pub(super) SshToolContext);

#[async_trait]
impl ToolHandler for RemoteInteractiveShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_interactive_shell",
            "在 SSH 主机上申请 PTY 后执行需要终端环境的命令，并实时返回合并后的终端输出。可传 input 作为一次性交互输入；它不是后台会话，长任务请使用 timeout_ms。",
            json!({
                "type": "object",
                "properties": {
                    "shell": { "type": "string", "enum": ["bash", "sh", "zsh"] },
                    "command": { "type": "string" },
                    "input": { "type": "string" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 1800000 }
                },
                "required": ["shell", "command"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = self.0.clone();
        let shell = input
            .get("shell")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| matches!(value.as_str(), "bash" | "sh" | "zsh"))
            .ok_or_else(|| "run_interactive_shell 的 shell 只支持 bash、sh 或 zsh".to_string())?;
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "run_interactive_shell 需要 command".to_string())?;
        if is_forbidden_destructive_command_line(command) {
            return Err(crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
        }
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|value| remote_path(&context, &json!({"path": value})))
            .transpose()?
            .unwrap_or_else(|| context.root.clone());
        let remote_command = format!(
            "cd {} && exec {} -lc {}",
            shell_escape(&cwd),
            shell_escape(&shell),
            shell_escape(command)
        );
        let input_bytes = input
            .get("input")
            .and_then(Value::as_str)
            .map(|value| value.as_bytes().to_vec());
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(context.config.operation_timeout_secs.saturating_mul(1000))
            .clamp(1_000, context.command_timeout_limit);
        let call_id = input
            .get("__agent_call_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let sink = context.output_sink.as_ref().map(|sink| {
            call_id
                .map(|value| sink.with_call_id(value))
                .unwrap_or_else(|| sink.clone())
        });
        if let Some(sink) = &sink {
            sink.emit_started(
                "ssh-pty",
                &[format!("cd {} && {}", cwd, command)],
                Path::new(&cwd),
            );
        }
        let error_sink = sink.clone();
        let result = if uses_openssh_fallback(&context.config) {
            tokio::task::spawn_blocking(move || {
                run_openssh_pty_command(
                    &context,
                    &remote_command,
                    input_bytes.as_deref(),
                    Duration::from_millis(timeout_ms),
                    sink.as_ref(),
                )
            })
            .await
            .map_err(|error| format!("SSH 终端后台任务失败: {error}"))?
        } else {
            await_remote(spawn_remote(context.clone(), move |session, context| {
                session.set_blocking(false);
                let channel =
                    retry_nonblocking("打开 SSH PTY 通道", Duration::from_secs(10), || {
                        session.channel_session()
                    })?;
                let result = execute_remote_pty_command(
                    channel,
                    &session,
                    &remote_command,
                    input_bytes.as_deref(),
                    context,
                    sink.as_ref(),
                    Duration::from_millis(timeout_ms),
                )?;
                Ok(result)
            }))
            .await
        };
        if let Ok(value) = &result {
            finish_remote_sink(error_sink.as_ref(), value);
        }
        if let Err(error) = &result {
            if let Some(sink) = &error_sink {
                sink.emit_remote_finished(
                    Some(if error.contains("取消") {
                        "cancelled"
                    } else {
                        "error"
                    }),
                    json!({ "success": false }),
                    json!({ "error": error }),
                );
            }
        }
        result
    }
}

pub(super) struct RemoteGitTool(pub(super) SshToolContext, pub(super) bool);

#[async_trait]
impl ToolHandler for RemoteGitTool {
    fn definition(&self) -> ToolDefinition {
        let name = if self.1 {
            "get_git_diff"
        } else {
            "get_git_status"
        };
        ToolDefinition::new(
            name,
            "读取 SSH 工作区的 Git 状态或差异。",
            if self.1 {
                json!({
                    "type": "object",
                    "properties": {
                        "cwd": { "type": "string" },
                        "path": { "type": "string" }
                    }
                })
            } else {
                json!({
                    "type": "object",
                    "properties": {
                        "cwd": { "type": "string" }
                    }
                })
            },
        )
        .read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|value| remote_path(&self.0, &json!({ "path": value })))
            .transpose()?
            .unwrap_or_else(|| self.0.root.clone());
        let command = if self.1 {
            let path = input
                .get("path")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    let path_input =
                        if value.trim_start().starts_with('/') || is_symbolic_remote_path(value) {
                            value.trim().to_string()
                        } else {
                            format!("{cwd}/{}", value.trim())
                        };
                    remote_path(&self.0, &json!({ "path": path_input }))
                })
                .transpose()?;
            path.map(|path| {
                let escaped = shell_escape(&path);
                format!("git diff --stat -- {escaped} && git diff -- {escaped}")
            })
            .unwrap_or_else(|| "git diff --stat && git diff".to_string())
        } else {
            "git status --short --branch".to_string()
        };
        let mut value = input;
        if let Value::Object(object) = &mut value {
            object.insert("shell".to_string(), json!("sh"));
            object.insert("command".to_string(), json!(command));
            object.insert("cwd".to_string(), json!(cwd));
        }
        RemoteShellTool(self.0.clone()).execute(value).await
    }
}

pub(super) struct RemoteProjectCheckTool(pub(super) SshToolContext);

#[derive(Debug, Clone)]
pub(super) struct RemoteProjectCheckPlan {
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) detected: String,
}

fn configured_remote_check(bytes: &[u8], kind: &str) -> Result<RemoteProjectCheckPlan, String> {
    let entry = crate::project_checks::configured_check_bytes(bytes, kind)?;
    Ok(RemoteProjectCheckPlan {
        command: entry.command,
        args: entry.args,
        detected: ".agent-checks.json".into(),
    })
}

fn go_project_check(kind: &str) -> RemoteProjectCheckPlan {
    RemoteProjectCheckPlan {
        command: "go".into(),
        args: vec![
            if kind == "check" { "vet" } else { kind }.into(),
            "./...".into(),
        ],
        detected: "go.mod".into(),
    }
}

#[cfg(test)]
mod project_check_tests {
    use super::*;

    #[test]
    fn remote_checks_keep_custom_executables_and_arguments_literal() {
        let plan = configured_remote_check(br#"{"checks":{"custom":{"command":"/tools/compiler dir/check","args":["literal;$value","build folder"]}}}"#, "custom").unwrap();
        let command =
            command_line(&json!({"command":shell_escape(&plan.command),"args":plan.args})).unwrap();
        assert_eq!(
            command,
            "'/tools/compiler dir/check' 'literal;$value' 'build folder'"
        );
        assert!(configured_remote_check(&vec![b' '; 128 * 1024 + 1], "test").is_err());
        assert_eq!(go_project_check("check").args, vec!["vet", "./..."]);
        assert_eq!(go_project_check("test").args, vec!["test", "./..."]);
    }
}

#[async_trait]
impl ToolHandler for RemoteProjectCheckTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_project_check",
            "在 SSH 工作区中执行 .agent-checks.json 声明的任意语言检查入口，或检测 Cargo、package.json、Go module、Gradle。命令和参数独立传递；需要执行权限。",
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "项目配置中的检查名称，或 build/test/check" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 1800000 }
                },
                "required": ["kind"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "run_project_check 需要检查名称".to_string())?
            .to_string();
        let context = self.0.clone();
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .map(|value| remote_path(&context, &json!({ "path": value })))
            .transpose()?
            .unwrap_or_else(|| context.root.clone());
        let sink = remote_output_sink_for_input(&context, &input);
        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": cwd,
                "itemPath": cwd,
                "message": "正在检测远程项目检查入口",
            }));
        }

        let plan = if uses_openssh_fallback(&context.config) {
            let timeout =
                Duration::from_secs(context.config.operation_timeout_secs.clamp(10, 1800));
            let child_path = |name: &str| {
                if cwd == "/" {
                    format!("/{name}")
                } else {
                    format!("{}/{}", cwd.trim_end_matches('/'), name)
                }
            };
            let exists = |name: &str| -> Result<bool, String> {
                let path = child_path(name);
                let output = run_openssh_context_command_checked(
                    &context,
                    &format!(
                        "if [ -f {path} ] && [ ! -L {path} ]; then exit 0; else exit 1; fi",
                        path = shell_escape(&path)
                    ),
                    None,
                    timeout,
                    None,
                    &[path.as_str()],
                    "检查远程项目文件",
                )?;
                match output.status.code() {
                    Some(0) => Ok(true),
                    Some(1) => Ok(false),
                    _ => Err(format!(
                        "检查远程项目文件失败：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )),
                }
            };
            if exists(".agent-checks.json")? {
                let path = child_path(".agent-checks.json");
                let output = run_openssh_context_command_checked(
                    &context,
                    &format!(
                        "head -c 131073 < {} | base64 | tr -d '\n'",
                        shell_escape(&path)
                    ),
                    None,
                    timeout,
                    None,
                    &[path.as_str()],
                    "读取远程项目检查配置",
                )?;
                if !output.status.success() {
                    return Err("读取远程项目检查配置失败".into());
                }
                let encoded = String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .collect::<String>();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(|e| e.to_string())?;
                configured_remote_check(&bytes, &kind)
            } else if !matches!(kind.as_str(), "build" | "test" | "check") {
                Err("请在 .agent-checks.json 声明该检查入口".into())
            } else if exists("Cargo.toml")? {
                Ok::<RemoteProjectCheckPlan, String>(RemoteProjectCheckPlan {
                    command: "cargo".to_string(),
                    args: vec![kind.clone()],
                    detected: "Cargo.toml".to_string(),
                })
            } else if exists("package.json")? {
                let package_path = child_path("package.json");
                let output = run_openssh_context_command_checked(
                    &context,
                    &format!(
                        "base64 < {path} 2>/dev/null | tr -d '\\n'",
                        path = shell_escape(&package_path)
                    ),
                    None,
                    timeout,
                    None,
                    &[package_path.as_str()],
                    "读取远程 package.json",
                )?;
                if !output.status.success() {
                    return Err(format!(
                        "无法读取远程 package.json：{}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                let encoded = String::from_utf8_lossy(&output.stdout)
                    .chars()
                    .filter(|value| !value.is_whitespace())
                    .collect::<String>();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(|error| format!("远程 package.json 解码失败：{error}"))?;
                if bytes.len() > 256 * 1024 {
                    return Err("远程 package.json 不能超过 256 KB".to_string());
                }
                let package: Value = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("远程 package.json 格式无效：{error}"))?;
                let scripts = package.get("scripts").and_then(Value::as_object);
                let script = if scripts.and_then(|values| values.get(&kind)).is_some() {
                    kind.clone()
                } else if kind == "check"
                    && scripts.and_then(|values| values.get("build")).is_some()
                {
                    "build".to_string()
                } else {
                    return Err(format!("远程 package.json 未定义 {} 脚本", kind));
                };
                Ok(RemoteProjectCheckPlan {
                    command: "npm".to_string(),
                    args: vec!["run".to_string(), script],
                    detected: "package.json".to_string(),
                })
            } else if exists("go.mod")? {
                Ok(go_project_check(&kind))
            } else {
                let task = match kind.as_str() {
                    "build" => "build",
                    "test" => "test",
                    _ => "check",
                };
                if exists("gradlew")? {
                    Ok(RemoteProjectCheckPlan {
                        command: "sh".to_string(),
                        args: vec!["./gradlew".to_string(), task.to_string()],
                        detected: "gradlew".to_string(),
                    })
                } else if exists("gradlew.bat")? {
                    Err("远程目录只有 gradlew.bat。SSH 远程项目检查按 Unix shell 执行，请使用 run_local_shell 连接 Windows OpenSSH，或提供 Unix 版 gradlew。".to_string())
                } else {
                    Err(
                        "未检测到已声明的远程项目检查入口，请读取项目脚本或配置 .agent-checks.json"
                            .to_string(),
                    )
                }
            }
        } else {
            let plan_cwd = cwd.clone();
            let plan_kind = kind.clone();
            await_remote(spawn_remote(context.clone(), move |session, context| {
                let sftp = retry_nonblocking_with_cancel(
                    "初始化远程项目检查",
                    Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                    &context.cancellation,
                    || session.sftp(),
                )?;
                let exists = |name: &str| -> Result<bool, String> {
                    let path = format!("{plan_cwd}/{name}");
                    match retry_nonblocking_with_cancel(
                        "检查远程项目文件",
                        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                        &context.cancellation,
                        || sftp.lstat(Path::new(&path)),
                    ) {
                        Ok(stat) => Ok(!matches!(
                            stat.file_type(),
                            ssh2::FileType::Directory | ssh2::FileType::Symlink
                        )),
                        Err(error)
                            if error.to_ascii_lowercase().contains("no such")
                                || error.to_ascii_lowercase().contains("not found") =>
                        {
                            Ok(false)
                        }
                        Err(error) => Err(error),
                    }
                };
                if exists(".agent-checks.json")? {
                    let path = format!("{plan_cwd}/.agent-checks.json");
                    let mut file = retry_nonblocking_with_cancel(
                        "打开远程项目检查配置", Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                        &context.cancellation, || sftp.open(Path::new(&path)),
                    )?;
                    let bytes = read_remote_bytes(&mut file, 128 * 1024 + 1, &context.cancellation)?;
                    return configured_remote_check(&bytes, &plan_kind);
                }
                if !matches!(plan_kind.as_str(), "build" | "test" | "check") {
                    return Err("请在 .agent-checks.json 声明该检查入口".into());
                }
                if exists("Cargo.toml")? {
                    return Ok(RemoteProjectCheckPlan {
                        command: "cargo".to_string(),
                        args: vec![plan_kind],
                        detected: "Cargo.toml".to_string(),
                    });
                }
                if exists("package.json")? {
                    let package_path = format!("{plan_cwd}/package.json");
                    let mut file = retry_nonblocking_with_cancel(
                        "打开远程 package.json",
                        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                        &context.cancellation,
                        || sftp.open(Path::new(&package_path)),
                    )
                    .map_err(|error| format!("无法读取远程 package.json：{error}"))?;
                    let bytes = read_remote_bytes(&mut file, 256 * 1024, &context.cancellation)?;
                    let package: Value = serde_json::from_slice(&bytes)
                        .map_err(|error| format!("远程 package.json 格式无效：{error}"))?;
                    let scripts = package.get("scripts").and_then(Value::as_object);
                    let script = if scripts.and_then(|values| values.get(&plan_kind)).is_some() {
                        plan_kind.clone()
                    } else if plan_kind == "check"
                        && scripts.and_then(|values| values.get("build")).is_some()
                    {
                        "build".to_string()
                    } else {
                        return Err(format!("远程 package.json 未定义 {} 脚本", plan_kind));
                    };
                    return Ok(RemoteProjectCheckPlan {
                        command: "npm".to_string(),
                        args: vec!["run".to_string(), script],
                        detected: "package.json".to_string(),
                    });
                }
                if exists("go.mod")? { return Ok(go_project_check(&plan_kind)); }
                let task = match plan_kind.as_str() {
                    "build" => "build",
                    "test" => "test",
                    _ => "check",
                };
                if exists("gradlew")? {
                    return Ok(RemoteProjectCheckPlan {
                        command: "sh".to_string(),
                        args: vec!["./gradlew".to_string(), task.to_string()],
                        detected: "gradlew".to_string(),
                    });
                }
                if exists("gradlew.bat")? {
                    return Err("远程目录只有 gradlew.bat。SSH 远程项目检查按 Unix shell 执行，请使用 run_local_shell 连接 Windows OpenSSH，或提供 Unix 版 gradlew。".to_string());
                }
                Err("未检测到已声明的远程项目检查入口，请读取项目脚本或配置 .agent-checks.json".to_string())
            }))
            .await
        }?;

        if let Some(sink) = &sink {
            sink.emit_remote_progress(json!({
                "stage": "scanning",
                "path": cwd,
                "itemPath": cwd,
                "message": format!("已检测到 {}，准备执行 {} 检查", plan.detected, kind),
            }));
        }
        let mut command_input = input;
        if let Value::Object(object) = &mut command_input {
            object.insert("command".to_string(), json!(shell_escape(&plan.command)));
            object.insert("args".to_string(), json!(plan.args));
            object.insert("cwd".to_string(), json!(cwd));
        }
        let output = RemoteCommandTool(context).execute(command_input).await?;
        let mut result = output;
        if let Value::Object(object) = &mut result {
            object.insert("kind".to_string(), json!(kind));
            object.insert("detected".to_string(), json!(plan.detected));
            let diagnostics = [
                object
                    .get("stderr")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                object
                    .get("stdout")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ]
            .into_iter()
            .flat_map(str::lines)
            .filter(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("error") || line.contains("warning") || line.contains("failed")
            })
            .take(64)
            .map(str::to_string)
            .collect::<Vec<_>>();
            object.insert("diagnostics".to_string(), json!(diagnostics));
        }
        Ok(result)
    }
}
