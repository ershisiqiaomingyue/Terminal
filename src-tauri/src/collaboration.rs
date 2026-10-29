use crate::history::{
    self, CollaborationAgentSummary, CollaborationDispatch, CollaborationJobView,
    HistorySessionInput, HISTORY_SCHEMA_VERSION,
};
use agent_core::{
    AgentChatRequest, AgentChatResponse, AgentContextTurn, AgentExecutionTarget, LocalArtifact,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

type WorkerKey = (String, String);

struct WorkerSignal {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl WorkerSignal {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

struct WorkerScheduler {
    slots: Arc<tokio::sync::Semaphore>,
    scheduled: Mutex<HashMap<WorkerKey, Arc<WorkerSignal>>>,
}

impl WorkerScheduler {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(tokio::sync::Semaphore::new(capacity)),
            scheduled: Mutex::new(HashMap::new()),
        })
    }

    fn reserve(
        self: &Arc<Self>,
        job_id: &str,
        agent_id: &str,
    ) -> Result<WorkerReservation, String> {
        let key = (job_id.to_string(), agent_id.to_string());
        let signal = Arc::new(WorkerSignal::new());
        let mut scheduled = self
            .scheduled
            .lock()
            .map_err(|_| "协作调度状态不可用".to_string())?;
        if scheduled.contains_key(&key) {
            return Err("该子 Agent 已经被领取或正在排队".to_string());
        }
        scheduled.insert(key.clone(), signal.clone());
        Ok(WorkerReservation {
            scheduler: self.clone(),
            key,
            signal,
        })
    }

    fn cancel(&self, job_id: &str, agent_id: &str) {
        let signal = self.scheduled.lock().ok().and_then(|scheduled| {
            scheduled
                .get(&(job_id.to_string(), agent_id.to_string()))
                .cloned()
        });
        if let Some(signal) = signal {
            signal.cancel();
        }
    }
}

fn worker_scheduler() -> &'static Arc<WorkerScheduler> {
    static SCHEDULER: OnceLock<Arc<WorkerScheduler>> = OnceLock::new();
    SCHEDULER.get_or_init(|| WorkerScheduler::new(history::MAX_COLLABORATION_CONCURRENCY))
}

struct WorkerReservation {
    scheduler: Arc<WorkerScheduler>,
    key: WorkerKey,
    signal: Arc<WorkerSignal>,
}

impl WorkerReservation {
    async fn acquire_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        let slot = self.scheduler.slots.clone().acquire_owned();
        tokio::pin!(slot);
        tokio::select! {
            biased;
            _ = self.signal.cancelled() => Err("协作子 Agent 已取消，未开始执行".to_string()),
            result = &mut slot => result.map_err(|_| "协作调度已关闭".to_string()),
        }
    }

    async fn cancelled(&self) {
        self.signal.cancelled().await;
    }
}

impl Drop for WorkerReservation {
    fn drop(&mut self) {
        if let Ok(mut scheduled) = self.scheduler.scheduled.lock() {
            let owns_reservation = scheduled
                .get(&self.key)
                .is_some_and(|signal| Arc::ptr_eq(signal, &self.signal));
            if owns_reservation {
                scheduled.remove(&self.key);
            }
        }
    }
}

pub(crate) fn cancel_scheduled_workers(targets: &[history::CollaborationCancellationTarget]) {
    for target in targets {
        worker_scheduler().cancel(&target.job_id, &target.agent_id);
    }
}

struct ClaimedWorker {
    app: AppHandle,
    job_id: String,
    agent_id: String,
    attempt: i64,
    lease_id: String,
    armed: bool,
}

