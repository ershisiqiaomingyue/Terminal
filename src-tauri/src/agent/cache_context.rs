use super::*;
use agent_core::{ModelTranscript, PromptCacheFingerprint};

pub(super) fn scope(
    provider: &AgentProviderSettings,
    model: &str,
    protocol: ProviderKind,
    request: &AgentChatRequest,
    root: &str,
    ssh_connection_id: Option<&str>,
) -> String {
    agent_core::prompt_cache_hash(&json!({
        "version":2, "provider":provider.id, "endpoint":agent_core::transcript_endpoint_identity(&provider.id, &provider.base_url),
        "model":model, "protocol":protocol, "target":request.execution_target,
        "root":root, "analysis":request.analysis_mode,
        "local_workspace":agent_core::local_workspace_path(request), "ssh_connection_id":ssh_connection_id,
    }))
}

pub(super) async fn save(
    app: &AppHandle,
    session: &str,
    turn: &str,
    scope: String,
    runtime: String,
    messages: Vec<AgentMessage>,
    images: &[ProviderImageAttachment],
    fingerprint: Option<PromptCacheFingerprint>,
) {
    let images = image_attachments_for_messages(images, &messages);
    let transcript = ModelTranscript {
        version: 1,
        scope,
        turn_id: turn.into(),
        runtime_context: runtime,
        messages,
        images,
        last_request: fingerprint,
    };
    let save_app = app.clone();
    let save_session = session.to_string();
    let result = tokio::task::spawn_blocking(move || {
        crate::history::save_model_transcript(&save_app, &save_session, &transcript)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    if let Err(error) = result {
        eprintln!("无法保存模型上下文: {error}");
        crate::diagnostics::record(
            "error",
            "agent.model_context_save_failed",
            "native.agent",
            json!({"session_id":session, "turn_id":turn}),
        );
        emit_agent_status_for_turn(
            app,
            session,
            "model_context_save_failed",
            "原始模型上下文未能保存，下轮将使用对话历史恢复",
            Some(turn),
        );
    }
}

pub(super) fn diagnostic(
    app: &AppHandle,
    session: &str,
    turn: &str,
    config: &ProviderConfig,
    request: &ProviderRequest,
    previous: &mut Option<PromptCacheFingerprint>,
) {
    if let Ok(next) = PromptCacheFingerprint::capture(config, request) {
        let diagnostic = next.compare(previous.as_ref());
        emit_agent_payload_for_turn(
            app,
            json!({"type":"cache_diagnostic", "session_id":session,
            "diagnostic":diagnostic}),
            turn,
        );
        *previous = Some(next);
    }
}

pub(super) fn initial_messages(
    restored_checkpoint_messages: Option<Vec<AgentMessage>>,
    request: &AgentChatRequest,
    prompt: &str,
    turn_id: &str,
    current_user_message: AgentMessage,
    context_budget: usize,
    request_overhead: usize,
) -> (Vec<AgentMessage>, agent_core::CompactionResult) {
    if let Some(mut checkpoint_messages) = restored_checkpoint_messages {
        // The durable checkpoint replaces only transcript-derived context.
        // Recovery notices and other explicit runtime context still apply.
        checkpoint_messages.extend(
            request
                .context
                .iter()
                .filter(|turn| {
                    !matches!(
                        turn.source.as_deref(),
                        Some("history" | "project_configuration")
                    )
                })
                .filter(|turn| {
                    turn.turn_id.as_deref() != Some(turn_id)
                        || !turn.role.eq_ignore_ascii_case("user")
                        || turn.content.trim() != prompt.trim()
                })
                .filter_map(context_turn_message),
        );
        let mut full = checkpoint_messages.clone();
        full.push(current_user_message.clone());
        let compacted = agent_core::compact_messages_with_current_with_overhead(
            &checkpoint_messages,
            current_user_message.clone(),
            context_budget,
            request_overhead,
        );
        (full, compacted)
    } else {
        let mut history = request
            .context
            .iter()
            .filter(|turn| turn.source.as_deref() != Some("project_configuration"))
            .filter_map(context_turn_message)
            .collect::<Vec<_>>();
        if history.last().is_some_and(|message| {
            message.role == MessageRole::User && message.content.trim() == prompt.trim()
        }) {
            history.pop();
        }
        let mut full = history.clone();
        full.push(current_user_message.clone());
        let compacted = agent_core::compact_messages_with_current_with_overhead(
            &history,
            current_user_message.clone(),
            context_budget,
            request_overhead,
        );
        (full, compacted)
    }
}

pub(super) fn restore_images(
    images: &mut Vec<ProviderImageAttachment>,
    transcript: Option<&ModelTranscript>,
) {
    if let Some(transcript) = transcript {
        for image in &transcript.images {
            if !images
                .iter()
                .any(|i| i.source_message_id == image.source_message_id && i.name == image.name)
            {
                images.push(image.clone());
            }
        }
    }
}

pub(super) fn final_assistant(
    text: &str,
    reasoning: &str,
    responses: Vec<Value>,
    thinking: Vec<Value>,
    content: Vec<Value>,
) -> AgentMessage {
    AgentMessage {
        role: MessageRole::Assistant,
        content: text.into(),
        reasoning_content: (!reasoning.is_empty()).then(|| reasoning.into()),
        responses_reasoning_items: responses,
        anthropic_thinking_blocks: thinking,
        anthropic_content_blocks: content,
        tool_call_id: None,
        tool_calls: vec![],
        source_message_id: None,
    }
}

/// P9-F：本轮是否值得先发一次「预热」请求。
///
/// 供应商的提示缓存通常要**被读取一次**才落盘（Anthropic 的 5 分钟 TTL、
/// OpenAI 的 5–10 分钟路由缓存同理）。压缩会改写前缀，压缩前那一轮的完整
/// 前缀如果没人读过，就是白付一次全价输入；而下一轮若立刻触发压缩，那次
/// 全价输入永远不会被复用。所以只在「下一轮就会压缩」且本轮前缀**在当前
/// 上下文窗口里仍占大头**时才预热：先发一个极小请求把前缀写成热缓存，
/// 紧跟的压缩后请求仍能命中它。
pub(super) fn should_prewarm_cache(
    estimated_tokens: usize,
    trigger_tokens: usize,
    messages_len: usize,
) -> bool {
    if messages_len < 2 || trigger_tokens == 0 {
        return false;
    }
    estimated_tokens >= trigger_tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_prewarm_only_fires_when_the_next_round_will_compact() {
        // P9-F：只在本轮前缀即将被压缩改写、且仍有复用之利时才预热。
        assert!(should_prewarm_cache(80_000, 80_000, 12));
        assert!(should_prewarm_cache(95_000, 80_000, 12));
        assert!(!should_prewarm_cache(79_999, 80_000, 12));
        assert!(!should_prewarm_cache(95_000, 0, 12));
        assert!(!should_prewarm_cache(95_000, 80_000, 1));
    }

    #[test]
    fn plugin_runtime_updates_survive_checkpoint_restore_without_rewriting_history() {
        let original = vec![
            AgentMessage::user("检查项目"),
            final_assistant("此前已读取目录。", "", vec![], vec![], vec![]),
        ];
        let mut messages = original.clone();
        let mut runtime = "当前配置：旧插件".to_string();
        let next = "当前配置：允许工具\n插件使用新目录 D:/new".to_string();
        let tokens = update_runtime(&mut messages, &mut runtime, next.clone());
        assert_eq!(&messages[..original.len()], original.as_slice());
        // P4-C：新消息带 replace 声明前缀（codex 同款语义——明示取代旧指引）。
        assert_eq!(
            messages.last().unwrap().content,
            format!("[runtime 指引已更新——以下内容取代此前提供的全部 runtime 指引]\n{next}")
        );
        assert_eq!(runtime, next);
        assert!(tokens > 0);
        update_runtime(&mut messages, &mut runtime, next.clone());
        assert_eq!(messages.len(), original.len() + 1);
        let mut checkpoint = vec![AgentMessage::system("较早对话摘要")];
        retain_runtime(&mut checkpoint, &runtime);
        // retain_runtime 幂等：checkpoint 补的是裸 runtime（无 replace 前缀），
        // 与 P4-C 的声明前缀识别共存（见 retain_runtime 文档）。
        assert_eq!(checkpoint.last().unwrap().content, next);
        update_runtime(&mut messages, &mut runtime, "当前配置：插件已停用".into());
        assert_eq!(
            messages.last().unwrap().content,
            format!("[runtime 指引已更新——以下内容取代此前提供的全部 runtime 指引]\n当前配置：插件已停用")
        );
    }

    #[test]
    fn prompt_reuses_evidence_and_limits_refreshes_to_concrete_gaps() {
        let prompt = compose(&request(), &[]).system;
        assert!(prompt.contains("新消息不意味着所有文件都发生了变化"));
        assert!(prompt.contains("写入前必须核验"));
        assert!(!prompt.contains("判断当前状态时，必须针对具体缺口复核"));
    }

    #[test]
    fn domain_workflows_and_language_preferences_only_come_from_instruction_files() {
        let empty = compose(&request(), &[]);
        for text in [
            "Android",
            "Yocto",
            "AOSP",
            "CarAdapter",
            "print_lunch_menu",
            "默认使用简体中文",
        ] {
            assert!(!empty.system.contains(text), "{text}");
            assert!(!empty.runtime.contains(text), "{text}");
        }
        let instructions = [(
            "~/.android-tool/AGENTS.md".into(),
            "默认使用简体中文；Yocto 构建按项目配置。".into(),
        )];
        let loaded = compose_with_globals(&request(), &[], &instructions, ProviderKind::DeepSeek);
        assert!(loaded.runtime.contains(&instructions[0].1));
        assert_eq!(empty.system, loaded.system);
        assert!(!compose(&request(), &[])
            .runtime
            .contains(&instructions[0].1));
    }

    #[test]
    fn default_guidance_catalog_never_injects_domain_bodies_without_selection() {
        let resources =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/agent-instructions");
        let globals = vec![(
            "~/.android-tool/AGENTS.md".into(),
            std::fs::read_to_string(resources.join("AGENTS.md")).unwrap(),
        )];
        let catalog = agent_core::scan_skill_repository(&resources.join("skills"))
            .unwrap()
            .into_iter()
            .map(|p| crate::skill_runtime::SkillView {
                id: p.id,
                name: p.name,
                description: p.description,
                repository: "guidance".into(),
                path: p.path,
                runtime: p.runtime,
                entrypoint: p.entrypoint,
                dependencies: p.dependencies,
                status: "ready".into(),
                status_message: "可加载".into(),
                builtin: false,
                enabled: true,
            })
            .collect::<Vec<_>>();
        let mut request = request();
        request.message = "修改按钮的圆角".into();
        request.workspace_path = Some("D:/workspace/Android-Tool".into());
        request.context.push(
            serde_json::from_value(
                json!({"role":"user", "source":"history", "content":"此前在排查 Yocto 编译"}),
            )
            .unwrap(),
        );
        let selected = vec![(
            "android-task-guidance".into(),
            std::fs::read_to_string(resources.join("skills/android-task-guidance/SKILL.md"))
                .unwrap(),
        )];
        for mode in ["ask", "risk", "full"] {
            request.permission_mode = Some(mode.into());
            let discovery = compose_with_catalog(
                &request,
                &[],
                &globals,
                ProviderKind::DeepSeek,
                Some(catalog.clone()),
            );
            assert!(discovery.runtime.contains("/android-task-guidance"));
            assert!(discovery.runtime.contains("/yocto-task-guidance"));
            assert!(discovery.runtime.contains("默认使用简体中文"));
            for body in [
                "capture_android_logcat",
                "android_archive_edit",
                "从用户选定的工程及项目指导中确认源码根",
            ] {
                assert!(!discovery.system.contains(body));
                assert!(!discovery.runtime.contains(body));
            }
            let loaded = compose_with_catalog(
                &request,
                &selected,
                &globals,
                ProviderKind::DeepSeek,
                Some(catalog.clone()),
            );
            assert!(loaded.runtime.contains("capture_android_logcat"));
            assert!(!loaded
                .runtime
                .contains("从用户选定的工程及项目指导中确认源码根"));
            // Selecting a Skill in a preceding turn doesn't alter future composition.
            let next = compose_with_catalog(
                &request,
                &[],
                &globals,
                ProviderKind::DeepSeek,
                Some(catalog.clone()),
            );
            assert_eq!(discovery.runtime, next.runtime);
            assert_eq!(discovery.system, loaded.system);
        }
    }

    #[test]
    fn local_source_memory_survives_native_snapshot_and_checkpoint_replacement() {
        let mut request = request();
        request.context.push(serde_json::from_value(json!({"role":"user", "content":"用这个目录", "source":"history",
            "artifacts":[{"kind":"directory","path":"D:/CarAdapter/out","name":"out","status":"user_attachment","content":"not replayed"}]})).unwrap());
        request.artifacts.push(serde_json::from_value(json!({"kind":"directory", "path":"D:/new/out", "name":"out", "status":"user_attachment"})).unwrap());
        let (full, _) = initial_messages(
            Some(vec![AgentMessage::system("compacted older transcript")]),
            &request,
            "继续推送",
            "new-turn",
            AgentMessage::user("继续推送"),
            8192,
            0,
        );
        // The raw history is replaced, but source identity is independently in
        // the runtime suffix which is appended after compaction on every turn.
        assert!(!full
            .iter()
            .any(|message| message.content.contains("D:/CarAdapter/out")));
        let prompt = compose(&request, &[]);
        assert!(prompt.runtime.contains("D:/CarAdapter/out"));
        assert!(prompt.runtime.contains("本机来源使用 windows__*"));
        assert!(prompt.runtime.contains("当前消息中的已选择来源优先"));
        assert!(!prompt.runtime.contains("not replayed"));
        let user_prompt = compose_agent_prompt(&request, &AgentLogEvidence::default());
        assert!(user_prompt.contains("D:/new/out"));
        assert!(user_prompt.contains("文件或目录优先"));
    }

    fn request() -> AgentChatRequest {
        serde_json::from_value(
            json!({"modelId":"p:claude-sonnet-4-5", "message":"check code", "artifacts":[]}),
        )
        .unwrap()
    }

    fn compose(
        request: &AgentChatRequest,
        skills: &[(String, String)],
    ) -> prompt_composer::ComposedAgentPrompt {
        compose_with_globals(request, skills, &[], ProviderKind::Anthropic)
    }

    fn compose_with_globals(
        request: &AgentChatRequest,
        skills: &[(String, String)],
        globals: &[(String, String)],
        protocol: ProviderKind,
    ) -> prompt_composer::ComposedAgentPrompt {
        compose_with_catalog(request, skills, globals, protocol, None)
    }

    fn compose_with_catalog(
        request: &AgentChatRequest,
        skills: &[(String, String)],
        globals: &[(String, String)],
        protocol: ProviderKind,
        catalog: Option<Vec<crate::skill_runtime::SkillView>>,
    ) -> prompt_composer::ComposedAgentPrompt {
        prompt_composer::compose(prompt_composer::AgentPromptContext {
            request,
            provider_name: "Provider",
            model_id: "claude-sonnet-4-5",
            protocol,
            tool_policy: AgentToolPolicy::Auto,
            model_supports_tools: true,
            request_tools: &[],
            selinux_enabled: false,
            selinux_has_roots: false,
            skill_instructions: skills,
            skill_catalog: catalog.as_deref(),
            is_ssh_target: false,
            is_wsl_target: false,
            default_environment: AgentToolEnvironment::Windows,
            ssh_root: None,
            wsl_root: None,
            bound_ssh_hosts: &[],
            default_ssh_host: None,
            workspace_context: None,
            global_instructions: globals,
        })
    }

    #[test]
    fn global_instructions_reach_standalone_and_remote_turns_in_all_modes_and_protocols() {
        let globals = vec![(
            "~/.android-tool/agent.md".into(),
            "优先使用中文，模型提供的可见推理摘要也尽量使用中文。".into(),
        )];
        for protocol in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
            ProviderKind::OpenAiCompatible,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
            ProviderKind::Glm,
            ProviderKind::MiMo,
        ] {
            for mode in ["ask", "risk", "full"] {
                for remote in [false, true] {
                    let mut request = request();
                    request.permission_mode = Some(mode.into());
                    request.tool_policy = AgentToolPolicy::Disabled;
                    if remote {
                        request.workspace_path = Some("/remote-project".into());
                        request.execution_target = Some(agent_core::AgentExecutionTarget {
                            kind: "ssh".into(),
                            root: Some("/remote-project".into()),
                            connection_id: Some("host".into()),
                            profile_id: None,
                        });
                    }
                    let empty = compose_with_globals(&request, &[], &[], protocol);
                    let prompt = compose_with_globals(&request, &[], &globals, protocol);
                    assert!(prompt.runtime.contains("[用户全局指令]"));
                    assert!(prompt.runtime.contains(&globals[0].1));
                    assert!(
                        prompt.runtime.find("[用户全局指令]").unwrap()
                            < prompt.runtime.find("[当前项目配置]").unwrap()
                    );
                    assert_eq!(empty.system, prompt.system);
                    let changed = compose_with_globals(
                        &request,
                        &[],
                        &[(globals[0].0.clone(), "已更新的全局偏好".into())],
                        protocol,
                    );
                    assert_ne!(changed.runtime, prompt.runtime);
                    assert_eq!(changed.system, prompt.system);
                }
            }
        }
    }

    #[test]
    fn global_instructions_outrank_project_configuration() {
        let globals = vec![(
            "~/.android-tool/AGENTS.md".into(),
            "思考摘要用中文；全局优先。".into(),
        )];
        let mut request = request();
        request.context.push(AgentContextTurn {
            role: "system".into(),
            content: "project-specific build instructions".into(),
            source: Some("project_configuration".into()),
            message_id: None,
            turn_id: None,
            model_id: None,
            attachments: vec![],
            artifacts: vec![],
        });
        let prompt = compose_with_globals(&request, &[], &globals, ProviderKind::DeepSeek);
        assert!(
            prompt.runtime.find("[用户全局指令]").unwrap()
                < prompt.runtime.find("[当前项目配置]").unwrap()
        );
        assert!(
            prompt
                .runtime
                .contains("这些偏好的优先级高于后面的[当前项目配置]"),
            "{}",
            prompt.runtime
        );
        assert!(!prompt
            .runtime
            .contains("具体项目约定和用户当前的明确要求优先"));
        let workspace = agent_core::WorkspaceAgentContext {
            root: "/workspace/demo".into(),
            instructions: vec![("AGENTS.md".into(), "项目内规则".into())],
            permissions: Default::default(),
        };
        let prompt = prompt_composer::compose(prompt_composer::AgentPromptContext {
            request: &request,
            provider_name: "Provider",
            model_id: "claude-sonnet-4-5",
            protocol: ProviderKind::DeepSeek,
            tool_policy: AgentToolPolicy::Auto,
            model_supports_tools: true,
            request_tools: &[],
            selinux_enabled: false,
            selinux_has_roots: false,
            skill_instructions: &[],
            skill_catalog: None,
            is_ssh_target: false,
            is_wsl_target: false,
            default_environment: AgentToolEnvironment::Windows,
            ssh_root: None,
            wsl_root: None,
            bound_ssh_hosts: &[],
            default_ssh_host: None,
            workspace_context: Some(&workspace),
            global_instructions: &globals,
        });
        assert!(
            prompt
                .runtime
                .contains("以下项目目录指令的优先级低于[用户全局指令]"),
            "{}",
            prompt.runtime
        );
    }

    #[test]
    fn project_and_skill_changes_update_runtime_without_rewriting_system() {
        let mut request = request();
        let first = compose(&request, &[]);
        request.context.push(AgentContextTurn {
            role: "system".into(),
            content: "project-specific build instructions".into(),
            source: Some("project_configuration".into()),
            message_id: None,
            turn_id: None,
            model_id: None,
            attachments: vec![],
            artifacts: vec![],
        });
        let next = compose(
            &request,
            &[("android-crash".into(), "inspect crashes".into())],
        );
        assert_eq!(first.system, next.system);
        assert_ne!(first.runtime, next.runtime);
        assert!(next.runtime.contains("project-specific build instructions"));
        assert!(next.runtime.contains("inspect crashes"));
        let first = compose(
            &request,
            &[("b".into(), "B".into()), ("a".into(), "A".into())],
        );
        let next = compose(
            &request,
            &[("a".into(), "A".into()), ("b".into(), "B".into())],
        );
        assert_eq!(first.runtime, next.runtime);
    }

    #[test]
    fn ssh_connection_guidance_tracks_available_tools_without_rewriting_system() {
        let request = request();
        let compose = |tools: &[ToolDefinition], ssh_root: Option<&str>| {
            prompt_composer::compose(prompt_composer::AgentPromptContext {
                request: &request,
                provider_name: "Provider",
                model_id: "model",
                protocol: ProviderKind::DeepSeek,
                tool_policy: AgentToolPolicy::Auto,
                model_supports_tools: true,
                request_tools: tools,
                selinux_enabled: false,
                selinux_has_roots: false,
                skill_instructions: &[],
                skill_catalog: None,
                is_ssh_target: false,
                is_wsl_target: false,
                default_environment: AgentToolEnvironment::Windows,
                ssh_root,
                wsl_root: None,
                bound_ssh_hosts: &[],
                default_ssh_host: None,
                workspace_context: None,
                global_instructions: &[],
            })
        };
        let unavailable = compose(&[], None);
        let chooser = compose(
            &[ToolDefinition::new(
                ssh_connection::CONNECT_SSH_TOOL,
                "Connect SSH",
                json!({"type":"object"}),
            )],
            None,
        );
        let connected = compose(&[], Some("/remote-project"));
        assert!(!unavailable.runtime.contains("request_ssh_connection"));
        assert!(chooser
            .runtime
            .contains("先单独调用 request_ssh_connection"));
        assert!(chooser
            .runtime
            .contains("仅讨论 SSH 概念或配置文本时无需连接"));
        assert!(!connected.runtime.contains("request_ssh_connection"));
        assert!(connected.runtime.contains("/remote-project"));
        assert!(connected
            .runtime
            .contains("本轮无前缀工具默认指向 Windows 本机"));
        assert_eq!(unavailable.system, chooser.system);
        assert_eq!(chooser.system, connected.system);
    }

    #[test]
    fn bound_ssh_hosts_are_described_with_target_usage() {
        let request = request();
        let bound = vec![
            super::super::BoundSshHost {
                connection_id: "conn-a".into(),
                name: "构建机".into(),
                root: "/srv/build".into(),
                context: Some(agent_core::SshToolContext::new(
                    agent_core::SshConnectionConfig {
                        id: "conn-a".into(),
                        name: "构建机".into(),
                        host: "127.0.0.1".into(),
                        port: 22,
                        username: "tester".into(),
                        private_key_path: None,
                        known_hosts_path: None,
                        strict_host_key: false,
                        default_root: Some("/srv/build".into()),
                        reuse_connection: false,
                        connect_timeout_secs: 1,
                        operation_timeout_secs: 1,
                        keepalive_interval_secs: 0,
                    },
                    "/srv/build",
                    None,
                    Arc::new(AtomicBool::new(false)),
                    None,
                )),
                note: None,
            },
            super::super::BoundSshHost {
                connection_id: "conn-b".into(),
                name: "日志机".into(),
                root: String::new(),
                context: None,
                note: Some("连接超时".into()),
            },
        ];
        let prompt = prompt_composer::compose(prompt_composer::AgentPromptContext {
            request: &request,
            provider_name: "Provider",
            model_id: "model",
            protocol: ProviderKind::DeepSeek,
            tool_policy: AgentToolPolicy::Auto,
            model_supports_tools: true,
            request_tools: &[],
            selinux_enabled: false,
            selinux_has_roots: false,
            skill_instructions: &[],
            skill_catalog: None,
            is_ssh_target: false,
            is_wsl_target: false,
            default_environment: AgentToolEnvironment::Windows,
            ssh_root: None,
            wsl_root: None,
            bound_ssh_hosts: &bound,
            default_ssh_host: Some(&("默认机".to_string(), "conn-default".to_string())),
            workspace_context: None,
            global_instructions: &[],
        });
        // 已连通主机：明确根目录与 target 值；未连通主机：明确本轮不可用。
        assert!(prompt.runtime.contains("对话绑定的 SSH 主机"));
        assert!(prompt.runtime.contains("构建机"));
        assert!(prompt.runtime.contains("/srv/build"));
        assert!(prompt.runtime.contains("日志机"));
        assert!(prompt.runtime.contains("连接超时"));
        assert!(prompt.runtime.contains("默认机"));
        assert!(prompt.runtime.contains("不填 target 时使用它"));
        // 没有默认 SSH 目标时也要说明 ssh__* 属于绑定主机。
        let unbound = prompt_composer::compose(prompt_composer::AgentPromptContext {
            request: &request,
            provider_name: "Provider",
            model_id: "model",
            protocol: ProviderKind::DeepSeek,
            tool_policy: AgentToolPolicy::Auto,
            model_supports_tools: true,
            request_tools: &[],
            selinux_enabled: false,
            selinux_has_roots: false,
            skill_instructions: &[],
            skill_catalog: None,
            is_ssh_target: false,
            is_wsl_target: false,
            default_environment: AgentToolEnvironment::Windows,
            ssh_root: None,
            wsl_root: None,
            bound_ssh_hosts: &[],
            default_ssh_host: None,
            workspace_context: None,
            global_instructions: &[],
        });
        assert!(!unbound.runtime.contains("对话绑定的 SSH 主机"));
    }

    #[test]
    fn every_protocol_receives_both_workspaces_independent_of_default_target() {
        let mut request = request();
        request.workspace_path = Some(env!("CARGO_MANIFEST_DIR").into());
        agent_core::bind_workspace_artifact(&mut request).unwrap();
        let local = super::super::turn_workspace_context(&request, "ask").unwrap();
        for protocol in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
            ProviderKind::OpenAiCompatible,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
            ProviderKind::Glm,
            ProviderKind::MiMo,
        ] {
            for default in [AgentToolEnvironment::Windows, AgentToolEnvironment::Ssh] {
                request.execution_target = Some(agent_core::AgentExecutionTarget {
                    kind: if default == AgentToolEnvironment::Ssh {
                        "ssh"
                    } else {
                        "local"
                    }
                    .into(),
                    root: Some("/remote-aosp".into()),
                    connection_id: Some("connected-host".into()),
                    profile_id: None,
                });
                let prompt = prompt_composer::compose(prompt_composer::AgentPromptContext {
                    request: &request,
                    provider_name: "Provider",
                    model_id: "model",
                    protocol,
                    tool_policy: AgentToolPolicy::Auto,
                    model_supports_tools: true,
                    request_tools: &[],
                    selinux_enabled: false,
                    selinux_has_roots: false,
                    skill_instructions: &[],
                    skill_catalog: None,
                    is_ssh_target: default == AgentToolEnvironment::Ssh,
                    is_wsl_target: false,
                    default_environment: default,
                    ssh_root: Some("/remote-aosp"),
                    wsl_root: None,
                    bound_ssh_hosts: &[],
                    default_ssh_host: None,
                    workspace_context: Some(&local),
                    global_instructions: &[],
                });
                assert!(prompt.runtime.contains("Windows 本机项目"));
                assert!(prompt
                    .runtime
                    .contains(&serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap()));
                assert!(prompt.runtime.contains("/remote-aosp"));
                assert!(prompt.runtime.contains("同一轮可以交替调用"));
                assert!(prompt.runtime.contains("跨主机传文件必须显式上传或下载"));
                assert!(!prompt
                    .runtime
                    .contains("只有用户明确要求操作 Windows 本机时"));
            }
        }
        let prompt = compose(&request, &[]);
        assert!(prompt.runtime.contains("Windows 本机项目"));
        assert!(!prompt.runtime.contains("`ssh__*` 表示当前已连接"));
        assert!(!prompt.runtime.contains("同一轮可以交替调用"));
    }

    #[test]
    fn native_transcript_scope_separates_both_local_projects_and_connected_hosts() {
        let provider: AgentProviderSettings = serde_json::from_value(json!({
            "id":"p", "name":"Provider", "kind":"anthropic", "baseUrl":"https://example.invalid", "model":"m"
        })).unwrap();
        let mut request = request();
        request.workspace_path = Some("D:/project-a".into());
        let original = scope(
            &provider,
            "m",
            ProviderKind::Anthropic,
            &request,
            "/remote",
            Some("host-a"),
        );
        request.workspace_path = Some("D:/project-b".into());
        assert_ne!(
            original,
            scope(
                &provider,
                "m",
                ProviderKind::Anthropic,
                &request,
                "/remote",
                Some("host-a")
            )
        );
        request.workspace_path = Some("D:/project-a".into());
        assert_ne!(
            original,
            scope(
                &provider,
                "m",
                ProviderKind::Anthropic,
                &request,
                "/remote",
                Some("host-b")
            )
        );
        assert_ne!(
            original,
            scope(
                &provider,
                "m",
                ProviderKind::Anthropic,
                &request,
                "D:/project-a",
                None
            )
        );
        assert_eq!(
            original,
            scope(
                &provider,
                "m",
                ProviderKind::Anthropic,
                &request,
                "/remote",
                Some("host-a")
            )
        );
    }

    #[test]
    fn restoring_model_history_does_not_reinject_project_config_into_prefix() {
        let mut request = request();
        request.context.push(AgentContextTurn {
            role: "system".into(),
            content: "project config".into(),
            source: Some("project_configuration".into()),
            message_id: None,
            turn_id: None,
            model_id: None,
            attachments: vec![],
            artifacts: vec![],
        });
        let original = vec![
            AgentMessage::user("old question"),
            AgentMessage::system("old runtime"),
            final_assistant("original answer", "", vec![], vec![], vec![]),
        ];
        let (full, compact) = initial_messages(
            Some(original.clone()),
            &request,
            "next question",
            "t2",
            AgentMessage::user("next question"),
            10000,
            0,
        );
        assert_eq!(&full[..original.len()], original.as_slice());
        assert_eq!(compact.messages.len(), original.len() + 1);
        assert!(compact
            .messages
            .iter()
            .all(|message| message.content != "project config"));
    }
}

