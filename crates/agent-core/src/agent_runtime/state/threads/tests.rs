use super::super::super::AgentRuntimeState;
use crate::{ThreadEvent, ThreadUpdate};

fn start_update_turn(state: &AgentRuntimeState, session_id: &str) {
    assert!(state.apply_thread_event_deferred(
        ThreadEvent::TurnStarted {
            thread_id: session_id.into(),
            turn_id: "turn".into(),
        },
        None
    ));
}

fn update_text(session_id: &str, text: &str) -> ThreadEvent {
    ThreadEvent::AgentDelta {
        thread_id: session_id.into(),
        turn_id: "turn".into(),
        item_id: "assistant".into(),
        text: text.into(),
    }
}

#[test]
fn first_thread_update_is_an_authoritative_snapshot() {
    let state = AgentRuntimeState::default();
    assert!(state.take_thread_update("missing").is_none());
    start_update_turn(&state, "a");
    assert!(state.apply_thread_event_deferred(update_text("a", "hello"), None));
    let ThreadUpdate::Snapshot { snapshot } = state.take_thread_update("a").unwrap() else {
        panic!("the first publication must establish a baseline");
    };
    assert_eq!(snapshot, state.read_thread_snapshot("a").unwrap());
    assert_eq!(snapshot.revision, 2);
    assert!(state.take_thread_update("a").is_none());
}

#[test]
fn thread_delta_does_not_retransmit_existing_large_text() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    let existing = "large-existing-text".repeat(50_000);
    assert!(state.apply_thread_event_deferred(update_text("a", &existing), None));
    assert!(matches!(
        state.take_thread_update("a"),
        Some(ThreadUpdate::Snapshot { .. })
    ));
    assert!(state.apply_thread_event_deferred(update_text("a", "new tail"), None));
    let update = state.take_thread_update("a").unwrap();
    let ThreadUpdate::Delta {
        base_revision,
        revision,
        metadata,
        events,
        ..
    } = &update
    else {
        panic!("a small new event should use delta IPC");
    };
    assert_eq!((*base_revision, *revision), (2, 3));
    assert!(metadata.items.is_empty());
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], ThreadEvent::AgentDelta { text, .. } if text == "new tail"));
    assert!(serde_json::to_vec(&update).unwrap().len() < 2_000);
    assert!(matches!(&state.read_thread_snapshot("a").unwrap().items[0],
            crate::ThreadItem::AgentMessage { content, .. } if content == &(existing + "new tail")));
}

#[test]
fn coalesced_thread_delta_replays_every_accepted_event() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    let ThreadUpdate::Snapshot { mut snapshot } = state.take_thread_update("a").unwrap() else {
        panic!("initial snapshot expected");
    };
    for (index, text) in ["#", " heading", "\n\n", "**bold**"].iter().enumerate() {
        assert!(state.apply_thread_event_deferred(update_text("a", text), None));
        assert_eq!(state.schedule_thread_publication("a"), index == 0);
    }
    assert!(state.apply_thread_event_deferred(
        ThreadEvent::TurnCompleted {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            status: crate::TurnStatus::Completed,
            error: None,
        },
        None
    ));
    let ThreadUpdate::Delta {
        base_revision,
        revision,
        metadata,
        events,
        ..
    } = state.take_thread_update("a").unwrap()
    else {
        panic!("coalesced delta expected");
    };
    assert_eq!((base_revision, revision), (1, 6));
    assert_eq!(events.len(), 5);
    std::thread::sleep(std::time::Duration::from_millis(2));
    for event in events {
        assert!(snapshot.apply(event));
    }
    let authoritative = state.read_thread_snapshot("a").unwrap();
    // Clients replay item events, then use the server's clock metadata.
    assert_eq!(metadata.active_turn, authoritative.active_turn);
    snapshot.active_turn = metadata.active_turn.clone();
    assert_eq!(snapshot, authoritative);
    assert_eq!(metadata.status, crate::ThreadStatus::Completed);
    assert!(state.schedule_thread_publication("a"));
    assert!(state.take_thread_update("a").is_none());
}

#[test]
fn queue_revision_gap_falls_back_to_snapshot_then_resumes_deltas() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    state.take_thread_update("a").unwrap();
    state.replace_thread_queue(
        "a",
        vec![crate::QueuedThreadItem {
            id: "queued".into(),
            content: "next request".into(),
            images: Vec::new(),
            artifacts: Vec::new(),
        }],
    );
    assert!(state.apply_thread_event_deferred(update_text("a", "text"), None));
    let ThreadUpdate::Snapshot { snapshot } = state.take_thread_update("a").unwrap() else {
        panic!("queue changes are not represented in the event journal");
    };
    assert_eq!(snapshot, state.read_thread_snapshot("a").unwrap());
    assert_eq!(snapshot.queue.len(), 1);
    assert!(state.apply_thread_event_deferred(update_text("a", " tail"), None));
    assert!(matches!(
        state.take_thread_update("a"),
        Some(ThreadUpdate::Delta {
            base_revision: 3,
            revision: 4,
            ..
        })
    ));
}

