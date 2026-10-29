use super::RequestState;
use crate::AgentRuntimeState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAction {
    Once,
    Session,
    Deny,
    Other,
}

impl PermissionAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "once" => Ok(Self::Once),
            "session" => Ok(Self::Session),
            "deny" => Ok(Self::Deny),
            "other" => Ok(Self::Other),
            _ => Err("权限决定无效，请选择 once、session、deny 或 other".to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Session => "session",
            Self::Deny => "deny",
            Self::Other => "other",
        }
    }
}

#[derive(Debug)]
pub struct PermissionResponse {
    pub action: PermissionAction,
    pub instruction: Option<String>,
    /// N1-A：true 表示这是超时合成的 Deny——没有人真正点过拒绝。调用方据此
    /// 补发 `permission_resolved` 事件；否则 pending 条目已被移除但事件缺失，
    /// 前端权限条永远等不到回应（N1 的 UI 卡死形态）。
    pub timed_out: bool,
}

pub enum PermissionWaitResult {
    Decision(PermissionResponse),
}

pub struct PendingPermission {
    pub sender: mpsc::Sender<PermissionResponse>,
    pub response_sent: bool,
    pub approval_key: String,
    pub call_id: String,
    pub cancellation: Arc<AtomicBool>,
    pub turn_id: String,
    pub workspace_root: Option<String>,
}

pub fn is_cancelled(
    state: &AgentRuntimeState,
    session_id: &str,
    cancellation: &Arc<AtomicBool>,
) -> bool {
    if cancellation.load(Ordering::Relaxed) {
        return true;
    }
    state
        .requests
        .lock()
        .map(|values| {
            values
                .get(session_id)
                .map(|control| !Arc::ptr_eq(&control.cancellation, cancellation))
                .unwrap_or(true)
        })
        .unwrap_or(true)
}

/// [`register_pending_permission`] and [`is_cancelled`] compare token *identity*,
/// so a prompt registered with a tool token always looks like it belongs to a
/// cancelled turn and fails immediately.
pub fn request_cancellation(
    state: &AgentRuntimeState,
    session_id: &str,
    turn_id: &str,
) -> Result<Arc<AtomicBool>, String> {
    state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .get(session_id)
        .filter(|control| control.turn_id == turn_id)
        .map(|control| control.cancellation.clone())
        .ok_or_else(|| "请求已取消".to_string())
}

pub fn register_pending_permission(
    state: &AgentRuntimeState,
    session_id: &str,
    request_id: String,
    approval_key: String,
    call_id: String,
    cancellation: Arc<AtomicBool>,
    turn_id: String,
    workspace_root: Option<String>,
) -> Result<mpsc::Receiver<PermissionResponse>, String> {
    let request_is_active = state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .get(session_id)
        .map(|control| {
            control.turn_id == turn_id
                && Arc::ptr_eq(&control.cancellation, &cancellation)
                && !control.cancellation.load(Ordering::Relaxed)
        })
        .unwrap_or(false);
    if !request_is_active {
        return Err("请求已取消".to_string());
    }
    let (sender, receiver) = mpsc::channel(1);
    state
        .permissions
        .lock()
        .map_err(|_| "权限状态不可用".to_string())?
        .insert(
            (session_id.to_string(), request_id.clone()),
            PendingPermission {
                sender,
                response_sent: false,
                approval_key,
                call_id,
                cancellation: cancellation.clone(),
                turn_id: turn_id.clone(),
                workspace_root,
            },
        );
    // Cancellation can race with the second lock acquisition above. Recheck
    // ownership after insertion so a cancelled turn cannot leave an orphaned
    // permission prompt behind.
    let still_active = state
        .requests
        .lock()
        .map(|values| {
            values
                .get(session_id)
                .map(|control| {
                    control.turn_id == turn_id
                        && Arc::ptr_eq(&control.cancellation, &cancellation)
                        && !control.cancellation.load(Ordering::Relaxed)
                })
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if !still_active {
        state
            .permissions
            .lock()
            .ok()
            .and_then(|mut values| values.remove(&(session_id.to_string(), request_id)));
        return Err("请求已取消".to_string());
    }
    Ok(receiver)
}

pub async fn wait_for_agent_cancellation(
    state: &AgentRuntimeState,
    session_id: &str,
    cancellation: &Arc<AtomicBool>,
) {
    while !is_cancelled(state, session_id, cancellation) {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

pub async fn wait_for_permission(
    state: &AgentRuntimeState,
    session_id: &str,
    request_id: String,
    cancellation: Arc<AtomicBool>,
    mut receiver: mpsc::Receiver<PermissionResponse>,
    timeout: Option<std::time::Duration>,
) -> Result<PermissionWaitResult, String> {
    let deadline = timeout.map(|value| tokio::time::Instant::now() + value);
    loop {
        if is_cancelled(state, session_id, &cancellation) {
            state.permissions_slot().lock().ok().and_then(|mut values| {
                values.remove(&(session_id.to_string(), request_id.clone()))
            });
            return Err("请求已取消".to_string());
        }
        if deadline.is_some_and(|value| tokio::time::Instant::now() >= value) {
            state.permissions_slot().lock().ok().and_then(|mut values| {
                values.remove(&(session_id.to_string(), request_id.clone()))
            });
            return Ok(PermissionWaitResult::Decision(PermissionResponse {
                action: PermissionAction::Deny,
                instruction: None,
                timed_out: true,
            }));
        }
        tokio::select! {
            biased;
            result = receiver.recv() => {
                if is_cancelled(state, session_id, &cancellation) {
                    state.permissions_slot().lock().ok().and_then(|mut values| values.remove(&(session_id.to_string(), request_id.clone())));
                    return Err("请求已取消".to_string());
                }
                let result = result.map(PermissionWaitResult::Decision).ok_or_else(|| "权限请求已关闭".to_string());
                state.permissions_slot().lock().ok().and_then(|mut values| values.remove(&(session_id.to_string(), request_id.clone())));
                return result;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }
    }
}
