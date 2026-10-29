use super::*;

pub(super) struct UpdateTaskPlanTool {
    pub(super) app: Option<AppHandle>,
    pub(super) session_id: Option<String>,
    pub(super) turn_id: Option<String>,
}

#[async_trait]
impl ToolHandler for UpdateTaskPlanTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "update_task_plan",
            "Publish the composer task plan. Call this only when the request needs a multi-step plan with dependent work. Do not call it for a simple question, a single file read, a single search, or any one-step action.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "One sentence describing the goal." },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "running", "done", "error"] }
                            },
                            "required": ["title", "status"]
                        }
                    }
                },
                "required": ["goal", "steps"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if let (Some(app), Some(session_id), Some(turn_id)) =
            (&self.app, &self.session_id, &self.turn_id)
        {
            emit_agent_event_payload_for_turn(
                app,
                json!({
                    "type": "task_plan",
                    "session_id": session_id,
                    "goal": input.get("goal").cloned().unwrap_or(Value::Null),
                    "steps": input.get("steps").cloned().unwrap_or(Value::Null),
                }),
                Some(turn_id),
            );
        }
        Ok(
            json!({ "ok": true, "goal": input.get("goal").cloned().unwrap_or(Value::Null), "steps": input.get("steps").cloned().unwrap_or(Value::Null) }),
        )
    }
}

/// G1：goal 三工具。goal 是跨 turn 的生命周期对象——创建它的 turn 结束后
/// 仍然存活，供后续 turn 查询与推进（对照 codex `ext/goal`，含预算归属）。
pub(super) struct GoalTools {
    pub(super) app: Option<AppHandle>,
    pub(super) session_id: Option<String>,
    /// G5-B：本层还能向下的委派层数，0 表示不再暴露委派能力。
    pub(super) remaining_depth: u8,
}

fn goal_brief(goal: &crate::history::GoalRecord) -> Value {
    json!({
        "goal_id": goal.id,
        "objective": goal.objective,
        "acceptance": goal.acceptance_list(),
        "status": goal.status,
        "blocked_reason": goal.blocked_reason,
        "blocked_rounds": goal.blocked_rounds,
        "round": goal.round,
        "max_rounds": goal.max_rounds,
        "token_budget": goal.token_budget,
        "tokens_used": goal.tokens_used,
        "parent_id": goal.parent_id,
    })
}