#[test]
fn journal_event_limit_falls_back_without_losing_text() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    state.take_thread_update("a").unwrap();
    for _ in 0..257 {
        assert!(state.apply_thread_event_deferred(update_text("a", "x"), None));
    }
    {
        let journals = state.thread_updates.lock().unwrap();
        let journal = &journals["a"];
        assert_eq!(journal.events.len(), 256);
        assert!(journal.bytes <= 512 * 1024);
    }
    let ThreadUpdate::Snapshot { snapshot } = state.take_thread_update("a").unwrap() else {
        panic!("evicted events require a complete snapshot");
    };
    assert_eq!(snapshot.revision, 258);
    assert!(
        matches!(&snapshot.items[0], crate::ThreadItem::AgentMessage { content, .. }
            if content == &"x".repeat(257))
    );
    assert!(state.apply_thread_event_deferred(update_text("a", "tail"), None));
    assert!(matches!(
        state.take_thread_update("a"),
        Some(ThreadUpdate::Delta { .. })
    ));
}

#[test]
fn journal_byte_limit_and_oversized_event_preserve_snapshot_recovery() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    state.take_thread_update("a").unwrap();
    let text = "a".repeat(180_000);
    for _ in 0..3 {
        assert!(state.apply_thread_event_deferred(update_text("a", &text), None));
    }
    {
        let journals = state.thread_updates.lock().unwrap();
        let journal = &journals["a"];
        assert!(journal.events.len() < 3);
        assert_eq!(
            journal.bytes,
            journal
                .events
                .iter()
                .map(|(_, bytes, _)| bytes)
                .sum::<usize>()
        );
        assert!(journal.bytes <= 512 * 1024);
    }
    assert!(matches!(
        state.take_thread_update("a"),
        Some(ThreadUpdate::Snapshot { .. })
    ));
    let oversized = "b".repeat(600_000);
    assert!(state.apply_thread_event_deferred(update_text("a", &oversized), None));
    {
        let journals = state.thread_updates.lock().unwrap();
        assert!(journals["a"].events.is_empty());
        assert_eq!(journals["a"].bytes, 0);
    }
    let ThreadUpdate::Snapshot { snapshot } = state.take_thread_update("a").unwrap() else {
        panic!("the event size cap must never truncate authoritative state");
    };
    assert!(
        matches!(&snapshot.items[0], crate::ThreadItem::AgentMessage { content, .. }
            if content == &(text.repeat(3) + &oversized))
    );
}

#[test]
fn rejected_duplicate_stale_and_empty_events_do_not_create_revision_gaps() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    state.take_thread_update("a").unwrap();
    assert!(state.apply_thread_event_deferred(update_text("a", "accepted"), Some("event")));
    assert!(!state.apply_thread_event_deferred(update_text("a", "duplicate"), Some("event")));
    assert!(!state.apply_thread_event_deferred(
        ThreadEvent::AgentDelta {
            thread_id: "a".into(),
            turn_id: "old-turn".into(),
            item_id: "assistant".into(),
            text: "stale".into(),
        },
        Some("old-event")
    ));
    assert!(!state.apply_thread_event_deferred(update_text("a", ""), None));
    let ThreadUpdate::Delta {
        base_revision,
        revision,
        events,
        ..
    } = state.take_thread_update("a").unwrap()
    else {
        panic!("rejected events must not affect replay continuity");
    };
    assert_eq!((base_revision, revision, events.len()), (1, 2, 1));
    assert!(matches!(&events[0], ThreadEvent::AgentDelta { text, .. } if text == "accepted"));
}

#[test]
fn thread_update_baselines_and_event_dedup_are_session_isolated() {
    let state = AgentRuntimeState::default();
    start_update_turn(&state, "a");
    start_update_turn(&state, "b");
    state.take_thread_update("a").unwrap();
    for session in ["a", "b"] {
        assert!(
            state.apply_thread_event_deferred(update_text(session, session), Some("same-event-id"))
        );
    }
    let ThreadUpdate::Delta { id, events, .. } = state.take_thread_update("a").unwrap() else {
        panic!("a already has a baseline");
    };
    assert_eq!(id, "a");
    assert!(events.iter().all(|event| event.source_thread_id() == "a"));
    let ThreadUpdate::Snapshot { snapshot } = state.take_thread_update("b").unwrap() else {
        panic!("b still needs its own initial baseline");
    };
    assert_eq!(snapshot.id, "b");
    assert!(
        matches!(&snapshot.items[0], crate::ThreadItem::AgentMessage { content, .. } if content == "b")
    );
}