impl ClaimedWorker {
    fn new(app: &AppHandle, dispatch: &CollaborationDispatch) -> Self {
        Self {
            app: app.clone(),
            job_id: dispatch.agent.job_id.clone(),
            agent_id: dispatch.agent.id.clone(),
            attempt: dispatch.attempt,
            lease_id: dispatch.lease_id.clone(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ClaimedWorker {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = history::release_collaboration_agent_claim(
            &self.app,
            &self.job_id,
            &self.agent_id,
            self.attempt,
            &self.lease_id,
        ) {
            eprintln!("无法释放协作子 Agent lease: {error}");
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationAgentRunResult {
    pub job: CollaborationJobView,
    pub agent: CollaborationAgentSummary,
    pub response: AgentChatResponse,
}

/// Starts every durable worker that is still runnable. SQLite's atomic claim
/// remains the arbiter, so duplicate UI refreshes or recovery passes cannot
/// execute the same worker twice.
pub(crate) fn dispatch_collaboration_job(app: &AppHandle, job_id: &str) -> Result<usize, String> {
    let job = history::collaboration_job_view(app, job_id)?
        .ok_or_else(|| "协作作业不存在".to_string())?;
    let runnable = job
        .agents
        .into_iter()
        .filter(|agent| matches!(agent.status.as_str(), "queued" | "interrupted"))
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    for agent_id in &runnable {
        let worker_app = app.clone();
        let worker_job_id = job_id.to_string();
        let worker_agent_id = agent_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) =
                run_collaboration_agent(worker_app, worker_job_id, worker_agent_id).await
            {
                // Another dispatcher winning the durable claim is expected;
                // actual worker failures are persisted by run_collaboration_agent.
                if !error.contains("已经被领取")
                    && !error.contains("当前状态")
                    && !error.contains("已取消")
                {
                    eprintln!("协作子 Agent 执行失败: {error}");
                }
            }
        });
    }
    Ok(runnable.len())
}

pub(crate) fn recover_and_dispatch_collaboration_jobs(app: &AppHandle) -> Result<usize, String> {
    let state = app.state::<agent_core::AgentRuntimeState>();
    let recovered = history::recover_collaboration_agents(app.clone(), state)?;
    Ok(recovered.len())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn execution_target(value: Option<Value>) -> Result<Option<AgentExecutionTarget>, String> {
    value
        .map(|target| {
            serde_json::from_value(target)
                .map_err(|error| format!("协作作业保存的执行目标无效: {error}"))
        })
        .transpose()
}

fn worker_prompt(dispatch: &CollaborationDispatch) -> String {
    let work_domain = dispatch
        .agent
        .workspace_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or("未绑定本地目录");
    let mut prompt = format!(
        "你正在执行持久化协作作业中的独立子任务。\n\n总目标：{}\n\n你的职责（{}）：{}\n\n固定工作域：{}\n权限模式：{}\n\n只在此子任务的会话、工作域和权限范围内工作。不要假设能够访问父对话的未提供内容、历史授权或其他子 Agent 的结果。完成后给出可供主任务汇总的结论、证据、已完成操作、未解决问题与下一步建议。",
        dispatch.job_goal,
        dispatch.agent.title,
        dispatch.instruction,
        work_domain,
        dispatch.agent.permission_mode,
    );
    if dispatch.attempt > 1 {
        prompt.push_str("\n\n这是中断后的新执行尝试，原会话历史仍然保留。先结合历史结果和当前工作区核实进度，再继续尚未完成的部分。不要重复已完成的操作；对于未收到结果的写入指令，必须先核实实际状态，不能仅因缺少结果就重新执行。保持用户要求的语言，避免重复打印已有结论。");
    }
    prompt
}

fn worker_request(dispatch: &CollaborationDispatch) -> Result<AgentChatRequest, String> {
    let target = execution_target(dispatch.execution_target.clone())?;
    Ok(AgentChatRequest {
        model_id: dispatch.agent_config.model_id.clone(),
        message: worker_prompt(dispatch),
        jira_context: None,
        artifacts: Vec::<LocalArtifact>::new(),
        workspace_path: dispatch.agent.workspace_path.clone(),
        session_id: Some(dispatch.agent.session_id.clone()),
        attachments: Vec::new(),
        turn_id: Some(dispatch.agent.turn_id.clone()),
        permission_mode: Some(dispatch.agent.permission_mode.clone()),
        analysis_mode: dispatch.agent_config.analysis_mode,
        context: vec![AgentContextTurn {
            role: "user".to_string(),
            content: format!(
                "协作作业 {} 已把本子任务分派给你。不要接收或复用父对话上下文；任务说明和总目标已包含在当前请求中。",
                dispatch.agent.job_id
            ),
            source: Some("collaboration_job".to_string()),
            message_id: None,
            turn_id: Some(dispatch.agent.turn_id.clone()),
            model_id: Some(dispatch.agent_config.model_id.clone()),
            attachments: Vec::new(),
            artifacts: Vec::new(),
        }],
        tool_policy: dispatch.agent_config.tool_policy,
        skill_ids: dispatch.agent_config.skill_ids.clone(),
        requested_tool: dispatch.agent_config.requested_tool.clone(),
        reasoning_effort: dispatch.agent_config.reasoning_effort.clone(),
        task_plan_mode: dispatch.agent_config.task_plan_mode.clone(),
        task_plan_min_steps: dispatch.agent_config.task_plan_min_steps,
        execution_target: target,
        execution_targets: Vec::new(),
        // G2-D：把委派层数带进子智能体的 turn，供 create_collaboration_job 判定。
        collaboration_depth: Some(dispatch.agent_config.depth),
    })
}

fn history_session(
    dispatch: &CollaborationDispatch,
    request: &AgentChatRequest,
    assistant: Option<&str>,
    response: Option<&AgentChatResponse>,
) -> HistorySessionInput {
    let updated_at = now_ms();
    let mut messages = vec![json!({
        "id": format!("{}-request", dispatch.agent.turn_id),
        "role": "user",
        "content": request.message,
        "createdAt": dispatch.agent.created_at,
        "turnId": dispatch.agent.turn_id,
    })];
    if let Some(assistant) = assistant {
        messages.push(json!({
            "id": format!("{}-result", dispatch.agent.turn_id),
            "role": "assistant",
            "content": assistant,
            "createdAt": updated_at,
            "finishedAt": updated_at,
            "turnId": dispatch.agent.turn_id,
        }));
    }
    HistorySessionInput {
        id: dispatch.agent.session_id.clone(),
        title: format!("协作 · {}", dispatch.agent.title),
        updated_at,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: Value::Array(Vec::new()),
        messages: Value::Array(messages),
        metadata: json!({
            "scope": "collaboration_agent",
            "collaboration": {
                "jobId": dispatch.agent.job_id,
                "agentId": dispatch.agent.id,
                "workspacePath": dispatch.agent.workspace_path,
                "permissionMode": dispatch.agent.permission_mode,
                "executionTarget": dispatch.execution_target,
            },
        }),
        report: response
            .and_then(|value| value.report.as_ref())
            .and_then(|value| serde_json::to_value(value).ok()),
        usage: response
            .and_then(|value| value.usage.as_ref())
            .and_then(|value| serde_json::to_value(value).ok()),
        // Agent terminal events are appended independently and must survive this upsert.
        events: None,
    }
}

fn finish_failed(
    app: &AppHandle,
    dispatch: &CollaborationDispatch,
    status: &str,
    error: &str,
) -> bool {
    let atomic_result = history::finish_collaboration_agent_turn(
        app,
        &dispatch.agent.job_id,
        &dispatch.agent.id,
        dispatch.attempt,
        &dispatch.lease_id,
        &dispatch.agent.session_id,
        &dispatch.agent.turn_id,
        status,
        Some(error),
        Some(error),
        None,
        Some(error),
    );
    if atomic_result.is_ok() {
        return true;
    }
    // Failures before the Agent run record is created still need to close the
    // claimed worker. This fenced fallback cannot commit a stale attempt.
    match history::finish_collaboration_agent(
        app,
        &dispatch.agent.job_id,
        &dispatch.agent.id,
        dispatch.attempt,
        &dispatch.lease_id,
        status,
        None,
        Some(error),
    ) {
        Ok(_) => true,
        Err(finish_error) => {
            eprintln!("无法保存协作子 Agent 终态: {finish_error}");
            false
        }
    }
}

/// Executes exactly one durable worker. The job's persisted dispatch record,
/// rather than an untrusted caller payload, owns its session, task text,
/// workspace, permissions, model settings, and execution target.
#[tauri::command]
pub async fn run_collaboration_agent(
    app: AppHandle,
    job_id: String,
    agent_id: String,
) -> Result<CollaborationAgentRunResult, String> {
    let reservation = worker_scheduler().reserve(&job_id, &agent_id)?;
    // Acquire capacity before the durable claim: waiting never starts a lease.
    let _slot = reservation.acquire_slot().await?;
    let dispatch = loop {
        match history::claim_collaboration_agent(
            &app,
            &job_id,
            &agent_id,
            history::collaboration_lease_owner(),
        ) {
            Ok(dispatch) => break dispatch,
            Err(error) if error == history::COLLABORATION_CAPACITY_ERROR => {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                    _ = reservation.cancelled() => {
                        return Err("协作子 Agent 已取消，未开始执行".to_string());
                    }
                }
            }
            Err(error) => return Err(error),
        }
    };
    // Declared after the permit so an aborted future clears the durable claim
    // before making its in-process slot available to another worker.
    let mut claimed_worker = ClaimedWorker::new(&app, &dispatch);
    let request = match worker_request(&dispatch) {
        Ok(request) => request,
        Err(error) => {
            if finish_failed(&app, &dispatch, "failed", &error) {
                claimed_worker.disarm();
            }
            return Err(error);
        }
    };
    let history_exists =
        match history::get_history_session(app.clone(), dispatch.agent.session_id.clone()) {
            Ok(value) => value.is_some(),
            Err(error) => {
                if finish_failed(&app, &dispatch, "failed", &error) {
                    claimed_worker.disarm();
                }
                return Err(error);
            }
        };
    if !history_exists {
        if let Err(error) = history::save_history_session(
            app.clone(),
            history_session(&dispatch, &request, None, None),
        ) {
            if finish_failed(&app, &dispatch, "failed", &error) {
                claimed_worker.disarm();
            }
            return Err(error);
        }
    }
    match crate::agent::execute_collaboration_agent_request(
        app.clone(),
        request.clone(),
        dispatch.agent.job_id.clone(),
        dispatch.agent.id.clone(),
        dispatch.attempt,
        dispatch.lease_id.clone(),
    )
    .await
    {
        Ok(response) => {
            let job = history::collaboration_job_view(&app, &dispatch.agent.job_id)?
                .ok_or_else(|| "协作作业结束后无法读取".to_string())?;
            let agent = job
                .agents
                .iter()
                .find(|agent| agent.id == dispatch.agent.id)
                .cloned()
                .ok_or_else(|| "协作子 Agent 结束后无法读取其状态".to_string())?;
            if agent.status != "running" {
                claimed_worker.disarm();
            }
            if agent.status != "completed" {
                return Err(format!(
                    "协作子 Agent 返回结果后未进入完成状态（{}）",
                    agent.status
                ));
            }
            Ok(CollaborationAgentRunResult {
                job,
                agent,
                response,
            })
        }
        Err(error) => {
            let status = if error == "请求已取消" || error.contains("已请求取消") {
                "cancelled"
            } else {
                "failed"
            };
            if finish_failed(&app, &dispatch, status, &error) {
                claimed_worker.disarm();
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_waiter_releases_reservation_without_consuming_permit() {
        let scheduler = WorkerScheduler::new(1);
        let running = scheduler
            .reserve("job-a", "agent-a")
            .expect("first worker should reserve");
        let running_slot = running
            .acquire_slot()
            .await
            .expect("first worker should acquire the only permit");
        let waiting = scheduler
            .reserve("job-b", "agent-b")
            .expect("second worker should queue");

        scheduler.cancel("job-b", "agent-b");
        let error = waiting
            .acquire_slot()
            .await
            .expect_err("cancelled waiter must not wait for a permit");
        assert!(error.contains("已取消"));
        assert_eq!(scheduler.slots.available_permits(), 0);
        drop(waiting);
        let replacement = scheduler
            .reserve("job-b", "agent-b")
            .expect("cancelled reservation should be reusable");

        drop(running_slot);
        let replacement_slot =
            tokio::time::timeout(Duration::from_secs(1), replacement.acquire_slot())
                .await
                .expect("released permit should wake the next worker")
                .expect("replacement should acquire released permit");
        drop(replacement_slot);
        assert_eq!(scheduler.slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn duplicate_reservation_does_not_replace_original_cancellation_signal() {
        let scheduler = WorkerScheduler::new(1);
        let original = scheduler
            .reserve("job-a", "agent-a")
            .expect("worker should reserve");
        assert!(scheduler.reserve("job-a", "agent-a").is_err());

        scheduler.cancel("job-a", "agent-a");
        let error = original
            .acquire_slot()
            .await
            .expect_err("original reservation should receive cancellation");
        assert!(error.contains("已取消"));
        assert_eq!(scheduler.slots.available_permits(), 1);
    }
}
