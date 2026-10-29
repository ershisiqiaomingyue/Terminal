use super::super::*;

pub(crate) fn emit_start_failure_snapshot(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    message: &str,
    error: &str,
    baseline_revision: Option<u64>,
) {
    let state = app.state::<AgentRuntimeState>();
    if !apply_unadmitted_start_failure_snapshot(
        &state,
        session_id,
        turn_id,
        message,
        error,
        baseline_revision,
    ) {
        return;
    }
    if let Err(history_error) = crate::history::finish_agent_turn(
        app,
        session_id,
        turn_id,
        "failed",
        Some(error),
        Some(error),
    ) {
        eprintln!("无法持久化 Agent 启动失败: {history_error}");
    }
    if let Some(update) = state.take_thread_update(session_id) {
        let _ = app.emit("agent:thread_update", update);
    }
}

pub(crate) fn can_apply_unadmitted_start_failure(
    active_request_turn_id: Option<&str>,
    snapshot: Option<&agent_core::ThreadSnapshot>,
    turn_id: &str,
    baseline_revision: Option<u64>,
) -> bool {
    if active_request_turn_id.is_some()
        || snapshot.map(|snapshot| snapshot.revision) != baseline_revision
    {
        return false;
    }
    match snapshot.and_then(|snapshot| snapshot.active_turn.as_ref()) {
        Some(turn) if turn.id == turn_id => false,
        Some(turn) if turn.status == TurnStatus::Running => false,
        _ => true,
    }
}

pub(crate) fn apply_unadmitted_start_failure_snapshot(
    state: &AgentRuntimeState,
    session_id: &str,
    turn_id: &str,
    message: &str,
    error: &str,
    baseline_revision: Option<u64>,
) -> bool {
    // Keep request admission and the fallback snapshot transition in one
    // critical section. Otherwise a new turn can acquire the session between
    // the idle check and TurnStarted, then have its live snapshot cleared.
    let requests = match state.requests.lock() {
        Ok(requests) => requests,
        Err(_) => return false,
    };
    let snapshot = state.read_thread_snapshot(session_id);
    if !can_apply_unadmitted_start_failure(
        requests
            .get(session_id)
            .map(|control| control.turn_id.as_str()),
        snapshot.as_ref(),
        turn_id,
        baseline_revision,
    ) {
        return false;
    }

    let applied = [
        ThreadEvent::TurnStarted {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
        },
        ThreadEvent::ItemAdded {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            item: ThreadItem::UserMessage {
                id: format!("user:{turn_id}"),
                content: message.to_string(),
                attachments: vec![],
                artifacts: vec![],
            },
        },
        ThreadEvent::ItemAdded {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            item: ThreadItem::AgentMessage {
                id: format!("assistant:{turn_id}"),
                content: error.to_string(),
                pending: false,
            },
        },
        ThreadEvent::TurnCompleted {
            thread_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            status: TurnStatus::Failed,
            error: Some(error.to_string()),
        },
    ]
    .into_iter()
    .all(|event| state.apply_thread_event_deferred(event, None));
    drop(requests);
    applied
}
