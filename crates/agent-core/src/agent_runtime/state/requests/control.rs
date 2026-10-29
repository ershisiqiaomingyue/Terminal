use crate::{AgentRuntimeState, AgentSteerRequest};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 8;

/// G5-A 第一步：执行单元的种类。当前内核统一跑 turn，协作 worker 的 turn
/// 标记为 Subagent（诊断/预算归属用）；WorkflowNode 为 run_workflow 编排的
/// 节点预留——完整 ExecutionUnit trait 等第三种执行单元真正落地时再抽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Turn,
    Subagent,
    WorkflowNode,
}

impl UnitKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            UnitKind::Turn => "turn",
            UnitKind::Subagent => "subagent",
            UnitKind::WorkflowNode => "workflow_node",
        }
    }
}

pub struct AgentRequestControl {
    pub turn_id: String,
    /// G5-A：本控制块所属的执行单元种类（缺省 Turn；协作 admission 标记
    /// Subagent）。运行时可查——诊断、预算归属、将来的 ExecutionUnit 抽象
    /// 都以它为锚点。
    pub unit_kind: AtomicU8,
    /// G5-A/G2：本单元归属的 goal id（单委托 job 关联的 goal）。
    pub parent_goal: Mutex<Option<String>>,
    pub cancellation: Arc<AtomicBool>,
    pub cancellation_reason: Arc<Mutex<Option<String>>>,
    /// 取消令牌（P1-B）。与 `cancellation` 标志同生同灭：置位标志的代码
    /// 必须经由 [`AgentRequestControl::request_cancellation`] 同时 cancel
    /// 令牌，等待在任意 `await` 上的准备阶段才能被立刻打断，而不是等执行
    /// 流自己走到下一个 `is_cancelled` 检查点。
    pub cancellation_token: CancellationToken,
    /// 回合任务的硬中止句柄（P1-C 兜底）。仅在软取消宽限超时后由
    /// `cancel_agent_request` 触发；正常完成路径从不使用。
    pub task_abort_handle: Mutex<Option<tokio::task::AbortHandle>>,
    pub steer_sender: mpsc::Sender<AgentSteerRequest>,
    pub tool_call_count: usize,
    pub(super) started_tool_call_ids: HashSet<String>,
    pub(super) active_tool_call_ids: HashSet<String>,
    pub(super) consecutive_tool_failures: std::collections::HashMap<String, usize>,
    pub(super) last_progress_at: Instant,
}

pub(crate) trait RequestState {
    fn requests_slot(&self) -> &Mutex<std::collections::HashMap<String, AgentRequestControl>>;
    fn permissions_slot(
        &self,
    ) -> &Mutex<std::collections::HashMap<(String, String), super::PendingPermission>>;
    fn approvals_slot(&self) -> &Mutex<HashSet<(String, String)>>;
}

impl AgentRequestControl {
    /// G5-A：标记本控制块的执行单元种类与目标归属（协作 admission 用）。
    pub fn describe_unit(&self, kind: UnitKind, parent_goal: Option<String>) {
        self.unit_kind.store(
            match kind {
                UnitKind::Turn => 0,
                UnitKind::Subagent => 1,
                UnitKind::WorkflowNode => 2,
            },
            Ordering::Relaxed,
        );
        if let Ok(mut goal) = self.parent_goal.lock() {
            *goal = parent_goal;
        }
    }

    /// G5-A：读取执行单元种类（诊断/预算归属）。
    pub fn unit_kind(&self) -> UnitKind {
        match self.unit_kind.load(Ordering::Relaxed) {
            1 => UnitKind::Subagent,
            2 => UnitKind::WorkflowNode,
            _ => UnitKind::Turn,
        }
    }

    /// 一致地请求取消：标志、原因与令牌必须同时生效（P1-B / P2 回归保护）。
    /// 任何直接 `cancellation.store(true)` 的写法都会让令牌与标志失步，
    /// 准备阶段的 `select!` 将追不上取消信号。
    pub fn request_cancellation(&self, reason: &str) {
        self.cancellation.store(true, Ordering::Relaxed);
        if let Ok(mut cancellation_reason) = self.cancellation_reason.lock() {
            *cancellation_reason = Some(reason.to_string());
        }
        self.cancellation_token.cancel();
    }

