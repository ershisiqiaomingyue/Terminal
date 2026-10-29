use crate::{is_cancelled, register_pending_permission};
use crate::{AgentRequestGuard, AgentRuntimeState};
use crate::{AgentSteerRequest, ThreadEvent, ThreadUpdate};
use std::time::Duration;

#[test]
fn request_locks_are_scoped_to_a_session() {
    let state = AgentRuntimeState::default();
    let first = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("first session should start");
    let second = AgentRequestGuard::start(&state, "session-b".to_string(), "turn-b".to_string())
        .expect("different session should start concurrently");

    let duplicate = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-c".to_string());
    assert!(duplicate.is_err());

    drop(first);
    AgentRequestGuard::start(&state, "session-a".to_string(), "turn-d".to_string())
        .expect("session lock should release when its request ends");
    drop(second);
}

#[test]
fn owned_guard_retains_shared_state_after_caller_is_dropped() {
    let state = AgentRuntimeState::default();
    let shared = state.clone();
    let guard = AgentRequestGuard::start_owned(&state, "session".into(), "turn".into())
        .expect("owned lease");
    let cancellation = guard.cancellation();
    drop(state);
    assert!(!is_cancelled(&shared, "session", &cancellation));
    assert!(AgentRequestGuard::start(&shared, "session".into(), "other".into()).is_err());
    drop(guard);
    assert!(is_cancelled(&shared, "session", &cancellation));
    AgentRequestGuard::start_owned(&shared, "session".into(), "next".into())
        .expect("lease released");
}

#[test]
fn stale_owned_guard_does_not_clear_new_turn_approvals_or_permissions() {
    let state = AgentRuntimeState::default();
    let stale =
        AgentRequestGuard::start_owned(&state, "session".into(), "old".into()).expect("old lease");
    let old_cancellation = stale.cancellation();
    let _old_permission = register_pending_permission(
        &state,
        "session",
        "old-permission".into(),
        "old-key".into(),
        "old-call".into(),
        old_cancellation,
        "old".into(),
        None,
    )
    .expect("old permission");
    state.requests.lock().unwrap().remove("session");
    let current =
        AgentRequestGuard::start_owned(&state, "session".into(), "new".into()).expect("new lease");
    let _new_permission = register_pending_permission(
        &state,
        "session",
        "new-permission".into(),
        "new-key".into(),
        "new-call".into(),
        current.cancellation(),
        "new".into(),
        None,
    )
    .expect("new permission");
    state
        .one_time_approvals
        .lock()
        .unwrap()
        .insert(("session".into(), "new-key".into()));
    drop(stale);
    assert_eq!(state.requests.lock().unwrap()["session"].turn_id, "new");
    let permissions = state.permissions.lock().unwrap();
    assert!(!permissions.contains_key(&("session".into(), "old-permission".into())));
    assert!(permissions.contains_key(&("session".into(), "new-permission".into())));
    drop(permissions);
    assert!(state
        .one_time_approvals
        .lock()
        .unwrap()
        .contains(&("session".into(), "new-key".into())));
    drop(current);
    assert!(state.permissions.lock().unwrap().is_empty());
    assert!(state.one_time_approvals.lock().unwrap().is_empty());
}

#[test]
fn steer_messages_are_scoped_to_their_session() {
    let state = AgentRuntimeState::default();
    let mut first = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("first session should start");
    let mut second =
        AgentRequestGuard::start(&state, "session-b".to_string(), "turn-b".to_string())
            .expect("second session should start");
    let first_sender = state
        .requests
        .lock()
        .expect("request state")
        .get("session-a")
        .expect("first request")
        .steer_sender
        .clone();
    first_sender
        .try_send(AgentSteerRequest {
            message: "continue A".to_string(),
            attachments: vec![],
            artifacts: vec![],
            source_message_id: None,
        })
        .expect("steer should enqueue");

    assert_eq!(
        first
            .take_steer_receiver()
            .try_recv()
            .expect("first steer")
            .message,
        "continue A"
    );
    assert!(second.take_steer_receiver().try_recv().is_err());
}

#[test]
fn tool_call_counts_are_scoped_to_a_turn_and_deduplicate_call_ids() {
    let state = AgentRuntimeState::default();
    let _request = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("request should start");

    assert_eq!(
        state.record_tool_call_started("session-a", "turn-a", "call-a"),
        Some(1)
    );
    assert_eq!(
        state.record_tool_call_started("session-a", "turn-a", "call-a"),
        None
    );
    assert_eq!(state.tool_call_count("session-a", "turn-a"), 1);
    assert_eq!(
        state.record_tool_call_started("session-a", "stale-turn", "call-b"),
        None
    );
    assert_eq!(state.tool_call_count("session-a", "stale-turn"), 0);
}

