use super::*;

pub(crate) fn update_agent_run_status(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.update_agent_run_status(session_id, turn_id, status, message)
}

pub(crate) fn finish_agent_run(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.finish_agent_run(session_id, turn_id, status, message)
}

pub(crate) fn finish_agent_turn(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
    assistant_content: Option<&str>,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.finish_agent_turn(session_id, turn_id, status, message, assistant_content)
}

pub(crate) fn request_for_interrupted_agent_run(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
) -> Result<AgentChatRequest, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let store = open_store(app)?;
    let run = store.agent_run(session_id, turn_id)?
        .ok_or_else(|| "没有找到可继续的 Agent 运行记录".to_string())?;
    if run.summary.status != "interrupted" || run.summary.finished_at.is_some() {
        return Err("该任务不是可继续的中断状态".to_string());
    }
    let mut request = serde_json::from_value(run.request)
        .map_err(|error| format!("保存的 Agent 请求无法恢复: {error}"))?;
    let session = store.get_record(session_id)?;
    let project_root = session.as_ref().and_then(|session| session.metadata["workspacePath"].as_str());
    agent_core::normalize_local_project_target(&mut request, project_root);
    Ok(request)
}

pub(crate) fn agent_request_for_run(
    app: &AppHandle,
    session: &str,
    turn: &str,
) -> Result<AgentChatRequest, String> {
    let run = open_store(app)?
        .agent_run(session, turn)?
        .ok_or("没有找到 Agent 运行记录".to_string())?;
    serde_json::from_value(run.request).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn list_agent_runs(
    app: AppHandle,
    session_id: Option<String>,
) -> Result<Vec<AgentRunSummary>, String> {
    open_store(&app)?
        .list_agent_runs(session_id.as_deref())
        .map(|runs| runs.into_iter().map(|run| run.summary).collect())
}

#[tauri::command(async)]
pub fn recover_interrupted_agent_runs(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
) -> Result<Vec<AgentRunSummary>, String> {
    let active = state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .iter()
        .map(|(session_id, control)| (session_id.clone(), control.turn_id.clone()))
        .collect::<HashSet<_>>();
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.recover_interrupted_runs(&active)
}

pub(crate) fn claim_collaboration_agent(
    app: &AppHandle,
    job_id: &str,
    agent_id: &str,
    lease_owner: &str,
) -> Result<CollaborationDispatch, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.claim_collaboration_agent(job_id, agent_id, lease_owner)
}

pub(crate) fn finish_collaboration_agent(
    app: &AppHandle,
    job_id: &str,
    agent_id: &str,
    attempt: i64,
    lease_id: &str,
    status: &str,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<CollaborationJobView, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?
        .finish_collaboration_agent(job_id, agent_id, attempt, lease_id, status, summary, error)
}

pub(crate) fn release_collaboration_agent_claim(
    app: &AppHandle,
    job_id: &str,
    agent_id: &str,
    attempt: i64,
    lease_id: &str,
) -> Result<bool, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.release_collaboration_agent_claim(job_id, agent_id, attempt, lease_id)
}

