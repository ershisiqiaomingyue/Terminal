//! G6 验收交付：顺序 workflow 编排器——证明「在既有内核上编排多节点执行
//! 不需要改内核」。节点 = 一次单委托（复用 collaboration 的 durable 调度/
//! fan-in 等待/取消传播/终态落库，即 G2 的 delegate 模式），上一节点的结果
//! 摘要作为下一节点的 context_notes 输入。本文件只做编排，不含任何执行
//! 逻辑——这正是 ExecutionUnit 抽象（G5）想要保障的形态。
use super::*;

/// G6：顺序 workflow——每步一个自包含指令，结果逐节点传递。
/// 步数上限 8（与协作组一致），避免误用成无限链。
#[derive(Clone)]
pub(super) struct WorkflowTool {
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
}

impl WorkflowTool {
    pub(super) fn with_cancellation(&self, cancellation: Arc<AtomicBool>) -> Self {
        let mut tool = self.clone();
        tool.cancellation = cancellation;
        tool
    }
}

/// 与协作工具同构派生——注册链复用 `register_collaboration_tool`，
/// turn.rs 构造路径零改动（G6：编排层不改内核的体现之一）。
impl From<&CreateCollaborationJobTool> for WorkflowTool {
    fn from(tool: &CreateCollaborationJobTool) -> Self {
        Self {
            app: tool.app.clone(),
            thread_id: tool.thread_id.clone(),
            parent_turn_id: tool.parent_turn_id.clone(),
            model_id: tool.model_id.clone(),
            workspace_path: tool.workspace_path.clone(),
            permission_mode: tool.permission_mode.clone(),
            execution_target: tool.execution_target.clone(),
            analysis_mode: tool.analysis_mode,
            skill_ids: tool.skill_ids.clone(),
            reasoning_effort: tool.reasoning_effort.clone(),
            task_plan_mode: tool.task_plan_mode.clone(),
            task_plan_min_steps: tool.task_plan_min_steps,
            cancellation: tool.cancellation.clone(),
        }
    }
}

#[async_trait]
impl ToolHandler for WorkflowTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "run_workflow",
            "Run a sequential workflow of self-contained steps. Each step runs as its own sub-agent turn; the previous step's result summary is passed to the next step as background. Use this when the user asks for a fixed pipeline of dependent stages (analyze, then transform, then verify). Do not use it for a normal multi-step task in one conversation. Each instruction must be self-contained — the sub-agent cannot read this conversation. The workspace and permission boundary are inherited and cannot be expanded.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "The workflow's overall goal; each step sees it." },
                    "steps": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Short stage label." },
                                "instruction": { "type": "string", "description": "Self-contained task for this stage." }
                            },
                            "required": ["instruction"]
                        }
                    }
                },
                "required": ["goal", "steps"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let goal = input
            .get("goal")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("workflow 必须提供总目标")?;
        let steps = input
            .get("steps")
            .and_then(Value::as_array)
            .ok_or("workflow 必须提供步骤列表")?;
        if steps.len() < 2 {
            return Err("workflow 至少包含 2 个步骤".into());
        }
        let mut node_results: Vec<Value> = Vec::with_capacity(steps.len());
        for (index, step) in steps.iter().enumerate() {
            if self.cancellation.load(Ordering::Relaxed) {
                return Ok(json!({
                    "status": "cancelled",
                    "completed_steps": node_results.len(),
                    "steps": node_results,
                    "hint": "workflow 已取消；已完成步骤的结果保留。",
                }));
            }
            let instruction = step
                .get("instruction")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or("每个步骤必须提供非空指令")?;
            let title = step
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("阶段");
            // 编排核心：上一节点结果作为下一节点的背景（摘要传递，不是全文）。
            let context_notes = if node_results.is_empty() {
                format!("工作流总目标：{goal}")
            } else {
                format!(
                    "工作流总目标：{goal}\n前序阶段「{}」的结果：{}",
                    node_results
                        .last()
                        .and_then(|result| result.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("上一阶段"),
                    node_results
                        .last()
                        .and_then(|result| result.get("result"))
                        .and_then(Value::as_str)
                        .unwrap_or("（无输出）"),
                )
            };
            let job = crate::history::create_collaboration_job(
                self.app.clone(),
                crate::history::CollaborationJobInput {
                    thread_id: self.thread_id.clone(),
                    parent_turn_id: Some(format!("{}:workflow:{}", self.parent_turn_id, index)),
                    goal: format!("{goal} · {title}"),
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
                        depth: 1,
                    },
                    agents: vec![crate::history::CollaborationAgentInput {
                        title: title.to_string(),
                        instruction: format!("{context_notes}\n\n本阶段任务：{instruction}"),
                        workspace_path: self.workspace_path.clone(),
                        permission_mode: Some(self.permission_mode.clone()),
                    }],
                },
            )?;
            // 复用 delegate 的等待语义：fan-in 轮询 + 取消传播 + 超时上限。
            let job_id = job.job.id.clone();
            let deadline = tokio::time::Instant::now() + COLLABORATION_WAIT_TIMEOUT;
            let (status, result, error) = loop {
                let fan_in = crate::history::collaboration_fan_in_state(&self.app, &job_id)?
                    .ok_or("workflow 作业创建后无法读取")?;
                if fan_in.terminal
                    || self.cancellation.load(Ordering::Relaxed)
                    || tokio::time::Instant::now() >= deadline
                {
                    break (
                        fan_in.status,
                        fan_in
                            .agents
                            .first()
                            .and_then(|agent| agent.summary.clone())
                            .unwrap_or_default(),
                        fan_in.agents.first().and_then(|agent| agent.error.clone()),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            };
            node_results.push(json!({
                "step": index + 1,
                "title": title,
                "job_id": job_id,
                "status": status,
                // 结果截断：长结果留在协作侧栏，编排只传摘要（与 delegate 同款约束）。
                "result": result.chars().take(2_000).collect::<String>(),
                "error": error,
            }));
        }
        let failed = node_results
            .iter()
            .any(|node| node.get("error").is_some_and(|error| !error.is_null()));
        Ok(json!({
            "status": if failed { "partial" } else { "completed" },
            "steps": node_results,
            "hint": "每个步骤的完整过程与输出在协作侧栏；此处为各阶段摘要。",
        }))
    }
}
