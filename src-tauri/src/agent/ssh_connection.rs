//! Connect only after a real user answer; remote tool permissions remain separate.
use super::*;
use agent_core::SshConnectionConfig;

pub(super) const CONNECT_SSH_TOOL: &str = "request_ssh_connection";
const PAGE_SIZE: usize = 5;
const NEXT_PAGE: &str = "查看下一组 SSH 连接";

pub(super) struct ConnectSshTool {
    pub app: AppHandle,
    pub session_id: String,
    pub turn_id: String,
    pub cancellation: Arc<AtomicBool>,
}

pub(super) fn connected_result(events: &[AgentEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AgentEvent::ToolCallFinished {
            tool_name,
            output,
            is_error: false,
            ..
        } if tool_name == CONNECT_SSH_TOOL && output["status"] == "connected" => output
            ["connectionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        _ => None,
    })
}

fn connection_label(connection: &SshConnectionConfig, index: usize) -> String {
    format!(
        "{}. {} · {}@{}:{}",
        index + 1,
        connection.name,
        connection.username,
        connection.host,
        connection.port
    )
    .chars()
    .take(190)
    .collect()
}

fn page_options(connections: &[SshConnectionConfig], page: usize) -> Vec<String> {
    let mut options = connections
        .iter()
        .enumerate()
        .skip(page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .map(|(index, connection)| connection_label(connection, index))
        .collect::<Vec<_>>();
    if connections.len() > PAGE_SIZE {
        options.push(NEXT_PAGE.into());
    }
    options
}

fn selected_answer<'a>(
    connections: &'a [SshConnectionConfig],
    answer: &str,
) -> Result<&'a SshConnectionConfig, String> {
    let answer = answer.trim();
    let matches = connections
        .iter()
        .enumerate()
        .filter(|(index, connection)| {
            answer == connection_label(connection, *index)
                || answer == connection.id
                || answer == connection.name
                || answer == connection.host
        })
        .map(|(_, connection)| connection)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [connection] => Ok(connection),
        [] => Err("回答未匹配已保存的 SSH 连接；本次未建立连接。请从询问选项中选择。".into()),
        _ => Err("多个 SSH 连接具有相同名称或主机；本次未连接，请选择带序号的完整选项。".into()),
    }
}

fn needs_confirmation(status: Option<&str>) -> bool {
    !matches!(status, Some("connected" | "connecting"))
}

fn connection_candidates(
    mut connections: Vec<SshConnectionConfig>,
    requested_id: Option<&str>,
) -> Result<Vec<SshConnectionConfig>, String> {
    if connections.is_empty() {
        return Err("未配置 SSH 连接，请先在设置中添加连接。".into());
    }
    if let Some(id) = requested_id {
        connections.retain(|connection| connection.id == id);
        if connections.is_empty() {
            return Err(
                "此对话指定的 SSH 连接不存在，请检查已保存的连接；不会切换到其他主机。".into(),
            );
        }
    }
    Ok(connections)
}