    /// 登记回合任务的中止句柄，供 P1-C 兜底使用。
    pub fn set_task_abort_handle(&self, handle: tokio::task::AbortHandle) {
        if let Ok(mut slot) = self.task_abort_handle.lock() {
            *slot = Some(handle);
        }
    }

    /// P1-C 兜底：软取消宽限超时后硬中止回合任务。返回是否存在可中止的句柄。
    pub fn abort_task(&self) -> bool {
        match self.task_abort_handle.lock() {
            Ok(mut slot) => slot.take().map(|handle| handle.abort()).is_some(),
            Err(_) => false,
        }
    }
}

impl AgentRuntimeState {
    pub(crate) fn has_active_agent_request(&self, session_id: &str) -> bool {
        self.requests
            .lock()
            .map(|requests| requests.contains_key(session_id))
            .unwrap_or(true)
    }

    pub fn record_tool_call_started(
        &self,
        session_id: &str,
        turn_id: &str,
        call_id: &str,
    ) -> Option<usize> {
        let mut requests = self.requests.lock().ok()?;
        let control = requests.get_mut(session_id)?;
        if control.turn_id != turn_id {
            return None;
        }
        control.last_progress_at = Instant::now();
        if !control.started_tool_call_ids.insert(call_id.to_string()) {
            return None;
        }
        control.active_tool_call_ids.insert(call_id.to_string());
        control.tool_call_count += 1;
        Some(control.tool_call_count)
    }

    pub fn tool_call_count(&self, session_id: &str, turn_id: &str) -> usize {
        self.requests
            .lock()
            .ok()
            .and_then(|requests| {
                requests
                    .get(session_id)
                    .filter(|control| control.turn_id == turn_id)
                    .map(|control| control.tool_call_count)
            })
            .unwrap_or_default()
    }

    pub fn record_turn_progress(&self, session_id: &str, turn_id: &str) {
        if let Ok(mut requests) = self.requests.lock() {
            if let Some(control) = requests
                .get_mut(session_id)
                .filter(|control| control.turn_id == turn_id)
            {
                control.last_progress_at = Instant::now();
            }
        }
    }

    pub fn record_tool_call_finished(
        &self,
        session_id: &str,
        turn_id: &str,
        call_id: &str,
        tool_name: &str,
        failed: bool,
    ) -> Option<String> {
        let mut requests = self.requests.lock().ok()?;
        let control = requests.get_mut(session_id)?;
        if control.turn_id != turn_id {
            return None;
        }
        control.last_progress_at = Instant::now();
        control.active_tool_call_ids.remove(call_id);
        if !failed {
            control.consecutive_tool_failures.remove(tool_name);
            return None;
        }
        let failures = control
            .consecutive_tool_failures
            .entry(tool_name.to_string())
            .or_default();
        *failures += 1;
        if *failures < MAX_CONSECUTIVE_TOOL_FAILURES || control.cancellation.load(Ordering::Relaxed)
        {
            return None;
        }
        let reason = format!("工具 {tool_name} 连续失败 {failures} 次");
        control.request_cancellation(&reason);
        Some(reason)
    }

    pub fn cancel_if_turn_idle(
        &self,
        session_id: &str,
        turn_id: &str,
        cancellation: &Arc<AtomicBool>,
        max_idle: Duration,
    ) -> Option<String> {
        let mut requests = self.requests.lock().ok()?;
        let control = requests.get_mut(session_id)?;
        if control.turn_id != turn_id
            || !Arc::ptr_eq(&control.cancellation, cancellation)
            || control.cancellation.load(Ordering::Relaxed)
            || !control.active_tool_call_ids.is_empty()
            || control.last_progress_at.elapsed() < max_idle
        {
            return None;
        }
        let reason = format!(
            "[turn_idle_timeout] 连续 {} 秒未收到模型或工具进度",
            max_idle.as_secs()
        );
        control.request_cancellation(&reason);
        Some(reason)
    }
}

pub fn cancel_request_token_if_matching(
    state: &AgentRuntimeState,
    session_id: &str,
    cancellation: &Arc<AtomicBool>,
) {
    if let Ok(requests) = state.requests.lock() {
        if let Some(control) = requests.get(session_id) {
            if Arc::ptr_eq(&control.cancellation, cancellation) {
                control.cancellation_token.cancel();
            }
        }
    }
}