#[test]
fn request_cancellation_sets_flag_reason_and_token_together() {
    let state = AgentRuntimeState::default();
    let guard = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("request should start");
    let token = guard.cancellation_token();
    assert!(!token.is_cancelled());

    state
        .requests
        .lock()
        .expect("request state")
        .get("session-a")
        .expect("control registered")
        .request_cancellation("请求已取消");

    assert!(guard
        .cancellation()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(
        guard
            .cancellation_reason()
            .lock()
            .expect("reason lock")
            .as_deref(),
        Some("请求已取消")
    );
    assert!(token.is_cancelled());
}

#[test]
fn cancellation_reasons_are_scoped_to_the_request_guard() {
    let state = AgentRuntimeState::default();
    let first = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("first request should start");
    let second = AgentRequestGuard::start(&state, "session-b".to_string(), "turn-b".to_string())
        .expect("second request should start");
    *first.cancellation_reason().lock().expect("reason lock") = Some("执行超时".to_string());

    assert_eq!(
        first
            .cancellation_reason()
            .lock()
            .expect("reason lock")
            .as_deref(),
        Some("执行超时")
    );
    assert_eq!(
        second
            .cancellation_reason()
            .lock()
            .expect("reason lock")
            .as_deref(),
        None
    );
}

#[test]
fn circuit_breaker_cancels_only_the_failing_turn() {
    let state = AgentRuntimeState::default();
    let first = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("first request should start");
    let second = AgentRequestGuard::start(&state, "session-b".to_string(), "turn-b".to_string())
        .expect("second request should start");

    for index in 0..7 {
        assert_eq!(
            state.record_tool_call_finished(
                "session-a",
                "turn-a",
                &format!("call-{index}"),
                "shell",
                true
            ),
            None,
        );
    }
    let reason = state
        .record_tool_call_finished("session-a", "turn-a", "call-8", "shell", true)
        .expect("eighth consecutive failure should cancel the turn");

    assert_eq!(reason, "工具 shell 连续失败 8 次");
    assert!(first
        .cancellation()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(!second
        .cancellation()
        .load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(
        second
            .cancellation_reason()
            .lock()
            .expect("reason lock")
            .as_deref(),
        None
    );
}

#[test]
fn idle_watchdog_ignores_active_tools_and_stale_turns() {
    let state = AgentRuntimeState::default();
    let request = AgentRequestGuard::start(&state, "session-a".to_string(), "turn-a".to_string())
        .expect("request should start");
    state.record_tool_call_started("session-a", "turn-a", "call-a");

    assert_eq!(
        state.cancel_if_turn_idle(
            "session-a",
            "turn-a",
            &request.cancellation(),
            Duration::ZERO
        ),
        None,
    );
    state.record_tool_call_finished("session-a", "turn-a", "call-a", "shell", false);
    assert!(state
        .cancel_if_turn_idle(
            "session-a",
            "stale-turn",
            &request.cancellation(),
            Duration::ZERO
        )
        .is_none());
    assert_eq!(
        state.cancel_if_turn_idle(
            "session-a",
            "turn-a",
            &request.cancellation(),
            Duration::ZERO
        ),
        Some("[turn_idle_timeout] 连续 0 秒未收到模型或工具进度".to_string()),
    );
}

#[test]
fn thread_snapshots_are_scoped_to_a_session() {
    let state = AgentRuntimeState::default();
    let snapshot = state
        .apply_thread_event(ThreadEvent::TurnStarted {
            thread_id: "session-a".to_string(),
            turn_id: "turn-a".to_string(),
        })
        .expect("event should create a snapshot");
    assert_eq!(snapshot.id, "session-a");
    assert_eq!(state.read_thread_snapshot("session-b"), None);
    assert_eq!(
        state
            .read_thread_snapshot("session-a")
            .expect("snapshot should persist")
            .revision,
        1
    );
}

#[test]
fn explicit_history_reset_is_monotonic_and_releases_old_projection_dedup() {
    let state = AgentRuntimeState::default();
    let event = || ThreadEvent::TurnStarted {
        thread_id: "a".into(),
        turn_id: "turn".into(),
    };
    state
        .apply_thread_event_once(event(), Some("original-seq-1"))
        .unwrap();
    state
        .apply_thread_event(ThreadEvent::AgentDelta {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            item_id: "assistant".into(),
            text: "old text".into(),
        })
        .unwrap();
    let before = state.read_thread_snapshot("a").unwrap();
    let reset = state.reset_idle_thread("a").unwrap();
    assert!(reset.revision > before.revision);
    assert!(reset.items.is_empty());
    assert!(matches!(
        state.take_thread_update("a"),
        Some(ThreadUpdate::Snapshot { .. })
    ));
    assert!(state
        .apply_thread_event_once(event(), Some("original-seq-1"))
        .is_some());
    let _guard = AgentRequestGuard::start(&state, "a".into(), "active".into()).unwrap();
    let active = state.read_thread_snapshot("a").unwrap();
    assert!(state.reset_idle_thread("a").is_none());
    assert_eq!(state.read_thread_snapshot("a"), Some(active));
}

#[test]
fn deferred_deltas_publish_once_and_terminal_state_keeps_all_text() {
    let state = AgentRuntimeState::default();
    state.apply_thread_event(ThreadEvent::TurnStarted {
        thread_id: "a".into(),
        turn_id: "turn".into(),
    });
    let chunks = [
        "#", "#", " heading", "\n", "\n", "*", "*", "bold", "*", "*", "\n", "    ",
    ];
    for (index, text) in chunks.iter().enumerate() {
        let event_id = format!("delta-{index}");
        let delta = || ThreadEvent::AgentDelta {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            item_id: "assistant".into(),
            text: text.to_string(),
        };
        assert!(state.apply_thread_event_deferred(delta(), Some(&event_id)));
        assert!(!state.apply_thread_event_deferred(delta(), Some(&event_id)));
        assert_eq!(state.schedule_thread_publication("a"), index == 0);
    }
    let completed = state
        .apply_thread_event(ThreadEvent::TurnCompleted {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            status: crate::TurnStatus::Completed,
            error: None,
        })
        .unwrap();
    assert_eq!(completed.status, crate::ThreadStatus::Completed);
    assert!(
        matches!(&completed.items[0], crate::ThreadItem::AgentMessage { content, pending, .. } if content == &chunks.concat() && !pending)
    );
    assert_eq!(state.take_thread_publication("a"), Some(completed));
    assert!(state.schedule_thread_publication("a"));
}