pub(crate) fn collaboration_run_lease(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
) -> Result<Option<(String, String, i64, String)>, String> {
    open_store(app)?
        .connection
        .query_row(
            "SELECT a.job_id, a.id, a.attempt, a.lease_id FROM collaboration_agents a
         JOIN collaboration_jobs j ON j.id = a.job_id
         WHERE a.session_id = ?1 AND a.turn_id = ?2 AND a.status = 'running'
           AND a.lease_id IS NOT NULL AND a.cancel_requested = 0 AND j.cancel_requested = 0",
            params![session_id, turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| format!("无法读取协作轮次权限: {error}"))
}

pub(crate) fn is_collaboration_session(app: &AppHandle, session_id: &str) -> Result<bool, String> {
    open_store(app)?
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM collaboration_agents WHERE session_id = ?1)",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法检查协作会话归属: {error}"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_collaboration_agent_run(
    app: &AppHandle,
    job_id: &str,
    agent_id: &str,
    attempt: i64,
    lease_id: &str,
    session_id: &str,
    turn_id: &str,
    request: &AgentChatRequest,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.start_collaboration_agent_run(
        job_id, agent_id, attempt, lease_id, session_id, turn_id, request,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_collaboration_agent_turn(
    app: &AppHandle,
    job_id: &str,
    agent_id: &str,
    attempt: i64,
    lease_id: &str,
    session_id: &str,
    turn_id: &str,
    status: &str,
    message: Option<&str>,
    assistant_content: Option<&str>,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<CollaborationJobView, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.finish_collaboration_agent_turn(
        job_id,
        agent_id,
        attempt,
        lease_id,
        session_id,
        turn_id,
        status,
        message,
        assistant_content,
        summary,
        error,
    )
}

pub(crate) fn request_collaboration_cancellation(
    app: &AppHandle,
    thread_id: &str,
    parent_turn_id: Option<&str>,
) -> Result<Vec<CollaborationCancellationTarget>, String> {
    let write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let targets = open_store(app)?.request_collaboration_cancellation(thread_id, parent_turn_id)?;
    drop(write_guard);
    crate::collaboration::cancel_scheduled_workers(&targets);
    Ok(targets)
}

pub(crate) fn collaboration_job_view(
    app: &AppHandle,
    job_id: &str,
) -> Result<Option<CollaborationJobView>, String> {
    open_store(app)?.collaboration_job_view(job_id)
}

pub(crate) fn collaboration_fan_in_state(
    app: &AppHandle,
    job_id: &str,
) -> Result<Option<CollaborationFanInState>, String> {
    open_store(app)?.collaboration_fan_in_state(job_id)
}

#[tauri::command]
pub fn create_collaboration_job(
    app: AppHandle,
    input: CollaborationJobInput,
) -> Result<CollaborationJobView, String> {
    let write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let job = open_store(&app)?.create_collaboration_job(input)?;
    drop(write_guard);
    let _ = app.emit(
        "collaboration:job-created",
        serde_json::json!({
            "jobId": job.job.id,
            "threadId": job.job.thread_id,
        }),
    );
    if let Err(error) = crate::collaboration::dispatch_collaboration_job(&app, &job.job.id) {
        // Creation is already committed. Keep the durable job visible so a
        // later recovery pass or explicit retry can dispatch it without the
        // caller accidentally creating a duplicate job.
        eprintln!("协作作业已保存，但首次调度失败: {error}");
    }
    Ok(job)
}

#[tauri::command]
pub fn get_collaboration_job(
    app: AppHandle,
    job_id: String,
) -> Result<Option<CollaborationJobView>, String> {
    collaboration_job_view(&app, &job_id)
}

#[cfg(test)]
pub(crate) fn create_collaboration_job_without_dispatch(
    app: &AppHandle,
    input: CollaborationJobInput,
) -> Result<CollaborationJobView, String> {
    open_store(app)?.create_collaboration_job(input)
}

#[tauri::command]
pub fn list_collaboration_jobs(
    app: AppHandle,
    thread_id: Option<String>,
) -> Result<Vec<CollaborationJobView>, String> {
    open_store(&app)?.list_collaboration_jobs(thread_id.as_deref())
}

#[tauri::command(async)]
pub fn recover_collaboration_agents(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
) -> Result<Vec<CollaborationAgentSummary>, String> {
    let active_sessions = state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let recovered = open_store(&app)?
        .recover_collaboration_agents(&active_sessions, collaboration_lease_owner())?;
    drop(write_guard);
    let job_ids = recovered
        .iter()
        .map(|agent| agent.job_id.clone())
        .collect::<HashSet<_>>();
    for job_id in job_ids {
        crate::collaboration::dispatch_collaboration_job(&app, &job_id)?;
    }
    Ok(recovered)
}