#[async_trait]
impl ToolHandler for GoalTools {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "goal",
            "Create, inspect, and advance a persistent goal for this conversation. A goal outlives a single turn: create one only when the user explicitly asks for a long-running objective that must survive across turns. 'create' fails if an unfinished goal already exists — update that one instead. Set 'blocked' only after the same blocking condition has recurred for at least three consecutive goal turns; 'paused' only at the user's explicit request. 'complete' is final and irreversible. Budget limits take precedence over pausing.",
            json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "get", "update"] },
                    "objective": { "type": "string", "description": "create: the concrete objective to start pursuing." },
                    "acceptance": { "type": "array", "items": { "type": "string" }, "description": "create: acceptance conditions that decide completion." },
                    "parent_id": { "type": "string", "description": "create: link this goal under a parent goal." },
                    "token_budget": { "type": "integer", "description": "create: positive token budget; omit unless explicitly requested." },
                    "goal_id": { "type": "string", "description": "update: the goal to advance. get: defaults to the active goal." },
                    "status": { "type": "string", "enum": ["complete", "blocked", "paused", "active", "dropped"], "description": "update: the new status." },
                    "blocked_reason": { "type": "string", "description": "update: required with status=blocked." }
                },
                "required": ["action"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        // G5-B：委派层数已达上限时不再暴露委派能力（工具面由上下文构造点决定）。
        if self.remaining_depth == 0 && input.get("instruction").is_some() {
            return Err(format!(
                "本层已是最后一层委派（最多 {} 层），请直接完成任务。",
                MAX_SUBAGENT_DEPTH
            ));
        }
        let (Some(app), Some(session_id)) = (&self.app, &self.session_id) else {
            return Err("goal 工具需要活跃会话".to_string());
        };
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or("缺少 action")?;
        match action {
            "create" => {
                if crate::history::agent_active_goal(app, session_id)?.is_some() {
                    return Err(
                        "当前会话已有未完成的目标；请用 update 推进它，或先将其 complete/dropped"
                            .into(),
                    );
                }
                let objective = input
                    .get("objective")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or("create 需要非空的 objective")?;
                let acceptance = input
                    .get("acceptance")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                let acceptance = serde_json::to_string(&acceptance).unwrap_or_else(|_| "[]".into());
                let now = crate::history::now_ms_public();
                let goal = crate::history::GoalRecord {
                    id: format!("goal-{}", uuid_suffix()),
                    session_id: session_id.clone(),
                    parent_id: input
                        .get("parent_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    objective: objective.to_string(),
                    acceptance,
                    status: "active".into(),
                    blocked_reason: None,
                    blocked_rounds: 0,
                    round: 0,
                    max_rounds: input.get("max_rounds").and_then(Value::as_u64).unwrap_or(8) as i64,
                    token_budget: input
                        .get("token_budget")
                        .and_then(Value::as_u64)
                        .map(|value| value as i64),
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at: now,
                    updated_at: now,
                };
                crate::history::agent_create_goal(app, &goal)?;
                Ok(json!({ "ok": true, "goal": goal_brief(&goal) }))
            }
            "get" => {
                if let Some(goal_id) = input.get("goal_id").and_then(Value::as_str) {
                    let goal = crate::history::agent_goal_by_id(app, goal_id)?
                        .ok_or("目标不存在或已删除")?;
                    return Ok(json!({ "goal": goal_brief(&goal) }));
                }
                Ok(json!({
                    "goal": crate::history::agent_active_goal(app, session_id)?.as_ref().map(goal_brief),
                }))
            }
            "update" => {
                let goal_id = input
                    .get("goal_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        crate::history::agent_active_goal(app, session_id)
                            .ok()
                            .flatten()
                            .map(|goal| goal.id)
                    })
                    .ok_or("update 需要目标：会话当前没有活跃目标")?;
                let status = input
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or("update 需要 status")?;
                let blocked_reason = input.get("blocked_reason").and_then(Value::as_str);
                if status == "blocked" && blocked_reason.is_none() {
                    return Err("blocked 需要同时提供 blocked_reason".into());
                }
                crate::history::agent_update_goal_status(
                    app,
                    &goal_id,
                    status,
                    blocked_reason,
                    1,
                    0,
                    0,
                )?;
                let goal =
                    crate::history::agent_goal_by_id(app, &goal_id)?.ok_or("目标不存在或已删除")?;
                Ok(json!({ "ok": true, "goal": goal_brief(&goal) }))
            }
            _ => Err("未知 action".to_string()),
        }
    }
}

/// Lets the model create a durable collaboration job after it has interpreted
/// the user's request. Scheduling is performed by the chat UI from the emitted
/// durable job event, so this handler never needs to borrow the active runtime.
#[derive(Clone)]
pub(super) struct CreateCollaborationJobTool {
    pub(super) app: AppHandle,
    pub(super) thread_id: String,
    pub(super) parent_turn_id: String,
    pub(super) model_id: String,
    pub(super) workspace_path: Option<String>,
    pub(super) permission_mode: String,
    pub(super) execution_target: Option<Value>,
    pub(super) analysis_mode: bool,
    pub(super) skill_ids: Vec<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) task_plan_mode: Option<String>,
    pub(super) task_plan_min_steps: Option<u8>,
    pub(super) cancellation: Arc<AtomicBool>,
    /// G2-D：本工具所处的委派层。根 turn 为 0，子智能体每委派一层 +1；
    /// 达到 `MAX_SUBAGENT_DEPTH` 后不再允许继续委派。
    pub(super) depth: u8,
}

