use super::super::*;

const BOUND_HOST_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(in crate::agent::turn) async fn prepare_turn_environments(
    app: &AppHandle,
    request: &AgentChatRequest,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
    is_ssh_target: bool,
    is_wsl_target: bool,
) -> Result<TurnEnvironments, String> {
    if request.tool_policy == AgentToolPolicy::Disabled
        && turn_id.starts_with(agent_core::background::BACKGROUND_RESULT_PREFIX)
    {
        return Ok(TurnEnvironments::empty());
    }
    emit_agent_status_for_turn(
        &app,
        &session_id,
        "preparing",
        if is_ssh_target {
            "正在准备 SSH 远程工具"
        } else if is_wsl_target {
            "正在准备 WSL 工具"
        } else {
            "正在准备本地工具"
        },
        Some(&turn_id),
    );
    // P1-A/P1-B：阶段边界快速短路。外层 turn.rs 的 select! 负责打断卡住的
    // await；这些检查覆盖「标志已置位但令牌分支尚未触发」的窗口，避免取消
    // 后继续做无用的准备工作。必须返回精确的「请求已取消」，让外层按取消
    // 终态收尾。
    if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    if is_ssh_target && request.tool_policy != AgentToolPolicy::Disabled {
        let connection_id = request
            .execution_target
            .as_ref()
            .and_then(|target| target.connection_id.as_deref())
            .filter(|id| !id.trim().is_empty())
            .ok_or("SSH 执行目标缺少连接 ID")?;
        let answer = ssh_connection::ensure_connected(
            app,
            session_id,
            turn_id,
            cancellation.clone(),
            Some(connection_id),
            &format!("ssh-connect:{turn_id}"),
        )
        .await?;
        if answer["status"] != "connected" {
            return Err("已跳过 SSH 连接，本轮远程操作未执行。".into());
        }
    }
    if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    let prepared_ssh =
        crate::agent::turn_support::prepare_agent_ssh_context(app, request, cancellation.clone())
            .await?;
    let (remote_root, ssh_connection_id) = match prepared_ssh {
        Some((context, connection_id)) => (Some(context), Some(connection_id)),
        None => (None, None),
    };
    let default_ssh_host = ssh_connection_id.as_deref().map(|connection_id| {
        let name = crate::ssh::load_connection_for_agent(app, connection_id)
            .map(|(config, _)| config.name)
            .unwrap_or_else(|_| connection_id.to_string());
        (name, connection_id.to_string())
    });
    if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    let (wsl_root, wsl_local_context) = if is_wsl_target {
        let target = request
            .execution_target
            .as_ref()
            .ok_or_else(|| "WSL 执行目标缺少配置".to_string())?;
        let profile_id = target
            .profile_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "WSL 执行目标缺少配置 ID".to_string())?;
        let profile = crate::wsl::load_profile_for_agent(&app, profile_id)?;
        let root = target
            .root
            .clone()
            .or(profile.default_root.clone())
            .unwrap_or_else(|| "/".to_string());
        let runtime = WslRuntimeConfig {
            distribution: profile.distribution,
            root: root.clone(),
        }
        .normalized()?;
        let context = LocalToolContext::new(
            vec![runtime.root.clone()],
            Vec::new(),
            false,
            cancellation.clone(),
            None,
        )
        .with_wsl(runtime.clone());
        (Some(runtime.root), Some(context))
    } else {
        (None, None)
    };
    // 该对话额外绑定的 SSH 主机：默认目标之外的主机各自建立独立远程上下文，
    // 供远程工具的 `target` 参数点名。任何一台准备失败都不影响本轮启动。
    let mut bound_ssh_hosts = Vec::new();
    if request.tool_policy != AgentToolPolicy::Disabled {
        for connection_id in bound_ssh_connection_ids(request, ssh_connection_id.as_deref()) {
            // 绑定主机是串行连接的，每台开始前都复查一次取消，避免取消后
            // 继续等下一台最长 60 秒的连接窗口。
            if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            bound_ssh_hosts.push(
                prepare_bound_ssh_host(
                    app,
                    request,
                    session_id,
                    turn_id,
                    cancellation,
                    &connection_id,
                )
                .await,
            );
        }
    }
    Ok(TurnEnvironments {
        ssh_context: remote_root,
        ssh_connection_id,
        wsl_root,
        wsl_context: wsl_local_context,
        default_ssh_host,
        bound_ssh_hosts,
    })
}