#[test]
fn reasoning_chunks_keep_a_bounded_item_count_and_preserve_boundaries() {
    let state = AgentRuntimeState::default();
    state.apply_thread_event(ThreadEvent::TurnStarted {
        thread_id: "a".into(),
        turn_id: "turn".into(),
    });
    for index in 0..2000 {
        assert!(state.apply_thread_event_deferred(
            ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "turn".into(),
                item: crate::ThreadItem::TimelineEvent {
                    id: format!("reasoning-{index}"),
                    data: serde_json::json!({"type":"reasoning_delta","text":"*\n"})
                },
            },
            None
        ));
    }
    let snapshot = state.read_thread_snapshot("a").unwrap();
    assert_eq!(snapshot.items.len(), 1);
    assert!(
        matches!(&snapshot.items[0], crate::ThreadItem::TimelineEvent { data, .. } if data["text"] == "*\n".repeat(2000))
    );
    state.apply_thread_event_deferred(
        ThreadEvent::AgentDelta {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            item_id: "assistant".into(),
            text: "answer".into(),
        },
        None,
    );
    state.apply_thread_event_deferred(
        ThreadEvent::ItemAdded {
            thread_id: "a".into(),
            turn_id: "turn".into(),
            item: crate::ThreadItem::TimelineEvent {
                id: "later-reasoning".into(),
                data: serde_json::json!({"type":"reasoning_delta","text":"later"}),
            },
        },
        None,
    );
    assert_eq!(state.read_thread_snapshot("a").unwrap().items.len(), 3);
}

#[test]
fn repeated_event_id_does_not_duplicate_a_snapshot_delta() {
    let state = AgentRuntimeState::default();
    let started = ThreadEvent::TurnStarted {
        thread_id: "session-a".to_string(),
        turn_id: "turn-a".to_string(),
    };
    assert!(state
        .apply_thread_event_once(started, Some("event-1"))
        .is_some());
    let message = ThreadEvent::ItemAdded {
        thread_id: "session-a".to_string(),
        turn_id: "turn-a".to_string(),
        item: crate::ThreadItem::AgentMessage {
            id: "assistant:turn-a".to_string(),
            content: String::new(),
            pending: true,
        },
    };
    assert!(state
        .apply_thread_event_once(message, Some("event-2"))
        .is_some());
    let delta = || ThreadEvent::AgentDelta {
        thread_id: "session-a".to_string(),
        turn_id: "turn-a".to_string(),
        item_id: "assistant:turn-a".to_string(),
        text: "hello".to_string(),
    };
    assert!(state
        .apply_thread_event_once(delta(), Some("event-3"))
        .is_some());
    assert!(state
        .apply_thread_event_once(delta(), Some("event-3"))
        .is_none());
    let snapshot = state
        .read_thread_snapshot("session-a")
        .expect("snapshot should persist");
    assert_eq!(snapshot.revision, 3);
    assert!(
        matches!(&snapshot.items[0], crate::ThreadItem::AgentMessage { content, .. } if content == "hello")
    );
}

#[test]
fn one_context_event_updates_both_projections_and_replays_idempotently() {
    let state = AgentRuntimeState::default();
    state
        .apply_thread_event(ThreadEvent::TurnStarted {
            thread_id: "a".into(),
            turn_id: "t".into(),
        })
        .unwrap();
    let context = ThreadEvent::ItemAdded {
        thread_id: "a".into(),
        turn_id: "t".into(),
        item: crate::ThreadItem::Context {
            id: "context-event".into(),
            data: serde_json::json!({"context_tokens": 900000}),
        },
    };
    let usage = ThreadEvent::UsageUpdated {
        thread_id: "a".into(),
        turn_id: "t".into(),
        usage: crate::ThreadUsageSnapshot {
            context_tokens: Some(900000),
            context_window: Some(1000000),
            ..Default::default()
        },
    };
    assert!(state
        .apply_thread_event_once(context.clone(), Some("context-event"))
        .is_some());
    assert!(state
        .apply_thread_event_once(usage.clone(), Some("context-event"))
        .is_some());
    assert!(state
        .apply_thread_event_once(context, Some("context-event"))
        .is_none());
    assert!(state
        .apply_thread_event_once(usage, Some("context-event"))
        .is_none());
    let snapshot = state.read_thread_snapshot("a").unwrap();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.runtime.usage.context_tokens, Some(900000));
    assert_eq!(snapshot.revision, 3);
    assert!(state.read_thread_snapshot_after("a", Some(3)).is_none());
    assert!(state.read_thread_snapshot_after("a", Some(4)).is_none());
    assert_eq!(
        state.read_thread_snapshot_after("a", Some(2)),
        Some(snapshot)
    );
    assert!(state.read_thread_snapshot_after("missing", None).is_none());
}