/// G2-D：子智能体最多可以再往下委派几层（0 = 根 turn 还能委派 2 层）。
pub(super) const MAX_SUBAGENT_DEPTH: u8 = 2;

impl CreateCollaborationJobTool {
    pub(super) fn with_cancellation(&self, cancellation: Arc<AtomicBool>) -> Self {
        let mut tool = self.clone();
        tool.cancellation = cancellation;
        tool
    }

    /// G2-D：本 turn 还能向下的委派层数；0 表示不允许再委派。
    pub(super) fn remaining_depth(&self) -> u8 {
        MAX_SUBAGENT_DEPTH.saturating_sub(self.depth)
    }
}

#[async_trait]
impl ToolHandler for CreateCollaborationJobTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "create_collaboration_job",
            "Delegate work to sub-agents. Two modes: (1) single delegation — pass `instruction` (plus optional `context_notes` and `wait`) to hand one self-contained task to one sub-agent and, by default, wait for its result, which is returned here as a summary. Use this when one independent subtask would benefit from running as its own agent loop. (2) parallel group — pass `agents` (2-8) for independent streams that benefit from parallel execution; results appear in the side panel. A sub-agent cannot read the parent transcript: every instruction and `context_notes` entry must be self-contained (paths, constraints, expected result). The sub-agent inherits this conversation's workspace and permission boundary and cannot expand it.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "The shared goal for this job (required for the agents mode; optional for single delegation)." },
                    "depth": { "type": "integer", "description": "G2-D single delegation only: how many more delegation layers the sub-agent may start (default 0, max 2). Use 1 only when the subtask genuinely needs its own sub-delegation." },
                    "instruction": { "type": "string", "description": "Single delegation mode: one self-contained task including the expected result." },
                    "context_notes": { "type": "string", "description": "Single delegation mode: what the sub-agent must know from this conversation (it cannot read the parent transcript)." },
                    "wait": { "type": "boolean", "description": "Single delegation mode: wait for the result (default true). false returns the job id immediately." },
                    "agents": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Short responsibility label." },
                                "instruction": { "type": "string", "description": "Self-contained task including the expected result." }
                            },
                            "required": ["title", "instruction"]
                        }
                    }
                }
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        // G2-D：委派层数上限。已到顶时直接拒绝，避免子智能体无限自我复制。
        let remaining_depth = self.remaining_depth();
        if remaining_depth == 0 {
            return Err(format!(
                "委派层数已达上限（最多 {} 层子智能体），请在本层直接完成任务。",
                MAX_SUBAGENT_DEPTH
            ));
        }
        // G2-A 单委托模式：`instruction` 分支——一个自包含任务交给一个子智能体，
        // 默认等待并把结果摘要作为工具结果回注父对话（摘要而非全文，
        // 全文在协作侧栏）。权限边界继承父 turn（只缩不放，同并行组模式）。
        if let Some(instruction) = input
            .get("instruction")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(true);
            // 上下文继承用摘要不用全文（G2 约束 1）：goal 锚点自动附上，
            // 其余由模型在 context_notes 里自己总结——子智能体读不到父对话。
            // goal_id 同时写进 job（预算归属，turn 花费据此回写 goals.tokens_used）。
            let active_goal = crate::history::agent_active_goal(&self.app, &self.thread_id)
                .ok()
                .flatten();
            let goal_id = active_goal.as_ref().map(|goal| goal.id.clone());
            let mut composed = String::new();
            if let Some(goal) = active_goal {
                composed.push_str(&format!("当前会话目标：{}\n", goal.objective));
            }
            composed.push_str(instruction);
            if let Some(notes) = input
                .get("context_notes")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                composed.push_str("\n\n父对话要点（子智能体读不到父对话，以下为全部背景）：\n");
                composed.push_str(notes);
            }
            let job = crate::history::create_collaboration_job(
                self.app.clone(),
                crate::history::CollaborationJobInput {
                    thread_id: self.thread_id.clone(),
                    parent_turn_id: Some(self.parent_turn_id.clone()),
                    goal: input
                        .get("goal")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| "单委托任务".to_string()),
                    goal_id,
                    workspace_path: self.workspace_path.clone(),
                    permission_mode: Some(self.permission_mode.clone()),
                    execution_target: self.execution_target.clone(),
                    agent_config: crate::history::CollaborationAgentConfig {
                        model_id: self.model_id.clone(),
                        analysis_mode: self.analysis_mode,
                        tool_policy: AgentToolPolicy::Auto,
                        skill_ids: self.skill_ids.clone(),
                        requested_tool: None,
                        reasoning_effort: self.reasoning_effort.clone(),
                        task_plan_mode: self.task_plan_mode.clone(),
                        task_plan_min_steps: self.task_plan_min_steps,
                        // G2-D：子智能体比本层深一层。
                        depth: self.depth.saturating_add(1),
                    },
                    agents: vec![crate::history::CollaborationAgentInput {
                        title: input
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("委托任务")
                            .to_string(),
                        instruction: composed,
                        workspace_path: self.workspace_path.clone(),
                        permission_mode: Some(self.permission_mode.clone()),
                    }],
                },
            )?;
            let job_id = job.job.id.clone();
            if !wait {
                return Ok(json!({
                    "job_id": job_id,
                    "mode": "delegate",
                    "status": "dispatched",
                    "hint": "子智能体已在协作侧栏启动；完成后结果在侧栏查看。",
                }));
            }
            let deadline = tokio::time::Instant::now() + COLLABORATION_WAIT_TIMEOUT;
            loop {
                let fan_in = crate::history::collaboration_fan_in_state(&self.app, &job_id)?
                    .ok_or_else(|| "协作作业创建后无法读取".to_string())?;
                if fan_in.terminal
                    || self.cancellation.load(Ordering::Relaxed)
                    || tokio::time::Instant::now() >= deadline
                {
                    // 结果回注 = 摘要（G2 约束 2）：截断到 4000 字符，全文在侧栏。
                    const RESULT_LIMIT: usize = 4000;
                    let result = fan_in
                        .agents
                        .first()
                        .and_then(|agent| agent.summary.clone())
                        .unwrap_or_default();
                    let (result, truncated) = if result.chars().count() > RESULT_LIMIT {
                        (result.chars().take(RESULT_LIMIT).collect::<String>(), true)
                    } else {
                        (result, false)
                    };
                    return Ok(json!({
                        "job_id": job_id,
                        "mode": "delegate",
                        "status": fan_in.status,
                        "terminal": fan_in.terminal,
                        "cancelled_wait": self.cancellation.load(Ordering::Relaxed),
                        "result": result,
                        "result_truncated": truncated,
                        "error": fan_in.agents.first().and_then(|agent| agent.error.clone()),
                    }));
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        let goal = input
            .get("goal")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "协作任务必须提供总目标".to_string())?;
        let raw_agents = input
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| "协作任务必须提供子智能体列表".to_string())?;
        if raw_agents.len() < 2 || raw_agents.len() > 8 {
            return Err("协作任务必须包含 2 到 8 个子智能体".to_string());
        }
        let agents = raw_agents
            .iter()
            .map(|agent| {
                let title = agent
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "每个子智能体必须提供职责名称".to_string())?;
                let instruction = agent
                    .get("instruction")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "每个子智能体必须提供独立任务说明".to_string())?;
                Ok(crate::history::CollaborationAgentInput {
                    title: title.to_string(),
                    instruction: instruction.to_string(),
                    // The parent turn determines the authority boundary. A
                    // model-generated worker cannot expand it with a path or
                    // a more permissive sandbox mode.
                    workspace_path: self.workspace_path.clone(),
                    permission_mode: Some(self.permission_mode.clone()),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let job = crate::history::create_collaboration_job(
            self.app.clone(),
            crate::history::CollaborationJobInput {
                thread_id: self.thread_id.clone(),
                parent_turn_id: Some(self.parent_turn_id.clone()),
                goal: goal.to_string(),
                goal_id: None,
                workspace_path: self.workspace_path.clone(),
                permission_mode: Some(self.permission_mode.clone()),
                execution_target: self.execution_target.clone(),
                agent_config: crate::history::CollaborationAgentConfig {
                    model_id: self.model_id.clone(),
                    analysis_mode: self.analysis_mode,
                    tool_policy: AgentToolPolicy::Auto,
                    skill_ids: self.skill_ids.clone(),
                    requested_tool: None,
                    reasoning_effort: self.reasoning_effort.clone(),
                    task_plan_mode: self.task_plan_mode.clone(),
                    task_plan_min_steps: self.task_plan_min_steps,
                    depth: self.depth.saturating_add(1),
                },
                agents,
            },
        )?;
        let job_id = job.job.id.clone();
        let deadline = tokio::time::Instant::now() + COLLABORATION_WAIT_TIMEOUT;
        loop {
            let fan_in = crate::history::collaboration_fan_in_state(&self.app, &job_id)?
                .ok_or_else(|| "协作作业创建后无法读取".to_string())?;
            if fan_in.terminal
                || self.cancellation.load(Ordering::Relaxed)
                || tokio::time::Instant::now() >= deadline
            {
                return Ok(json!({
                    "job_id": job_id,
                    "thread_id": job.job.thread_id,
                    "agent_count": job.agents.len(),
                    "status": fan_in.status,
                    "terminal": fan_in.terminal,
                    "cancelled_wait": self.cancellation.load(Ordering::Relaxed),
                    "fan_in": fan_in,
                }));
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

pub(super) struct ReadLogRangeTool {
    pub(super) paths: Vec<String>,
    pub(super) cancellation: Arc<AtomicBool>,
}

#[async_trait]
impl ToolHandler for ReadLogRangeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_local_log_range",
            "Read a bounded line range from a selected local log or archive member. For archive results, pass both source and member from search_local_logs.",
            json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "member": { "type": "string" },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "max_lines": { "type": "integer", "maximum": 512 },
                    "max_bytes": { "type": "integer", "maximum": 1048576 }
                },
                "required": ["start_line"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let request = LogRangeRequest {
            paths: self.paths.clone(),
            source: input
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_string),
            member: input
                .get("member")
                .and_then(Value::as_str)
                .map(str::to_string),
            start_line: input.get("start_line").and_then(Value::as_u64).unwrap_or(1) as usize,
            max_lines: input.get("max_lines").and_then(Value::as_u64).unwrap_or(80) as usize,
            max_bytes: input
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(48 * 1024) as usize,
        };
        let result = read_log_range_with_cancellation(
            &request.paths,
            request.source.as_deref(),
            request.member.as_deref(),
            request.start_line,
            request.max_lines,
            request.max_bytes,
            &self.cancellation,
        )?;
        serde_json::to_value(result).map_err(|error| error.to_string())
    }
}

pub(super) struct SearchLocalLogsTool {
    pub(super) paths: Vec<String>,
    pub(super) cancellation: Arc<AtomicBool>,
}

#[async_trait]
impl ToolHandler for SearchLocalLogsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "search_local_logs",
            "Search selected local logs and archive members with bounded excerpts. Results include source and member name for read_local_log_range.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max_results": { "type": "integer", "maximum": 128 }
                },
                "required": ["query"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        serde_json::to_value(search_log_paths_with_cancellation(
            &self.paths,
            query,
            24,
            512,
            &self.cancellation,
        )?)
        .map_err(|error| error.to_string())
    }
}

pub(super) struct RebuildTimelineTool {
    pub(super) paths: Vec<String>,
    pub(super) cancellation: Arc<AtomicBool>,
}