/// 从请求里挑出「该对话绑定的附加 SSH 主机」：跳过默认目标与重复项。
fn bound_ssh_connection_ids(request: &AgentChatRequest, default_id: Option<&str>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for target in &request.execution_targets {
        if !target.kind.eq_ignore_ascii_case("ssh") {
            continue;
        }
        let Some(connection_id) = target
            .connection_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if Some(connection_id) == default_id || ids.iter().any(|value| value == connection_id) {
            continue;
        }
        ids.push(connection_id.to_string());
    }
    ids
}

/// 连接一台绑定的附加主机并准备它的远程上下文；失败时返回带原因的占位项。
async fn prepare_bound_ssh_host(
    app: &AppHandle,
    request: &AgentChatRequest,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
    connection_id: &str,
) -> BoundSshHost {
    let config = match crate::ssh::load_connection_for_agent(app, connection_id) {
        Ok((config, _)) => config,
        Err(error) => {
            return BoundSshHost::unavailable(
                connection_id,
                connection_id,
                format!("读取该主机的 SSH 配置失败：{error}"),
            )
        }
    };
    let name = config.name.clone();
    let requested_root = request
        .execution_targets
        .iter()
        .find(|target| target.connection_id.as_deref() == Some(connection_id))
        .and_then(|target| target.root.clone());
    if let Err(error) =
        connect_bound_ssh_host(app, session_id, turn_id, cancellation, connection_id, &name).await
    {
        return BoundSshHost::unavailable(connection_id, &name, error);
    }
    match crate::agent::turn_support::prepare_ssh_context_for(
        app,
        request,
        cancellation.clone(),
        connection_id,
        requested_root.as_deref(),
    )
    .await
    {
        Ok(context) => BoundSshHost {
            connection_id: connection_id.to_string(),
            name,
            root: context.root().to_string(),
            context: Some(context),
            note: None,
        },
        Err(error) => BoundSshHost::unavailable(connection_id, &name, error),
    }
}

/// 建立（或复用）绑定主机的传输通道，最多等待 `BOUND_HOST_CONNECT_TIMEOUT`。
async fn connect_bound_ssh_host(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    cancellation: &Arc<AtomicBool>,
    connection_id: &str,
    name: &str,
) -> Result<(), String> {
    let runtime = app.state::<crate::ssh::SshRuntimeState>();
    let tasks = app.state::<AndroidTaskManager>();
    let status =
        crate::ssh::connection_progress(&runtime, connection_id)?.map(|progress| progress.status);
    if status.as_deref() != Some("connected") {
        emit_agent_status_for_turn(
            app,
            session_id,
            "connecting",
            &format!("正在连接绑定的 SSH 主机 {name}"),
            Some(turn_id),
        );
        crate::ssh::connect_ssh_host(app, &runtime, &tasks, connection_id, None)?;
        let deadline = std::time::Instant::now() + BOUND_HOST_CONNECT_TIMEOUT;
        loop {
            if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("本轮已取消，未连接该绑定主机。".to_string());
            }
            match crate::ssh::connection_progress(&runtime, connection_id)? {
                Some(progress) if progress.status == "connected" => break,
                Some(progress) if progress.status == "failed" => {
                    return Err(progress
                        .error
                        .or_else(|| Some(progress.message))
                        .unwrap_or_else(|| "SSH 连接失败".to_string()))
                }
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "连接超时（{} 秒），本轮不会暴露该主机的远程工具。",
                    BOUND_HOST_CONNECT_TIMEOUT.as_secs()
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }
    Ok(())
}
