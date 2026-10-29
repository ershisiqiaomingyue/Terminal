use super::*;

pub(crate) fn agent_queue_lease_owner() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| next_history_id("queue-owner"))
}

pub(crate) fn collaboration_lease_owner() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| next_history_id("collaboration-owner"))
}

pub(crate) fn enqueue_agent_queue_item(
    app: &AppHandle,
    session_id: &str,
    queue_id: &str,
    idempotency_key: &str,
    request: &AgentChatRequest,
) -> Result<AgentQueueEnqueueResult, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.enqueue_agent_queue_item(session_id, queue_id, idempotency_key, request)
}

pub(crate) fn list_agent_queue_items(
    app: &AppHandle,
    session_id: Option<&str>,
) -> Result<Vec<AgentQueueItem>, String> {
    open_store(app)?.list_agent_queue_items(session_id)
}

pub(crate) fn update_agent_queue_content(
    app: AppHandle,
    session_id: String,
    queue_id: String,
    content: String,
) -> Result<bool, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.update_agent_queue_content(&session_id, &queue_id, &content)
}

pub(crate) fn reorder_agent_queue_items(
    app: &AppHandle,
    session_id: &str,
    queue_ids: &[String],
) -> Result<bool, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.reorder_agent_queue_items(session_id, queue_ids)
}

pub(crate) fn claim_next_agent_queue_item(
    app: &AppHandle,
    session_id: &str,
    lease_owner: &str,
) -> Result<Option<AgentQueueLease>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.claim_agent_queue_item(session_id, None, lease_owner)
}

pub(crate) fn claim_agent_queue_item_by_id(
    app: &AppHandle,
    session_id: &str,
    queue_id: &str,
    lease_owner: &str,
) -> Result<Option<AgentQueueLease>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.claim_agent_queue_item(session_id, Some(queue_id), lease_owner)
}

pub(crate) fn ack_agent_queue_item(
    app: &AppHandle,
    session_id: &str,
    queue_id: &str,
    lease_id: &str,
    lease_owner: &str,
    disposition: &str,
) -> Result<AgentQueueItem, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.ack_agent_queue_item(session_id, queue_id, lease_id, lease_owner, disposition)
}

pub(crate) fn nack_agent_queue_item(
    app: &AppHandle,
    session_id: &str,
    queue_id: &str,
    lease_id: &str,
    lease_owner: &str,
    error: Option<&str>,
) -> Result<AgentQueueItem, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.nack_agent_queue_item(session_id, queue_id, lease_id, lease_owner, error)
}

pub(crate) fn recover_agent_queue_items(
    app: &AppHandle,
    lease_owner: &str,
) -> Result<Vec<AgentQueueItem>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.recover_agent_queue_items(lease_owner)
}

pub(crate) fn recover_legacy_queue_items(
    app: &AppHandle,
    lease_owner: &str,
) -> Result<Vec<(String, Result<AgentQueueItem, String>)>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.recover_legacy_queue_items(lease_owner)
}

pub(crate) fn enqueue_agent_steer(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    request: &AgentSteerRequest,
) -> Result<PersistedAgentSteer, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.enqueue_agent_steer(session_id, turn_id, request)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_queue_steer_and_ack(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    request: &AgentSteerRequest,
    queue_id: &str,
    lease_id: &str,
    lease_owner: &str,
) -> Result<PersistedAgentSteer, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.persist_queue_steer_and_ack(
        session_id,
        turn_id,
        request,
        queue_id,
        lease_id,
        lease_owner,
    )
}

pub(crate) fn pending_agent_steers(
    app: &AppHandle,
    session_id: &str,
) -> Result<Vec<PersistedAgentSteer>, String> {
    open_store(app)?.pending_agent_steers(session_id)
}

pub(crate) fn update_agent_steer_status(
    app: &AppHandle,
    steer_id: &str,
    status: &str,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.update_agent_steer_status(steer_id, status)
}

pub(crate) fn reject_agent_steer_if_terminal(
    app: &AppHandle,
    steer_id: &str,
) -> Result<bool, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.reject_agent_steer_if_terminal(steer_id)
}

/// N6-B：turn 终态落库成功后统一回收该轮所有未应用（pending/delivered）的
/// 追加要求，返回被回收的 steer id 供 UI 标注「未送达」。
pub(crate) fn reject_agent_steers_for_turn(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
) -> Result<Vec<String>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.reject_agent_steers_for_turn(session_id, turn_id)
}

/// N6-B（缺口补齐）：终态落库失败时只读查询该轮仍悬挂的追加要求 id，
/// 供调用方在库写不进的情况下仍然逐条广播 `steer_rejected`。
pub(crate) fn pending_agent_steer_ids_for_turn(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
) -> Result<Vec<String>, String> {
    open_store(app)?.pending_agent_steer_ids_for_turn(session_id, turn_id)
}

pub(crate) fn mark_next_agent_steer_applied(
    app: &AppHandle,
    session_id: &str,
    request: &AgentSteerRequest,
) -> Result<Option<String>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.mark_next_agent_steer_applied(session_id, request)
}

pub(crate) fn start_agent_run(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    request: &AgentChatRequest,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.start_agent_run(session_id, turn_id, request)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_agent_run_from_queue(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    request: &AgentChatRequest,
    queue_id: &str,
    lease_id: &str,
    lease_owner: &str,
) -> Result<AgentQueueItem, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(app)?.start_agent_run_from_queue(
        session_id,
        turn_id,
        request,
        queue_id,
        lease_id,
        lease_owner,
    )
}

/// Builds provider context from the durable conversation record. Requests from
/// the native queue deliberately leave `context` empty so they are evaluated
/// after the preceding turn has persisted its terminal message.
pub(crate) fn agent_context_for_session(
    app: &AppHandle,
    session_id: &str,
) -> Result<Vec<AgentContextTurn>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(record) = open_store(app)?.get_record(session_id)? else {
        return Ok(Vec::new());
    };
    Ok(raw_agent_context_from_record(&record))
}

#[derive(Debug)]
pub(super) struct AgentContextCheckpointCandidate {
    pub(super) checkpoint: AgentContextCheckpoint,
}
