use super::*;

pub(super) fn mcp_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("mcp__{server_id}__{tool_name}")
}

pub(super) struct McpAgentTool {
    pub(super) name: String,
    pub(super) server_id: String,
    pub(super) remote_name: String,
    pub(super) description: String,
    pub(super) input_schema: Value,
    pub(super) config: McpServerConfig,
    pub(super) cancellation: Arc<AtomicBool>,
}

impl From<ConnectedMcpTool> for McpAgentTool {
    fn from(binding: ConnectedMcpTool) -> Self {
        let remote_name = binding.tool.name;
        Self {
            name: mcp_tool_name(&binding.server_id, &remote_name),
            server_id: binding.server_id,
            remote_name,
            description: binding
                .tool
                .description
                .unwrap_or_else(|| "Call a connected MCP server tool.".to_string()),
            input_schema: binding.tool.input_schema,
            config: binding.config,
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl McpAgentTool {
    pub(super) fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = cancellation;
        self
    }
}

pub(super) async fn wait_for_mcp_cancellation(cancellation: Arc<AtomicBool>) {
    while !cancellation.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[async_trait]
impl ToolHandler for McpAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(&self.name, &self.description, self.input_schema.clone())
            .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        // Same boundary as plugin tools: the MCP server only knows the schema it
        // published, so host bookkeeping would be rejected as an unknown key.
        let mut input = input;
        agent_core::strip_host_tool_bookkeeping(&mut input);
        let result = tokio::select! {
            result = mcp_call_tool(&self.config, &self.remote_name, input) => {
                result.map_err(|error| format!("MCP server {}: {error}", self.server_id))?
            }
            _ = wait_for_mcp_cancellation(self.cancellation.clone()) => {
                return Err("请求已取消".to_string());
            }
        };
        Ok(json!({
            "content": result.content,
            "isError": result.is_error,
        }))
    }
}

pub(super) struct LspAgentTool {
    pub(super) name: String,
    pub(super) operation: LspOperation,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}

impl LspAgentTool {
    pub(super) fn description(&self) -> &'static str {
        match self.operation {
            LspOperation::Diagnostics => {
                "Return structured diagnostics using user-configured language servers for any mapped language. Configuration: tools/lsp-servers.json. If a server is missing, continue with source search/project checks."
            }
            LspOperation::Symbols => "Return document symbols for one selected source file.",
            LspOperation::Definition => {
                "Jump to the definition at a zero-based line and UTF-16 character position."
            }
            LspOperation::References => {
                "Find references at a zero-based line and UTF-16 character position."
            }
            LspOperation::Hover => {
                "Return the type/documentation the language server reports for the symbol at a zero-based line and UTF-16 character position. Read-only: it never writes files."
            }
            LspOperation::Completion => {
                "Return at most 50 completion candidates (label/kind/detail) at a zero-based line and UTF-16 character position. Read-only: it never writes files."
            }
            LspOperation::Rename => {
                "Return the WorkspaceEdit JSON for renaming the symbol at a zero-based line and UTF-16 character position. Read-only: the edit is never applied, so apply it with the host file tools if the user approves."
            }
            LspOperation::Format => {
                "Return the formatting edits JSON for one selected source file. Read-only: the edits are never applied, so apply them with the host file tools if the user approves."
            }
            LspOperation::DocumentSymbols => {
                "Return the symbol tree of one selected source file."
            }
            LspOperation::WorkspaceSymbols => {
                "Search workspace symbols through the language server; returns at most 50 matches."
            }
        }
    }

    /// The input schema grows with the operation; the four original diagnostics
    /// and navigation tools keep exactly the schema they always had.
    fn input_schema(&self) -> Value {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "line": { "type": "integer", "minimum": 0 },
                "character": { "type": "integer", "minimum": 0 }
            },
            "required": ["path"]
        });
        match self.operation {
            LspOperation::Rename => {
                schema["properties"]["newName"] = json!({ "type": "string", "minLength": 1 });
                schema["required"] = json!(["path", "newName"]);
            }
            LspOperation::WorkspaceSymbols => {
                schema["properties"]["query"] = json!({ "type": "string" });
                schema["required"] = json!(["path", "query"]);
            }
            _ => {}
        }
        schema
    }
}