pub(super) async fn ensure_connected(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    cancellation: Arc<AtomicBool>,
    requested_id: Option<&str>,
    call_id: &str,
) -> Result<Value, String> {
    let connections =
        connection_candidates(crate::ssh::list_ssh_connections(app.clone())?, requested_id)?;
    let runtime = app.state::<crate::ssh::SshRuntimeState>();
    let existing_id = requested_id
        .map(str::to_owned)
        .or_else(|| crate::ssh::connected_selection_id(&runtime));
    let existing = existing_id
        .as_deref()
        .map(|id| crate::ssh::connection_progress(&runtime, id))
        .transpose()?
        .flatten();
    let connection = if !needs_confirmation(
        existing.as_ref().map(|progress| progress.status.as_str()),
    ) {
        connections
            .iter()
            .find(|connection| Some(connection.id.as_str()) == existing_id.as_deref())
            .cloned()
            .ok_or("SSH 连接配置已被删除")?
    } else {
        let mut page = 0;
        loop {
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".into());
            }
            let answer = user_interaction::AskUserTool { app:app.clone(),session_id:session_id.into(),
                turn_id:turn_id.into() }.execute(json!({
                "question":"远程操作需要 SSH，目前尚未连接。请选择要连接的已保存主机；选择后建立连接，文件与命令操作仍按当前权限确认。",
                "options":page_options(&connections,page),"__agent_call_id":call_id,
            })).await?;
            if answer["status"] != "answered" {
                return Ok(
                    json!({"status":"skipped","instruction":"用户跳过了 SSH 连接，依赖 SSH 的操作未执行；不要猜测目标或继续远程操作。"}),
                );
            }
            let answer = answer["answer"].as_str().unwrap_or_default();
            if answer == NEXT_PAGE && connections.len() > PAGE_SIZE {
                page = (page + 1) % connections.len().div_ceil(PAGE_SIZE);
                continue;
            }
            break selected_answer(&connections, answer)?.clone();
        }
    };
    if cancellation.load(Ordering::Relaxed) {
        return Err("请求已取消".into());
    }
    let progress = crate::ssh::connection_progress(&runtime, &connection.id)?;
    if needs_confirmation(progress.as_ref().map(|progress| progress.status.as_str())) {
        crate::ssh::start_ssh_connection(
            app.clone(),
            runtime.clone(),
            app.state::<crate::tasks::TaskManager>(),
            connection.id.clone(),
            None,
        )?;
    }
    emit_agent_status_for_turn(
        app,
        session_id,
        "connecting_ssh",
        format!("正在连接 SSH · {}", connection.name),
        Some(turn_id),
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".into());
        }
        let progress = crate::ssh::connection_progress(&runtime, &connection.id)?
            .ok_or("SSH 连接状态已丢失")?;
        match progress.status.as_str() {
            "connected" => {
                return Ok(
                    json!({"status":"connected","connectionId":connection.id,"name":connection.name,
                "instruction":"SSH 已连接。后续远程操作使用下一轮提供的 ssh__* 工具及其精确路径；本机工具的默认环境保持原设置。"}),
                )
            }
            "failed" => return Err(progress.error.unwrap_or(progress.message)),
            "disconnected" => return Err("SSH 连接已断开，本次远程操作未执行。".into()),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("等待 SSH 连接超时，本次远程操作未执行。".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[async_trait]
impl ToolHandler for ConnectSshTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(CONNECT_SSH_TOOL,
            "When the user's task needs operations on an SSH host and no SSH workspace is available, ask them to select a saved connection and wait for connection. Do not use for general questions about SSH. No saved connections returns an explicit error. Skipping never connects; this does not approve remote commands or file writes. Call this separately, then use the newly available ssh__* tools in the next round.",
            json!({"type":"object","properties":{},"additionalProperties":false}))
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let call_id = input["__agent_call_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("SSH 连接请求缺少运行时调用标识")?;
        ensure_connected(
            &self.app,
            &self.session_id,
            &self.turn_id,
            self.cancellation.clone(),
            None,
            call_id,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connections() -> Vec<SshConnectionConfig> {
        (0..12).map(|index|serde_json::from_value(json!({"id":format!("id-{index}"),"name":"build-host","host":format!("host-{index}"),"port":22,"username":"user"})).unwrap()).collect()
    }
    #[test]
    fn chooser_covers_all_saved_hosts_without_ambiguous_or_arbitrary_targets() {
        let connections = connections();
        for page in 0..3 {
            let options = page_options(&connections, page);
            assert!(options.len() <= 6);
            for option in options.iter().filter(|option| option.as_str() != NEXT_PAGE) {
                let selected = selected_answer(&connections, option).unwrap();
                assert!(connections
                    .iter()
                    .any(|connection| connection.id == selected.id));
            }
        }
        assert_eq!(
            selected_answer(&connections, "host-11").unwrap().id,
            "id-11"
        );
        assert!(selected_answer(&connections, "build-host").is_err());
        assert!(selected_answer(&connections, "attacker.example").is_err());
        assert!(needs_confirmation(None));
        assert!(needs_confirmation(Some("disconnected")));
        assert!(needs_confirmation(Some("failed")));
        assert!(!needs_confirmation(Some("connected")));
        assert!(!needs_confirmation(Some("connecting")));
    }

    #[test]
    fn missing_configuration_errors_and_existing_projects_never_switch_hosts() {
        assert!(connection_candidates(vec![], None)
            .unwrap_err()
            .contains("未配置"));
        assert!(connection_candidates(connections(), Some("deleted-host"))
            .unwrap_err()
            .contains("不存在"));
        let candidates = connection_candidates(connections(), Some("id-8")).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            selected_answer(&candidates, &page_options(&candidates, 0)[0])
                .unwrap()
                .id,
            "id-8"
        );
        assert!(selected_answer(&candidates, "host-0").is_err());
    }

    #[test]
    fn only_successful_native_connection_results_enable_remote_tools() {
        for (name, status, is_error, expected) in [
            (CONNECT_SSH_TOOL, "connected", false, Some("chosen-host")),
            (CONNECT_SSH_TOOL, "skipped", false, None),
            (CONNECT_SSH_TOOL, "connected", true, None),
            ("plugin__other__tool", "connected", false, None),
        ] {
            let event = AgentEvent::ToolCallFinished {
                session_id: "session".into(),
                call_id: "call".into(),
                tool_name: name.into(),
                input: json!({}),
                output: json!({"status":status,"connectionId":"chosen-host"}),
                is_error,
            };
            assert_eq!(connected_result(&[event]).as_deref(), expected);
        }
    }
}