pub(super) fn retain_runtime(messages: &mut Vec<AgentMessage>, runtime: &str) {
    // P4-C：带 replace 声明前缀的 runtime 消息与裸 runtime 内容都要识别——
    // 同内容重复调用不得重复追加（update_runtime 的幂等语义）。
    let declared =
        format!("[runtime 指引已更新——以下内容取代此前提供的全部 runtime 指引]\n{runtime}");
    if !messages
        .iter()
        .any(|m| m.role == MessageRole::System && (m.content == runtime || m.content == declared))
    {
        messages.push(AgentMessage::system(runtime));
    }
}

/// Plugin preparation and steering can change runtime guidance. Carry the new
/// configuration after the transcript without rewriting its cached prefix.
/// P4-C（codex 同款语义）：变更是「替换声明」——新消息明示取代此前提供的
/// runtime 指引，让模型不会把旧配置当作仍有效；历史本体依然 append-only。
pub(super) fn update_runtime(
    messages: &mut Vec<AgentMessage>,
    current: &mut String,
    next: String,
) -> usize {
    if *current != next {
        *current = next;
        messages.push(AgentMessage::system(format!(
            "[runtime 指引已更新——以下内容取代此前提供的全部 runtime 指引]\n{}",
            current
        )));
    } else {
        retain_runtime(messages, current);
    }
    agent_core::estimate_messages_tokens(&[AgentMessage::system(current.clone())])
}
