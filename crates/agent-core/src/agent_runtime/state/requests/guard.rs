use super::control::AgentRequestControl;
use super::RequestState;
use crate::{AgentRuntimeState, AgentSteerRequest};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct AgentRequestGuard<'a> {
    state: Cow<'a, AgentRuntimeState>,
    session_id: String,
    cancellation: Arc<AtomicBool>,
    cancellation_reason: Arc<Mutex<Option<String>>>,
    cancellation_token: CancellationToken,
    steer_receiver: Option<mpsc::Receiver<AgentSteerRequest>>,
}

impl<'a> AgentRequestGuard<'a> {
    pub fn start(
        state: &'a AgentRuntimeState,
        session_id: String,
        turn_id: String,
    ) -> Result<Self, String> {
        Self::start_with_state(Cow::Borrowed(state), session_id, turn_id)
    }

    fn start_with_state(
        state: Cow<'a, AgentRuntimeState>,
        session_id: String,
        turn_id: String,
    ) -> Result<Self, String> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancellation_reason = Arc::new(Mutex::new(None));
        // P1-B：令牌与标志在同一处创建，控制表与守卫各持一份克隆，
        // 取消时二者必须一起生效（见 `AgentRequestControl::request_cancellation`）。
        let cancellation_token = CancellationToken::new();
        let (steer_sender, steer_receiver) = mpsc::channel(16);
        let mut requests = state
            .requests_slot()
            .lock()
            .map_err(|_| "请求状态不可用".to_string())?;
        if requests.contains_key(&session_id) {
            return Err("当前会话已有 Agent 正在执行".to_string());
        }
        requests.insert(
            session_id.clone(),
            AgentRequestControl {
                turn_id,
                unit_kind: std::sync::atomic::AtomicU8::new(0),
                parent_goal: Mutex::new(None),
                cancellation: cancellation.clone(),
                cancellation_reason: cancellation_reason.clone(),
                cancellation_token: cancellation_token.clone(),
                task_abort_handle: Mutex::new(None),
                steer_sender,
                tool_call_count: 0,
                started_tool_call_ids: HashSet::new(),
                active_tool_call_ids: HashSet::new(),
                consecutive_tool_failures: HashMap::new(),
                last_progress_at: Instant::now(),
            },
        );
        drop(requests);
        Ok(Self {
            state,
            session_id,
            cancellation,
            cancellation_reason,
            cancellation_token,
            steer_receiver: Some(steer_receiver),
        })
    }

    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancellation.clone()
    }

    pub fn cancellation_reason(&self) -> Arc<Mutex<Option<String>>> {
        self.cancellation_reason.clone()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    pub fn take_steer_receiver(&mut self) -> mpsc::Receiver<AgentSteerRequest> {
        self.steer_receiver
            .take()
            .expect("steer receiver can only be taken once")
    }
}

impl AgentRequestGuard<'static> {
    /// Retain native turn ownership independently of a frontend invocation.
    pub fn start_owned(
        state: &AgentRuntimeState,
        session_id: String,
        turn_id: String,
    ) -> Result<Self, String> {
        Self::start_with_state(Cow::Owned(state.clone()), session_id, turn_id)
    }
}

impl Drop for AgentRequestGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.state.requests_slot().lock() {
            let owns_request = requests
                .get(&self.session_id)
                .map(|control| Arc::ptr_eq(&control.cancellation, &self.cancellation))
                .unwrap_or(false);
            if owns_request {
                // Hold the session reservation until cleanup completes. A stale
                // guard must not clear a newer turn's single-use approvals.
                if let Ok(mut approvals) = self.state.approvals_slot().lock() {
                    approvals.retain(|(session_id, _)| session_id != &self.session_id);
                }
                requests.remove(&self.session_id);
            }
        }
        if let Ok(mut permissions) = self.state.permissions_slot().lock() {
            permissions.retain(|(session_id, _), pending| {
                session_id != &self.session_id
                    || !Arc::ptr_eq(&pending.cancellation, &self.cancellation)
            });
        }
    }
}