#[async_trait]
impl ToolHandler for LspAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name.clone(), self.description(), self.input_schema()).read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        if !self.roots.iter().any(|root| {
            fs::metadata(root)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        }) {
            return Err("LSP 需要用户明确选择工作目录，单文件会话不会扩大读取范围".to_string());
        }
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .map(|path| agent_core::resolve_selected_path(&path, &self.roots))
            .ok_or_else(|| "LSP 工具需要 path".to_string())?;
        if !is_allowed_agent_path(&path, &self.roots, self.allow_external_paths) {
            return Err("LSP 路径不在用户选择范围内".to_string());
        }
        if !fs::metadata(&path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
        {
            return Err("LSP 只支持已存在的文件".to_string());
        }
        let workspace = crate::lsp::workspace_root(&path, &self.roots)?;
        let config_path = crate::user_storage::root()?.join("tools/lsp-servers.json");
        let params = crate::lsp::LspParams {
            line: input
                .get("line")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            character: input
                .get("character")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            query: input
                .get("query")
                .and_then(Value::as_str)
                .map(str::to_string),
            new_name: input
                .get("newName")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        execute_lsp(
            path,
            workspace,
            config_path,
            self.operation,
            params,
            self.cancellation.clone(),
        )
        .await
    }
}

pub(super) fn local_tool_registry(
    paths: &[String],
    excluded_paths: &[String],
    mcp_tools: Vec<ConnectedMcpTool>,
    cancellation: Arc<AtomicBool>,
    allow_external_paths: bool,
    output_sink: Option<CommandOutputSink>,
    ssh_context: Option<&agent_core::SshToolContext>,
    wsl_context: Option<&agent_core::LocalToolContext>,
    android_app: Option<&AppHandle>,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<ToolRegistry, String> {
    // Every handler captures this turn's cancellation token, output sink, and
    // remote context. Reusing a registry across turns lets a prior stop
    // cancel a later, unrelated tool call.
    let mut registry = ToolRegistry::new();
    if let Some(ssh_context) = ssh_context {
        agent_core::register_ssh_tools(
            &mut registry,
            ssh_context
                .with_allow_external_paths(allow_external_paths)
                .with_output_sink(output_sink.as_ref().map(|sink| sink.inner().clone()))
                .with_cancellation(cancellation.clone()),
        )?;
    } else {
        // WinScope and the local log/LSP tools are desktop-only. A WSL turn
        // still uses this branch for the shared file/process tools, but must
        // not be able to accidentally inspect the Windows workspace.
        if wsl_context.is_none() {
            registry
                .register_handler(InspectWinScopeTraceTool {
                    roots: paths.to_vec(),
                    allow_external_paths,
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(CaptureAndroidLogcatTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidLogcatTool {
                    app: android_app.cloned(),
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidTaskTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidArchiveHistoryTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            if let Some(app) = android_app {
                registry
                    .register_handler(StartAndroidArchiveTaskTool {
                        app: app.clone(),
                        roots: paths.to_vec(),
                        allow_external_paths,
                        cancellation: cancellation.clone(),
                    })
                    .map_err(|error| error.to_string())?;
                registry
                    .register_handler(AndroidArchiveReadTool {
                        app: app.clone(),
                        roots: paths.to_vec(),
                        allow_external_paths,
                        cancellation: cancellation.clone(),
                    })
                    .map_err(|error| error.to_string())?;
                registry
                    .register_handler(AndroidArchiveWriteTool {
                        app: app.clone(),
                        roots: paths.to_vec(),
                        allow_external_paths,
                        cancellation: cancellation.clone(),
                    })
                    .map_err(|error| error.to_string())?;
                registry
                    .register_handler(AndroidAdbReadTool {
                        app: app.clone(),
                        cancellation: cancellation.clone(),
                    })
                    .map_err(|error| error.to_string())?;
                registry
                    .register_handler(AndroidAdbActionTool {
                        app: app.clone(),
                        roots: paths.to_vec(),
                        allow_external_paths,
                        cancellation: cancellation.clone(),
                    })
                    .map_err(|error| error.to_string())?;
            }
            registry
                .register_handler(QueryPerfettoTraceTool {
                    app: android_app.cloned(),
                    roots: paths.to_vec(),
                    allow_external_paths,
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidDiagnosticsTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidBugreportTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAospBuildTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidSimpleperfTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidNativeSymbolizerTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidMethodProfileTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidHeapDumpTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidBenchmarkTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(QueryAndroidDeviceTool {
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(ReadLogRangeTool {
                    paths: paths.to_vec(),
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(SearchLocalLogsTool {
                    paths: paths.to_vec(),
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
            registry
                .register_handler(RebuildTimelineTool {
                    paths: paths.to_vec(),
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
        }
        let local_context = if let Some(context) = wsl_context {
            context
                .with_allow_external_paths(allow_external_paths)
                .with_output_sink(output_sink.as_ref().map(|sink| sink.inner().clone()))
                .with_cancellation(cancellation.clone())
        } else {
            agent_core::LocalToolContext::new(
                paths.to_vec(),
                excluded_paths.to_vec(),
                allow_external_paths,
                cancellation.clone(),
                output_sink.as_ref().map(|sink| sink.inner().clone()),
            )
        };
        agent_core::register_local_tools(&mut registry, local_context.clone())?;
        registry
            .register_handler(UpdateTaskPlanTool {
                app: android_app.cloned(),
                session_id: session_id.map(str::to_string),
                turn_id: turn_id.map(str::to_string),
            })
            .map_err(|error| error.to_string())?;
        // G1：goal 一等公民工具——跨 turn 的目标生命周期（创建/查询/推进）。
        registry
            .register_handler(GoalTools {
                app: android_app.cloned(),
                session_id: session_id.map(str::to_string),
                remaining_depth: super::application_tools::MAX_SUBAGENT_DEPTH,
            })
            .map_err(|error| error.to_string())?;
    }
    if let (Some(app), Some(session), Some(turn)) = (android_app, session_id, turn_id) {
        let context = if let Some(ssh) = ssh_context {
            agent_core::background::BackgroundShellContext::Ssh(
                ssh.with_allow_external_paths(allow_external_paths),
            )
        } else {
            let local = wsl_context.cloned().unwrap_or_else(|| {
                agent_core::LocalToolContext::new(
                    paths.to_vec(),
                    excluded_paths.to_vec(),
                    allow_external_paths,
                    cancellation.clone(),
                    None,
                )
            });
            agent_core::background::BackgroundShellContext::Local(
                local.with_allow_external_paths(allow_external_paths),
            )
        };
        let source = registry.clone();
        registry
            .replace_handler(background_shell::BackgroundShellTool {
                app: app.clone(),
                session_id: session.into(),
                turn_id: turn.into(),
                source,
                context,
                cancellation: cancellation.clone(),
            })
            .map_err(|e| e.to_string())?;
    }
    for tool in mcp_tools {
        registry
            .register_handler(McpAgentTool::from(tool).with_cancellation(cancellation.clone()))
            .map_err(|error| error.to_string())?;
    }
    if ssh_context.is_none() && wsl_context.is_none() {
        for (name, operation) in [
            ("lsp_diagnostics", LspOperation::Diagnostics),
            ("lsp_symbols", LspOperation::Symbols),
            ("lsp_definition", LspOperation::Definition),
            ("lsp_references", LspOperation::References),
            ("lsp_hover", LspOperation::Hover),
            ("lsp_completion", LspOperation::Completion),
            ("lsp_rename", LspOperation::Rename),
            ("lsp_format", LspOperation::Format),
            ("lsp_document_symbols", LspOperation::DocumentSymbols),
            ("lsp_workspace_symbols", LspOperation::WorkspaceSymbols),
        ] {
            registry
                .register_handler(LspAgentTool {
                    name: name.to_string(),
                    operation,
                    roots: paths.to_vec(),
                    allow_external_paths,
                    cancellation: cancellation.clone(),
                })
                .map_err(|error| error.to_string())?;
        }
    }

    if ssh_context.is_none() && wsl_context.is_none() {
        if let (Some(session), Some(turn)) = (session_id, turn_id) {
            crate::plugin_host::register_tools(&mut registry, session, turn, cancellation)?;
        }
    }
    Ok(registry)
}

pub(super) fn register_environment_aliases(
    registry: &mut ToolRegistry,
    source: &ToolRegistry,
    environment: AgentToolEnvironment,
    description: &str,
) -> Result<(), String> {
    let names = source
        .list_public()
        .into_iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    for name in names {
        registry
            .register_alias_from(
                format!("{}__{name}", environment.prefix()),
                source,
                &name,
                description,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 对话绑定的附加主机：为每台已连通的主机建一份独立的 `ssh` 工具注册表，
/// 让 `ssh__*` 别名可以通过可选 `target` 参数路由过去。
fn bound_ssh_registries(
    bound_ssh_hosts: &[BoundSshHost],
    cancellation: &Arc<AtomicBool>,
    allow_external_paths: bool,
    output_sink: &Option<CommandOutputSink>,
    android_app: Option<&AppHandle>,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<Vec<(String, String, ToolRegistry)>, String> {
    let mut registries = Vec::new();
    for host in bound_ssh_hosts.iter().filter(|host| host.is_ready()) {
        let Some(context) = host.context.as_ref() else {
            continue;
        };
        let registry = local_tool_registry(
            std::slice::from_ref(&host.root),
            &[],
            Vec::new(),
            cancellation.clone(),
            allow_external_paths,
            output_sink.clone(),
            Some(context),
            None,
            android_app,
            session_id,
            turn_id,
        )?;
        registries.push((host.name.clone(), host.connection_id.clone(), registry));
    }
    Ok(registries)
}

/// 注册 `ssh__*`：有多台已绑定主机时改用可路由别名（带可选 `target`），
/// 否则保持原来的单环境别名，旧会话行为不变。
fn register_ssh_aliases(
    registry: &mut ToolRegistry,
    ssh: &ToolRegistry,
    bound: &[(String, String, ToolRegistry)],
    has_default_host: bool,
) -> Result<(), String> {
    let names = ssh
        .list_public()
        .into_iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let mut targets: Vec<(String, &ToolRegistry)> = Vec::new();
    for (name, connection_id, host_registry) in bound {
        targets.push((name.clone(), host_registry));
        if connection_id.trim() != name.trim() {
            targets.push((connection_id.clone(), host_registry));
        }
    }
    // 不填 target 时落到 primary：有默认 SSH 目标时是它，否则是第一台已绑定主机。
    let hint = if has_default_host {
        "可选参数 target 指定该对话绑定的远程主机（填连接名或连接 ID，不填则使用默认主机）。"
    } else {
        "可选参数 target 指定该对话绑定的远程主机（填连接名或连接 ID，不填则使用第一台已绑定的主机）。"
    };
    for name in names {
        if targets.is_empty() {
            registry
                .register_alias_from(
                    format!("{}__{name}", AgentToolEnvironment::Ssh.prefix()),
                    ssh,
                    &name,
                    "[SSH 远程]",
                )
                .map_err(|error| error.to_string())?;
        } else {
            registry
                .register_routed_alias(
                    format!("{}__{name}", AgentToolEnvironment::Ssh.prefix()),
                    (ssh, &name),
                    &targets,
                    "[SSH 远程]",
                    hint,
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn environment_tool_registry(
    default_environment: AgentToolEnvironment,
    windows_paths: &[String],
    windows_excluded_paths: &[String],
    ssh_paths: &[String],
    wsl_paths: &[String],
    mcp_tools: Vec<ConnectedMcpTool>,
    cancellation: Arc<AtomicBool>,
    allow_external_paths: bool,
    output_sink: Option<CommandOutputSink>,
    ssh_context: Option<&agent_core::SshToolContext>,
    wsl_context: Option<&agent_core::LocalToolContext>,
    bound_ssh_hosts: &[BoundSshHost],
    android_app: Option<&AppHandle>,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> Result<ToolRegistry, String> {
    let windows_skill_cancellation = cancellation.clone();
    let windows = local_tool_registry(
        windows_paths,
        windows_excluded_paths,
        mcp_tools,
        cancellation.clone(),
        allow_external_paths,
        output_sink.clone(),
        None,
        None,
        android_app,
        session_id,
        turn_id,
    )?;
    let bound = bound_ssh_registries(
        bound_ssh_hosts,
        &cancellation,
        allow_external_paths,
        &output_sink,
        android_app,
        session_id,
        turn_id,
    )?;
    let mut ssh = ssh_context
        .map(|context| {
            local_tool_registry(
                ssh_paths,
                &[],
                Vec::new(),
                cancellation.clone(),
                allow_external_paths,
                output_sink.clone(),
                Some(context),
                None,
                android_app,
                session_id,
                turn_id,
            )
        })
        .transpose()?;
    // 默认目标不是 SSH 时也要让绑定主机可用：借第一台已连通主机的注册表作为
    // `ssh__*` 的基础定义，其余主机通过 `target` 路由。
    if ssh.is_none() {
        if let Some((_, _, first)) = bound.first() {
            ssh = Some(first.clone());
        }
    }
    let wsl = wsl_context
        .map(|context| {
            local_tool_registry(
                wsl_paths,
                &[],
                Vec::new(),
                cancellation,
                allow_external_paths,
                output_sink,
                None,
                Some(context),
                android_app,
                session_id,
                turn_id,
            )
        })
        .transpose()?;

    let mut registry = match default_environment {
        AgentToolEnvironment::Windows => windows.clone(),
        AgentToolEnvironment::Ssh => ssh
            .as_ref()
            .cloned()
            .ok_or_else(|| "SSH 默认执行环境尚未准备完成".to_string())?,
        AgentToolEnvironment::Wsl => wsl
            .as_ref()
            .cloned()
            .ok_or_else(|| "WSL 默认执行环境尚未准备完成".to_string())?,
    };
    register_environment_aliases(
        &mut registry,
        &windows,
        AgentToolEnvironment::Windows,
        "[Windows 本机]",
    )?;
    if let Some(ssh) = ssh.as_ref() {
        register_ssh_aliases(&mut registry, ssh, &bound, ssh_context.is_some())?;
    }
    if let Some(wsl) = wsl.as_ref() {
        register_environment_aliases(
            &mut registry,
            wsl,
            AgentToolEnvironment::Wsl,
            "[Windows WSL]",
        )?;
    }
    if let Some(app) = android_app {
        if let Some(session) = session_id {
            if let Some(turn) = turn_id {
                // 询问工具自己解析当前请求的取消令牌，不能接收工具级令牌。
                registry
                    .register_handler(user_interaction::AskUserTool {
                        app: app.clone(),
                        session_id: session.into(),
                        turn_id: turn.into(),
                    })
                    .map_err(|error| error.to_string())?;
                // 只有完全没有任何远程主机（默认或绑定）时才需要先连一台。
                if ssh_context.is_none() && !bound_ssh_hosts.iter().any(|host| host.is_ready()) {
                    registry
                        .register_handler(ssh_connection::ConnectSshTool {
                            app: app.clone(),
                            session_id: session.into(),
                            turn_id: turn.into(),
                            cancellation: windows_skill_cancellation.clone(),
                        })
                        .map_err(|error| error.to_string())?;
                }
            }
            for cancel in [false, true] {
                registry
                    .register_handler(background_shell::BackgroundTaskTool {
                        app: app.clone(),
                        session_id: session.into(),
                        cancel,
                    })
                    .map_err(|e| e.to_string())?;
            }
        }
        registry
            .register_handler(LoadSkillTool {
                app: app.clone(),
                cancellation: windows_skill_cancellation,
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(registry)
}

pub(super) fn register_collaboration_tool(
    registry: &mut ToolRegistry,
    approved_registry: &mut Option<ToolRegistry>,
    tool: Option<&CreateCollaborationJobTool>,
    cancellation: Arc<AtomicBool>,
) -> Result<(), String> {
    let Some(tool) = tool else {
        return Ok(());
    };
    registry
        .register_handler(tool.with_cancellation(cancellation.clone()))
        .map_err(|error| error.to_string())?;
    // G6：顺序 workflow 编排器与协作工具同链注册（同构派生），证明编排
    // 层复用既有内核而不改动它。
    registry
        .register_handler(WorkflowTool::from(tool).with_cancellation(cancellation.clone()))
        .map_err(|error| error.to_string())?;
    if let Some(approved_registry) = approved_registry.as_mut() {
        approved_registry
            .register_handler(tool.with_cancellation(cancellation.clone()))
            .map_err(|error| error.to_string())?;
        approved_registry
            .register_handler(WorkflowTool::from(tool).with_cancellation(cancellation))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_environment_tool_registries(
    default_environment: AgentToolEnvironment,
    windows_paths: &[String],
    windows_excluded_paths: &[String],
    ssh_paths: &[String],
    wsl_paths: &[String],
    mcp_tools: &[ConnectedMcpTool],
    cancellation: Arc<AtomicBool>,
    allow_external_paths: bool,
    output_sink: &Option<CommandOutputSink>,
    ssh_context: Option<&agent_core::SshToolContext>,
    wsl_context: Option<&agent_core::LocalToolContext>,
    bound_ssh_hosts: &[BoundSshHost],
    android_app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    collaboration_tool: Option<&CreateCollaborationJobTool>,
) -> Result<(ToolRegistry, Option<ToolRegistry>), String> {
    let collaboration_cancellation = cancellation.clone();
    let mut registry = environment_tool_registry(
        default_environment,
        windows_paths,
        windows_excluded_paths,
        ssh_paths,
        wsl_paths,
        mcp_tools.to_vec(),
        cancellation.clone(),
        allow_external_paths,
        output_sink.clone(),
        ssh_context,
        wsl_context,
        bound_ssh_hosts,
        Some(android_app),
        Some(session_id),
        Some(turn_id),
    )?;
    let mut approved_registry = if allow_external_paths {
        None
    } else {
        Some(environment_tool_registry(
            default_environment,
            windows_paths,
            windows_excluded_paths,
            ssh_paths,
            wsl_paths,
            mcp_tools.to_vec(),
            cancellation,
            true,
            output_sink.clone(),
            ssh_context,
            wsl_context,
            bound_ssh_hosts,
            Some(android_app),
            Some(session_id),
            Some(turn_id),
        )?)
    };
    register_collaboration_tool(
        &mut registry,
        &mut approved_registry,
        collaboration_tool,
        collaboration_cancellation,
    )?;
    Ok((registry, approved_registry))
}

#[tauri::command]
pub async fn cancel_agent_request(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    session_id: String,
    turn_id: Option<String>,
    source: Option<String>,
) -> Result<AgentCancellationStatus, String> {
    // 先把本轮自己的取消标志置上，再去做协作级联。级联要抢进程级 history 写锁
    // 并开一次 SQLite；放在前面会让「点停止」排在工具事件持久化之后，用户看到
    // 的是按钮点了没反应。停止只需要一个 AtomicBool，必须是最快的路径。
    let mut cancellations = Vec::<(String, String, Arc<AtomicBool>)>::new();
    let mut stale_active_turn = false;
    let mut parent_requested = false;
    {
        let requests = state
            .requests
            .lock()
            .map_err(|_| "请求状态不可用".to_string())?;
        if let Some(control) = requests.get(&session_id) {
            if turn_id
                .as_deref()
                .is_some_and(|requested| requested != control.turn_id)
            {
                stale_active_turn = true;
            } else {
                // P1-B：标志与令牌必须一起置位（见 AgentRequestControl::request_cancellation），
                // 准备阶段等待在 await 上的工作才能被立刻打断。
                control.request_cancellation("请求已取消");
                cancellations.push((
                    session_id.clone(),
                    control.turn_id.clone(),
                    control.cancellation.clone(),
                ));
                parent_requested = true;
            }
        }
    }
    // 级联取消失败不能让本轮的停止失效：上面的取消标志已经生效，这里只记录。
    let child_targets = match crate::history::request_collaboration_cancellation(
        &app,
        &session_id,
        turn_id.as_deref(),
    ) {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!("无法级联取消协作任务: {error}");
            Vec::new()
        }
    };
    let cascade_requested = !child_targets.is_empty();
    if !child_targets.is_empty() {
        let requests = state
            .requests
            .lock()
            .map_err(|_| "请求状态不可用".to_string())?;
        for target in &child_targets {
            if cancellations
                .iter()
                .any(|(target_session_id, _, _)| target_session_id == &target.session_id)
            {
                continue;
            }
            if let Some(control) = requests.get(&target.session_id) {
                if control.turn_id == target.turn_id {
                    // 与主目标一致：标志 + 令牌同时置位（P1-B）。
                    control.request_cancellation("请求已取消");
                    cancellations.push((
                        target.session_id.clone(),
                        control.turn_id.clone(),
                        control.cancellation.clone(),
                    ));
                }
            }
        }
    }
    let source = match source.as_deref() {
        Some("stop_button") => "stop_button",
        Some("delete_session") => "delete_session",
        Some("cancel_submission") => "cancel_submission",
        Some("collaboration_stop") => "collaboration_stop",
        _ => "unspecified",
    };
    crate::diagnostics::record(
        "info",
        "agent.cancellation_requested",
        "native.agent",
        json!({
            "session_id": session_id, "turn_id": turn_id, "source": source,
            "parent_requested": parent_requested, "cascade_requested": cascade_requested,
            "stale_active_turn": stale_active_turn, "active_targets": cancellations.len(),
        }),
    );
    for (target_session_id, target_turn_id, _) in &cancellations {
        crate::diagnostics::record(
            "info",
            "agent.cancellation_applied",
            "native.agent",
            json!({
                "session_id": target_session_id, "turn_id": target_turn_id,
                "source": source, "request_session_id": session_id,
                "cascade": target_session_id != &session_id,
            }),
        );
        // G4-A：Interrupt hook——每个被取消的目标通知一次（无决策语义，
        // 失败只记日志；codex 同位置：interrupted turn aborted 之前）。
        super::hooks::run_interrupt_hooks(target_session_id, target_turn_id).await;
    }
    if stale_active_turn && cancellations.is_empty() {
        // A stale UI callback may cancel children belonging to its old turn,
        // but it must never cancel or report completion for the new live turn.
        return Ok(AgentCancellationStatus {
            requested: cascade_requested,
            completed: false,
            aborted: false,
        });
    }
    if cancellations.is_empty() {
        return Ok(AgentCancellationStatus {
            requested: cascade_requested,
            completed: true,
            aborted: false,
        });
    }

    // The Rust turn owns the provider stream and native tools directly. The
    // request cancellation flag is therefore the single cancellation source;
    // there is no external child process to signal here.

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1_500);
    loop {
        let still_active = {
            let requests = state
                .requests
                .lock()
                .map_err(|_| "请求状态不可用".to_string())?;
            cancellations
                .iter()
                .any(|(target_session_id, _, cancellation)| {
                    requests
                        .get(target_session_id)
                        .is_some_and(|control| Arc::ptr_eq(&control.cancellation, cancellation))
                })
        };
        if !still_active {
            return Ok(AgentCancellationStatus {
                requested: parent_requested || cascade_requested,
                completed: true,
                aborted: false,
            });
        }
        if tokio::time::Instant::now() >= deadline {
            // P1-C 兜底：软取消（标志 + 令牌）在宽限期内没有让回合退出，
            // 说明执行流卡在无法打断的等待里。硬中止回合任务，收尾由
            // queue.rs 的外层任务完成——abort 表现为 JoinError，映射成
            // 「请求已取消」后走既有的 cancelled 终态落库与事件路径。
            // 正常路径永远不会走到这里。请求表项仍然保留到任务真正退出：
            // Provider 或子进程可能短暂忽略取消，提前移除会让同一会话在
            // 第一个 Agent 仍可能改文件时启动第二个 Agent。
            let mut aborted = false;
            for (target_session_id, target_turn_id, target_cancellation) in &cancellations {
                let target_aborted = state
                    .requests
                    .lock()
                    .map(|requests| {
                        requests.get(target_session_id).is_some_and(|control| {
                            control.turn_id == *target_turn_id
                                && Arc::ptr_eq(&control.cancellation, target_cancellation)
                                && control.abort_task()
                        })
                    })
                    .unwrap_or(false);
                if target_aborted {
                    aborted = true;
                    crate::diagnostics::record(
                        "warn",
                        "agent.cancellation_aborted",
                        "native.agent",
                        json!({
                            "session_id": target_session_id, "turn_id": target_turn_id,
                            "source": source, "grace_ms": 1_500,
                        }),
                    );
                }
            }
            return Ok(AgentCancellationStatus {
                requested: parent_requested || cascade_requested,
                completed: false,
                aborted,
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
