//! G5-A：ExecutionUnit——执行单元的生命周期契约与驱动器。
//!
//! codex `SessionTask`/`handle_task_abort` 的对应物：把「spawn → 登记 P1-C
//! 硬中止句柄 → await → JoinError 映射 → on_abort 清理钩子」这段生命周期
//! 机器从 queue.rs 的内联代码抽成可复用、可测试的形态。内核 `run_agent_turn`
//! 保持唯一（G5-B 结论），trait 只约束**生命周期**，不重复执行逻辑。
//!
//! 与总纲 G5-A 草图的差异：trait 放在 `src-tauri/src/agent/` 而非 agent-core
//! ——`run_agent_turn` 依赖 AppHandle/Tauri 状态，内核类型决定了契约层级；
//! agent-core 保留 `UnitKind`/`parent_goal`（G5-A 第一步的锚点）。

use super::*;
use agent_core::AgentChatResponse;

/// 执行单元生命周期契约（G5-A）。实现方持有自己的上下文（guard、请求等），
/// 驱动器 [`spawn_and_drive_unit`] 只依赖本 trait（对象安全，async_trait 装箱）。
#[async_trait]
pub(crate) trait ExecutionUnit: Send + Sync + 'static {
    fn session_id(&self) -> &str;
    fn turn_id(&self) -> &str;
    /// 与 AgentRequestControl 注册的取消标志同源（硬中止登记的身份匹配用）。
    fn cancellation(&self) -> Arc<AtomicBool>;

    /// 单元主体。实现必须是可被 tokio 硬中止的任务（驱动器负责登记句柄）。
    async fn run(self: Arc<Self>) -> Result<AgentChatResponse, String>;

    /// P1-C 清理钩子：任务被硬中止（JoinError::cancelled）后调用。
    /// 软取消路径与正常完成路径不会走到这里。默认空——TurnUnit 无需额外
    /// 清理（guard Drop + 终态分支已覆盖）；将来的 WorkflowNode 在此级联
    /// 取消子 job。
    fn on_abort(&self) {}
}

/// 生命周期驱动器：spawn 单元、登记硬中止句柄、await，并把 JoinError 归一为
/// 三个结果（正常值 / 「请求已取消」 / 异常终止文案）。驱动完成后调用
/// `end_tool_group` 收工具组时间线。终态落库/事件/预算回写仍由调用方
/// （queue.rs 的 admission 分支）负责——那是 admission 层的差异面。
pub(crate) async fn spawn_and_drive_unit(
    state: &AgentRuntimeState,
    unit: Arc<dyn ExecutionUnit>,
) -> Result<AgentChatResponse, String> {
    let session_id = unit.session_id().to_string();
    let turn_id = unit.turn_id().to_string();
    let cancellation = unit.cancellation();
    let driver_unit = unit.clone();
    let task = tokio::spawn(async move { driver_unit.run().await });
    register_turn_task_for_hard_abort(
        state,
        &session_id,
        &turn_id,
        &cancellation,
        task.abort_handle(),
    );
    let result = match task.await {
        Ok(result) => result,
        Err(join_error) if join_error.is_cancelled() => {
            // P1-C 清理钩子：硬中止后给实现方一次资源收尾的机会。
            unit.on_abort();
            Err("请求已取消".to_string())
        }
        Err(join_error) => Err(format!("Agent 任务异常终止：{join_error}")),
    };
    end_tool_group(&session_id, &turn_id);
    result
}

/// G5-A 第一个实现：普通对话回合（协作 worker 的 turn 也由同一路径执行，
/// admission 时经 `describe_unit` 标记 Subagent 与 goal——执行内核唯一）。
pub(crate) struct TurnUnit {
    pub app: AppHandle,
    pub request: AgentChatRequest,
    pub session_id: String,
    pub turn_id: String,
    /// 'static guard（start_owned）的 take-once 移交槽：run 恰被驱动器调用
    /// 一次；run 结束时 guard Drop 释放会话预约（既有语义不变）。
    pub guard: std::sync::Mutex<Option<agent_core::AgentRequestGuard<'static>>>,
    pub cancellation: Arc<AtomicBool>,
}

impl TurnUnit {
    pub(crate) fn new(
        app: AppHandle,
        request: AgentChatRequest,
        session_id: String,
        turn_id: String,
        guard: agent_core::AgentRequestGuard<'static>,
        cancellation: Arc<AtomicBool>,
    ) -> Self {
        Self {
            app,
            request,
            session_id,
            turn_id,
            guard: std::sync::Mutex::new(Some(guard)),
            cancellation,
        }
    }
}

#[async_trait]
impl ExecutionUnit for TurnUnit {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn turn_id(&self) -> &str {
        &self.turn_id
    }

    fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancellation.clone()
    }

    async fn run(self: Arc<Self>) -> Result<AgentChatResponse, String> {
        let guard = self
            .guard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .expect("ExecutionUnit::run 被驱动器调用多次（take-once 契约破坏）");
        let state = self.app.state::<AgentRuntimeState>();
        let mcp_state = self.app.state::<McpRuntimeState>();
        run_agent_turn(
            self.app.clone(),
            state,
            mcp_state,
            self.request.clone(),
            self.session_id.clone(),
            self.turn_id.clone(),
            guard,
        )
        .await
    }
}

#[cfg(test)]
mod execution_tests {
    use super::*;

    fn response() -> AgentChatResponse {
        AgentChatResponse {
            session_id: "s".into(),
            text: "ok".into(),
            usage: None,
            report: None,
            report_error: None,
            evidence: Default::default(),
            context_checkpoint: None,
            turn_id: "t".into(),
            termination_reason: "model_finished".into(),
        }
    }

    /// 可编程桩单元：完成前等待 `hold_ms`，可注入 panic 模拟卡死。
    struct StubUnit {
        hold_ms: u64,
        panic_in_run: bool,
        aborts: std::sync::Arc<std::sync::Mutex<usize>>,
        token: Arc<AtomicBool>,
    }

    impl StubUnit {
        fn new(hold_ms: u64, panic_in_run: bool, token: Arc<AtomicBool>) -> Self {
            Self {
                hold_ms,
                panic_in_run,
                aborts: std::sync::Arc::new(std::sync::Mutex::new(0)),
                token,
            }
        }
    }

    #[async_trait]
    impl ExecutionUnit for StubUnit {
        fn session_id(&self) -> &str {
            "s"
        }
        fn turn_id(&self) -> &str {
            "t"
        }
        fn cancellation(&self) -> Arc<AtomicBool> {
            self.token.clone()
        }
        async fn run(self: Arc<Self>) -> Result<AgentChatResponse, String> {
            if self.panic_in_run {
                panic!("模拟卡死的等待（不响应取消）");
            }
            tokio::time::sleep(std::time::Duration::from_millis(self.hold_ms)).await;
            Ok(response())
        }
        fn on_abort(&self) {
            if let Ok(mut count) = self.aborts.lock() {
                *count += 1;
            }
        }
    }

    fn guard_pair(
        state: &AgentRuntimeState,
    ) -> (agent_core::AgentRequestGuard<'static>, Arc<AtomicBool>) {
        let guard =
            agent_core::AgentRequestGuard::start_owned(state, "s".into(), "t".into()).unwrap();
        let cancellation = guard.cancellation();
        (guard, cancellation)
    }

    /// 正常路径：run 的结果原样穿透，工具组收尾被调用（幂等，无副作用断言）。
    #[tokio::test]
    async fn driver_passes_through_normal_completion() {
        let state = AgentRuntimeState::default();
        let (_guard, token) = guard_pair(&state);
        let unit = Arc::new(StubUnit::new(10, false, token));
        let result = spawn_and_drive_unit(&state, unit).await.unwrap();
        assert_eq!(result.text, "ok");
        assert_eq!(result.termination_reason, "model_finished");
    }

    /// P1-C 端到端：控制表登记 → cancel 置位 + 硬中止 → JoinError 映射为
    /// 「请求已取消」→ on_abort 钩子被调用一次。
    #[tokio::test]
    async fn driver_maps_hard_abort_and_invokes_on_abort_hook() {
        let state = AgentRuntimeState::default();
        let (guard, token) = guard_pair(&state);
        let stub = Arc::new(StubUnit::new(60_000, false, token.clone()));
        let aborts = stub.aborts.clone();
        // spawn 需要 'static future：驱动器拿 state 的克隆（Arc 字段共享）。
        let state_for_task = state.clone();
        let driven = tokio::spawn(async move {
            spawn_and_drive_unit(&state_for_task, stub as Arc<dyn ExecutionUnit>).await
        });
        // 模拟 cancel_agent_request 的兜底路径：软取消置位后硬中止。
        // spawn 不保证任务已起跑——轮询等登记完成（最多 2s）。
        token.store(true, Ordering::Relaxed);
        let mut registered = false;
        for _ in 0..200 {
            let ok = state
                .requests
                .lock()
                .ok()
                .and_then(|requests| requests.get("s").map(|control| control.abort_task()))
                .unwrap_or(false);
            if ok {
                registered = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(registered, "硬中止句柄应已登记");
        let result = driven.await.unwrap();
        assert_eq!(result.unwrap_err(), "请求已取消");
        assert_eq!(*aborts.lock().unwrap(), 1, "on_abort 恰好一次");
        drop(guard);
    }

    /// panic 传播：run 内 panic 映射为「Agent 任务异常终止」而不是毒化调用方。
    #[tokio::test]
    async fn driver_maps_run_panic_to_termination_error() {
        let state = AgentRuntimeState::default();
        let (_guard, token) = guard_pair(&state);
        let unit = Arc::new(StubUnit::new(0, true, token));
        let message = spawn_and_drive_unit(&state, unit).await.unwrap_err();
        assert!(message.contains("Agent 任务异常终止"), "实际: {message}");
    }
}
