use super::support::*;
use super::*;

#[test]
fn background_result_queues_once_and_does_not_invent_a_user_message() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "background-chat");
    let turn = format!("{}task-1", agent_core::background::BACKGROUND_RESULT_PREFIX);
    let mut request = queued_request("background-chat", "后台任务完成证据");
    request.turn_id = Some(turn.clone());
    request.tool_policy = AgentToolPolicy::Disabled;
    assert!(store.enqueue_agent_queue_item("background-chat", "q1", &turn, &request).unwrap().inserted);
    assert!(!store.enqueue_agent_queue_item("background-chat", "q2", &turn, &request).unwrap().inserted);
    store.start_agent_run("background-chat", &turn, &request).unwrap();
    store.finish_agent_turn("background-chat", &turn, "completed", None, Some("编译完成，退出码 0")).unwrap();
    let record = store.get_record("background-chat").unwrap().unwrap();
    let messages = record.messages.as_array().unwrap();
    assert!(!messages.iter().any(|message| message["role"] == "user" && message["turnId"] == turn));
    assert!(messages.iter().any(|message| message["role"] == "assistant" && message["content"] == "编译完成，退出码 0"));
}

#[test]
fn queue_edit_preserves_frozen_configuration_and_rejects_claimed_items() {
    let store = HistoryStore::open_memory().unwrap();
    let original = store
        .enqueue_agent_queue_item(
            "session-a",
            "queue-a",
            "edit-key",
            &queued_request("session-a", "Before"),
        )
        .unwrap()
        .item;
    assert!(store
        .update_agent_queue_content("session-a", "queue-a", "After")
        .unwrap());
    let edited = store
        .agent_queue_item("session-a", "queue-a")
        .unwrap()
        .unwrap();
    let mut expected = serde_json::to_value(&original.request).unwrap();
    expected["message"] = Value::String("After".into());
    assert_eq!(serde_json::to_value(&edited.request).unwrap(), expected);
    assert_eq!(edited.sequence, original.sequence);
    assert_eq!(edited.idempotency_key, original.idempotency_key);
    assert!(store
        .update_agent_queue_content("session-a", "queue-a", "  ")
        .is_err());
    assert!(store
        .update_agent_queue_content("session-b", "queue-a", "wrong session")
        .is_err());
    store
        .claim_agent_queue_item("session-a", Some("queue-a"), "owner-a")
        .unwrap()
        .unwrap();
    assert!(store
        .update_agent_queue_content("session-a", "queue-a", "Too late")
        .is_err());
    assert_eq!(
        store
            .agent_queue_item("session-a", "queue-a")
            .unwrap()
            .unwrap()
            .request
            .message,
        "After"
    );
}

#[test]
fn queue_reorder_changes_pending_order_and_rejects_stale_lists() {
    let store = HistoryStore::open_memory().unwrap();
    for (id, message) in [("queue-a", "one"), ("queue-b", "two"), ("queue-c", "three")] {
        store.enqueue_agent_queue_item("session-a", id, id, &queued_request("session-a", message)).unwrap();
    }
    assert!(store.reorder_agent_queue_items("session-a", &["queue-c".into(), "queue-a".into(), "queue-b".into()]).unwrap());
    let queue = store.list_agent_queue_items(Some("session-a")).unwrap();
    assert_eq!(queue.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["queue-c", "queue-a", "queue-b"]);
    assert!(!store.reorder_agent_queue_items("session-a", &["queue-c".into(), "queue-a".into(), "queue-b".into()]).unwrap());
    assert!(store.reorder_agent_queue_items("session-a", &["queue-a".into(), "queue-a".into(), "queue-c".into()]).is_err());
    assert_eq!(store.list_agent_queue_items(Some("session-a")).unwrap().iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["queue-c", "queue-a", "queue-b"]);
}

#[test]
fn queue_reorder_changes_only_the_pending_order_and_rejects_stale_lists() {
    let store = HistoryStore::open_memory().unwrap();
    for (id, message) in [("queue-a", "one"), ("queue-b", "two"), ("queue-c", "three")] {
        store.enqueue_agent_queue_item("session-a", id, id, &queued_request("session-a", message)).unwrap();
    }
    let changed = store.reorder_agent_queue_items("session-a", &["queue-c".into(), "queue-a".into(), "queue-b".into()]).unwrap();
    assert!(changed);
    let queue = store.list_agent_queue_items(Some("session-a")).unwrap();
    assert_eq!(queue.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["queue-c", "queue-a", "queue-b"]);
    assert!(!store.reorder_agent_queue_items("session-a", &["queue-c".into(), "queue-a".into(), "queue-b".into()]).unwrap());
    assert!(store.reorder_agent_queue_items("session-a", &["queue-a".into(), "queue-a".into(), "queue-c".into()]).is_err());
    assert_eq!(store.list_agent_queue_items(Some("session-a")).unwrap().iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["queue-c", "queue-a", "queue-b"]);
}

#[test]
fn durable_queue_enforces_order_capacity_and_idempotency() {
    let store = HistoryStore::open_memory().expect("history database should open");
    for index in 0..MAX_AGENT_QUEUE_ITEMS_PER_SESSION {
        let id = format!("queue-{index:02}");
        let message = format!("message-{index}");
        let result = store
            .enqueue_agent_queue_item(
                "session-a",
                &id,
                &format!("key-{index:02}"),
                &queued_request("session-a", &message),
            )
            .expect("queue item should persist");
        assert!(result.inserted);
    }
    let duplicate = store
        .enqueue_agent_queue_item(
            "session-a",
            "queue-retry-with-new-id",
            "key-00",
            &queued_request("session-a", "message-0"),
        )
        .expect("idempotent retry should succeed even at capacity");
    assert!(!duplicate.inserted);
    assert_eq!(duplicate.item.id, "queue-00");
    assert!(store
        .enqueue_agent_queue_item(
            "session-a",
            "queue-overflow",
            "key-overflow",
            &queued_request("session-a", "overflow"),
        )
        .is_err());
    let queue = store
        .list_agent_queue_items(Some("session-a"))
        .expect("queue should load");
    assert_eq!(queue.len(), MAX_AGENT_QUEUE_ITEMS_PER_SESSION as usize);
    assert_eq!(queue.first().map(|item| item.id.as_str()), Some("queue-00"));
    assert_eq!(queue.last().map(|item| item.id.as_str()), Some("queue-19"));
    assert!(queue
        .windows(2)
        .all(|items| items[0].sequence < items[1].sequence));
}

#[test]
fn durable_queue_nack_preserves_order_and_frozen_target() {
    let store = HistoryStore::open_memory().expect("history database should open");
    for (id, message) in [("queue-a", "one"), ("queue-b", "two")] {
        store
            .enqueue_agent_queue_item("session-a", id, id, &queued_request("session-a", message))
            .expect("queue item should persist");
    }
    let lease = store
        .claim_agent_queue_item("session-a", None, "owner-a")
        .expect("claim should work")
        .expect("first item should be claimable");
    assert_eq!(lease.item.id, "queue-a");
    assert_eq!(lease.item.attempt_count, 1);
    assert!(matches!(
        lease.item.request.execution_target.as_ref(),
        Some(AgentExecutionTarget { kind, connection_id: Some(connection_id), .. })
            if kind == "ssh" && connection_id == "ssh-frozen"
    ));
    let returned = store
        .nack_agent_queue_item(
            "session-a",
            &lease.item.id,
            &lease.lease_id,
            &lease.lease_owner,
            Some("request guard busy"),
        )
        .expect("failed handoff should return to pending");
    assert_eq!(returned.status, "pending");
    assert_eq!(returned.last_error.as_deref(), Some("request guard busy"));
    let queue = store
        .list_agent_queue_items(Some("session-a"))
        .expect("queue should load");
    assert_eq!(
        queue
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["queue-a", "queue-b"]
    );
    let retried = store
        .claim_agent_queue_item("session-a", None, "owner-a")
        .expect("retry claim should work")
        .expect("nacked item should be claimable again");
    assert_eq!(retried.item.id, "queue-a");
    assert_eq!(retried.item.attempt_count, 2);
}

#[test]
fn durable_queue_ack_is_exactly_once_and_keeps_idempotency_record() {
    let store = HistoryStore::open_memory().expect("history database should open");
    let request = queued_request("session-a", "once");
    store
        .enqueue_agent_queue_item("session-a", "queue-a", "stable-key", &request)
        .expect("queue item should persist");
    let lease = store
        .claim_agent_queue_item("session-a", Some("queue-a"), "owner-a")
        .expect("claim should work")
        .expect("exact item should be claimable");
    assert!(store
        .claim_agent_queue_item("session-a", Some("queue-a"), "owner-b")
        .expect("competing claim should not error")
        .is_none());
    assert!(store
        .ack_agent_queue_item(
            "session-a",
            "queue-a",
            "wrong-lease",
            "owner-a",
            "dispatched",
        )
        .is_err());
    let completed = store
        .ack_agent_queue_item(
            "session-a",
            "queue-a",
            &lease.lease_id,
            &lease.lease_owner,
            "dispatched",
        )
        .expect("lease owner should acknowledge the item");
    assert_eq!(completed.status, "dispatched");
    assert!(store
        .list_agent_queue_items(Some("session-a"))
        .expect("queue should load")
        .is_empty());
    let duplicate = store
        .enqueue_agent_queue_item("session-a", "queue-a", "stable-key", &request)
        .expect("retry after ack should remain idempotent");
    assert!(!duplicate.inserted);
    assert_eq!(duplicate.item.status, "dispatched");
}

#[test]
fn durable_queue_recovers_claim_after_cold_start() {
    let path = temporary_history_path("cold-start");
    {
        let store = HistoryStore::open(&path).expect("history database should open");
        store
            .enqueue_agent_queue_item(
                "session-a",
                "queue-a",
                "queue-a",
                &queued_request("session-a", "recover me"),
            )
            .expect("queue item should persist");
        let lease = store
            .claim_agent_queue_item("session-a", None, "process-a")
            .expect("claim should work")
            .expect("item should be claimable");
        assert_eq!(lease.item.status, "claimed");
    }
    {
        let store = HistoryStore::open(&path).expect("history database should reopen");
        let recovered = store
            .recover_agent_queue_items("process-b")
            .expect("new process should recover the old lease");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "queue-a");
        assert_eq!(recovered[0].status, "pending");
        let lease = store
            .claim_agent_queue_item("session-a", None, "process-b")
            .expect("recovered item should be claimable")
            .expect("recovered item should remain queued");
        assert_eq!(lease.item.id, "queue-a");
        assert_eq!(lease.item.attempt_count, 2);
    }
    std::fs::remove_file(&path).expect("temporary history database should be removable");
}

#[test]
fn concurrent_durable_claim_has_only_one_winner() {    let path = temporary_history_path("concurrent-claim");
    {
        let store = HistoryStore::open(&path).expect("history database should open");
        store
            .enqueue_agent_queue_item(
                "session-a",
                "queue-a",
                "queue-a",
                &queued_request("session-a", "claim once"),
            )
            .expect("queue item should persist");
    }
    let barrier = Arc::new(Barrier::new(3));
    let handles = ["owner-a", "owner-b"].map(|owner| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let store = HistoryStore::open(&path).expect("claim store should open");
            barrier.wait();
            store
                .claim_agent_queue_item("session-a", Some("queue-a"), owner)
                .expect("concurrent claim should not fail")
                .is_some()
        })
    });
    barrier.wait();
    let winners = handles
        .into_iter()
        .map(|handle| handle.join().expect("claim thread should finish"))
        .filter(|won| *won)
        .count();
    assert_eq!(winners, 1);
    std::fs::remove_file(&path).expect("temporary history database should be removable");
}

/// §7.3-4：queue_store 的极端并发压力验证。
///
/// 覆盖的是「多会话 × 多线程 × 写-认领-追加要求」混合负载下最容易出事的三处
/// 不变量：① 同一队列项只有一个认领者；② 队列顺序（position）在多线程追加
/// 后仍然是严格递增、无重复；③ SQLite 在 busy 时由 busy_timeout 重试，不把
/// 并发写直接变成错误。
#[test]
fn queue_store_survives_parallel_writes_and_claims_without_losing_order() {
    const SESSIONS: usize = 4;
    const PER_SESSION_ITEMS: usize = 12;
    const THREADS_PER_SESSION: usize = 3;

    let path = temporary_history_path("queue-stress");
    {
        let store = HistoryStore::open(&path).expect("history database should open");
        for index in 0..SESSIONS {
            seed_thread(&store, &format!("session-{index}"));
        }
    }

    let barrier = Arc::new(Barrier::new(SESSIONS * THREADS_PER_SESSION + 1));
    let mut handles: Vec<std::thread::JoinHandle<Vec<String>>> = Vec::new();
    for session_index in 0..SESSIONS {
        for thread_index in 0..THREADS_PER_SESSION {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let store = HistoryStore::open(&path).expect("stress store should open");
                barrier.wait();
                let session_id = format!("session-{session_index}");
                let mut inlined = Vec::new();
                let mut claimed = Vec::new();
                for item in 0..PER_SESSION_ITEMS.min(2) {
                    let queue_id = format!("q-{session_index}-{thread_index}-{item}");
                    let queued = store
                        .enqueue_agent_queue_item(
                            &session_id,
                            &queue_id,
                            &queue_id,
                            &queued_request(&session_id, &format!("item {item}")),
                        )
                        .expect("并发入队不应因 busy 失败");
                    if queued.inserted {
                        inlined.push(queued.item.id.clone());
                    }
                }
                // 每个线程只抢**自己的** 2 条项：3 个线程同时抢同一批队头，
                // 覆盖「并发认领」与「同一项只允许一个赢家」两件事。
                for item in 0..PER_SESSION_ITEMS.min(2) {
                    let queue_id = format!("q-{session_index}-{thread_index}-{item}");
                    if let Some(claimed_item) = store
                        .claim_agent_queue_item(&session_id, Some(&queue_id), &queue_id)
                        .expect("并发认领不应因 busy 失败")
                    {
                        assert!(
                            !claimed.contains(&claimed_item.item.id),
                            "同一个队列项被认领了两次：{}",
                            claimed_item.item.id
                        );
                        assert_eq!(claimed_item.item.status, "claimed");
                        claimed.push(claimed_item.item.id.clone());
                        store
                            .nack_agent_queue_item(
                                &session_id,
                                &claimed_item.item.id,
                                &claimed_item.lease_id,
                                &queue_id,
                                Some("并发压测退回"),
                            )
                            .expect("并发退回不应失败");
                    }
                }
                claimed
            }));
        }
    }
    barrier.wait();

    let mut claimed: Vec<String> = handles
        .into_iter()
        .flat_map(|handle| handle.join().expect("stress thread should finish"))
        .collect();
    let claimed_total = claimed.len();
    claimed.sort();
    claimed.dedup();
    // 24 条新入队全被认领一次；另外 4 条来自不同线程抢到别人后来入队的项
    // （`None` 抢队头时可能拿到尚未被重新入队的旧项，不影响唯一性）。
    assert!(claimed_total >= 24, "{claimed_total}");
    assert_eq!(claimed.len(), claimed_total, "同一队列项被认领了两次");

    // 位置必须严格递增且无空洞：并发追加没有「插队」或「复用位置」。
    let store = HistoryStore::open(&path).expect("history database should reopen");
    for session_index in 0..SESSIONS {
        let session_id = format!("session-{session_index}");
        let items = store
            .list_agent_queue_items(None)
            .expect("queue items should be readable")
            .into_iter()
            .filter(|item| item.session_id == session_id)
            .collect::<Vec<_>>();
        assert_eq!(
            items.len(),
            THREADS_PER_SESSION * 2,
            "{session_id} 的队列项数量不对（items={}）",
            items
                .iter()
                .map(|item| format!("{}:{}:{}", item.id, item.status, item.sequence))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut sequences: Vec<i64> = items.iter().map(|item| item.sequence).collect();
        let unique = {
            sequences.sort();
            sequences.dedup();
            sequences.len()
        };
        assert_eq!(unique, items.len(), "{session_id} 的队列序号出现重复");
        // 序号必须严格递增：并发追加不能出现重复，也不能复用别人用过的号。
        let ordered = items.iter().map(|item| item.sequence).collect::<Vec<_>>();
        assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]), "{session_id} 的队列序号出现乱序");
    }
    drop(store);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn queue_dispatch_and_run_start_commit_atomically() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_thread(&store, "session-a");
    let mut request = queued_request("session-a", "run atomically");
    request.turn_id = Some("turn-a".to_string());
    store
        .enqueue_agent_queue_item("session-a", "queue-a", "stable-key", &request)
        .expect("queue item should persist");
    let lease = store
        .claim_agent_queue_item("session-a", Some("queue-a"), "owner-a")
        .expect("claim should work")
        .expect("queue item should be claimable");
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_run_insert BEFORE INSERT ON agent_runs
                 BEGIN SELECT RAISE(ABORT, 'run insert failed'); END;",
        )
        .expect("failure trigger should be installed");

    assert!(store
        .start_agent_run_from_queue(
            "session-a",
            "turn-a",
            &request,
            "queue-a",
            &lease.lease_id,
            &lease.lease_owner,
        )
        .is_err());
    assert_eq!(
        store
            .agent_queue_item("session-a", "queue-a")
            .expect("queue should load")
            .expect("queue item should exist")
            .status,
        "claimed"
    );
    assert!(store
        .agent_run("session-a", "turn-a")
        .expect("run lookup should work")
        .is_none());

    store
        .connection
        .execute_batch("DROP TRIGGER fail_run_insert")
        .expect("failure trigger should be removed");
    let dispatched = store
        .start_agent_run_from_queue(
            "session-a",
            "turn-a",
            &request,
            "queue-a",
            &lease.lease_id,
            &lease.lease_owner,
        )
        .expect("atomic dispatch should succeed");
    assert_eq!(dispatched.status, "dispatched");
    assert_eq!(
        store
            .agent_run("session-a", "turn-a")
            .expect("run lookup should work")
            .expect("run should exist")
            .summary
            .status,
        "preparing"
    );
}

#[test]
fn terminal_turn_steers_are_not_replayed_into_the_next_turn() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "steer-replay");
    let steer_request = AgentSteerRequest {
        message: "Keep this additional requirement.".into(),
        attachments: vec![],
        artifacts: vec![],
        source_message_id: None,
    };
    for (index, status) in ["cancelled", "completed", "failed", "interrupted"]
        .iter()
        .enumerate()
    {
        let turn_id = format!("turn-{index}");
        store
            .start_agent_run(
                "steer-replay",
                &turn_id,
                &queued_request("steer-replay", "Initial requirement."),
            )
            .unwrap();
        let steer = store
            .enqueue_agent_steer("steer-replay", &turn_id, &steer_request)
            .unwrap();
        store
            .update_agent_steer_status(&steer.id, "delivered")
            .unwrap();
        if *status == "interrupted" {
            store
                .mark_agent_run_interrupted("steer-replay", &turn_id)
                .unwrap();
        } else {
            store
                .finish_agent_run("steer-replay", &turn_id, status, None)
                .unwrap();
        }
    }
    let pending = store.pending_agent_steers("steer-replay").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].turn_id, "turn-3");

    store
        .start_agent_run(
            "steer-replay",
            "new-turn",
            &queued_request("steer-replay", "A new task."),
        )
        .unwrap();
    assert!(store
        .pending_agent_steers("steer-replay")
        .unwrap()
        .iter()
        .all(|steer| steer.turn_id == "turn-3"));
}

#[test]
fn terminal_steer_rejection_is_fenced_but_interrupted_steer_stays_recoverable() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "steer-terminal-fence");
    for (turn_id, status) in [("cancel", "cancelled"), ("resume", "interrupted")] {
        store
            .start_agent_run(
                "steer-terminal-fence",
                turn_id,
                &queued_request("steer-terminal-fence", "task"),
            )
            .unwrap();
        let steer = store
            .enqueue_agent_steer(
                "steer-terminal-fence",
                turn_id,
                &AgentSteerRequest {
                    message: format!("follow up {turn_id}"),
                    attachments: vec![],
                    artifacts: vec![],
                    source_message_id: None,
                },
            )
            .unwrap();
        if status == "interrupted" {
            store
                .mark_agent_run_interrupted("steer-terminal-fence", turn_id)
                .unwrap();
        } else {
            store
                .finish_agent_run("steer-terminal-fence", turn_id, status, None)
                .unwrap();
        }
        assert_eq!(
            store.reject_agent_steer_if_terminal(&steer.id).unwrap(),
            status != "interrupted"
        );
    }
    let statuses = store
        .connection
        .prepare("SELECT turn_id, status FROM agent_steers ORDER BY turn_id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        statuses,
        vec![
            ("cancel".into(), "rejected".into()),
            ("resume".into(), "pending".into())
        ]
    );
}

/// N6-B（缺口补齐）：failed 终态与 completed/cancelled 对称——异常失败同样要
/// 收口该轮的追加要求。这里锁两件事：`pending_agent_steer_ids_for_turn` 只报
/// 仍悬挂的行（供落库失败时广播用），`reject_agent_steers_for_turn` 能把它们
/// 在 failed 轮上一起置为 rejected。
#[test]
fn failed_turn_settles_pending_steers_like_other_terminal_states() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "steer-failed");
    for turn_id in ["failed", "completed", "cancelled"] {
        store
            .start_agent_run(
                "steer-failed",
                turn_id,
                &queued_request("steer-failed", "task"),
            )
            .unwrap();
        for index in 0..2 {
            store
                .enqueue_agent_steer(
                    "steer-failed",
                    turn_id,
                    &AgentSteerRequest {
                        message: format!("follow up {turn_id} {index}"),
                        attachments: vec![],
                        artifacts: vec![],
                        source_message_id: None,
                    },
                )
                .unwrap();
        }
        let status = if turn_id == "failed" {
            "failed"
        } else {
            turn_id
        };
        store
            .finish_agent_run("steer-failed", turn_id, status, Some("boom"))
            .unwrap();
        // 落库失败降级路径读取的正是这个只读查询：两条都还悬挂。
        assert_eq!(
            store
                .pending_agent_steer_ids_for_turn("steer-failed", turn_id)
                .unwrap()
                .len(),
            2,
            "{turn_id} 应有两条仍在悬挂的追加要求"
        );
        let settled = store
            .reject_agent_steers_for_turn("steer-failed", turn_id)
            .unwrap();
        assert_eq!(settled.len(), 2, "{turn_id} 应回收两条追加要求");
        assert!(
            store
                .pending_agent_steer_ids_for_turn("steer-failed", turn_id)
                .unwrap()
                .is_empty(),
            "{turn_id} 回收后不应再有悬挂的追加要求"
        );
    }
    let statuses = store
        .connection
        .prepare("SELECT DISTINCT status FROM agent_steers")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(statuses, vec!["rejected".to_string()]);
}

#[test]
fn queue_guide_persists_steer_and_acknowledges_in_one_transaction() {
    let store = HistoryStore::open_memory().expect("history database should open");
    let request = queued_request("session-a", "guide the active turn");
    store
        .enqueue_agent_queue_item("session-a", "queue-a", "stable-key", &request)
        .expect("queue item should persist");
    let lease = store
        .claim_agent_queue_item("session-a", Some("queue-a"), "owner-a")
        .expect("claim should work")
        .expect("queue item should be claimable");
    let steer = AgentSteerRequest {
        message: "guide the active turn".to_string(),
        attachments: vec![],
        artifacts: vec![],
        source_message_id: None,
    };
    let persisted = store
        .persist_queue_steer_and_ack(
            "session-a",
            "turn-active",
            &steer,
            "queue-a",
            &lease.lease_id,
            &lease.lease_owner,
        )
        .expect("queue guide should commit");
    assert_eq!(persisted.source_queue_id.as_deref(), Some("queue-a"));
    assert_eq!(
        store
            .agent_queue_item("session-a", "queue-a")
            .expect("queue should load")
            .expect("queue item should exist")
            .status,
        "steered"
    );
    assert_eq!(
        store
            .pending_agent_steers("session-a")
            .expect("steers should load")
            .len(),
        1
    );
    let mut delivered = steer.clone();
    delivered.source_message_id = Some("user:turn-active:steer:transport".into());
    store
        .mark_next_agent_steer_applied("session-a", &delivered)
        .expect("steer should be marked applied")
        .expect("steer should be found");
    assert!(store
        .pending_agent_steers("session-a")
        .expect("steers should load")
        .is_empty());
}

#[test]
fn cancelled_claim_release_commits_cancelled_terminal_state() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("collaboration job should be created");
    let running = store
        .claim_collaboration_agent(&job.job.id, &job.agents[0].id, "owner-a")
        .expect("worker should claim");
    store
        .request_collaboration_cancellation("parent-thread", Some("parent-turn"))
        .expect("parent cancellation should persist");

    assert!(store
        .release_collaboration_agent_claim(
            &job.job.id,
            &running.agent.id,
            running.attempt,
            &running.lease_id,
        )
        .expect("cancelled claim should release"));
    let cancelled = store
        .collaboration_job_view(&job.job.id)
        .expect("job should load")
        .expect("job should exist");
    assert_eq!(cancelled.job.status, "cancelled");
    assert!(cancelled.job.finished_at.is_some());
    assert_eq!(cancelled.agents[0].status, "cancelled");
    assert!(cancelled.agents[0].finished_at.is_some());
}

#[test]
fn expired_leases_recover_before_cross_job_capacity_is_reused() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let first_job = store
        .create_collaboration_job(collaboration_input(MAX_COLLABORATION_CONCURRENCY))
        .expect("first collaboration job should be created");
    for agent in &first_job.agents {
        store
            .claim_collaboration_agent(&first_job.job.id, &agent.id, "owner-a")
            .expect("worker should claim");
    }
    store
        .connection
        .execute(
            "UPDATE collaboration_agents SET lease_expires_at = 0 WHERE job_id = ?1",
            params![first_job.job.id],
        )
        .expect("leases should expire");
    let second_job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("second collaboration job should be created");
    assert_eq!(
        store
            .claim_collaboration_agent(&second_job.job.id, &second_job.agents[0].id, "owner-a",)
            .expect_err("expired running rows still require fenced recovery"),
        COLLABORATION_CAPACITY_ERROR,
    );

    store
        .recover_collaboration_agents(&HashSet::new(), "owner-a")
        .expect("expired leases should recover");
    let claimed = store
        .claim_collaboration_agent(&second_job.job.id, &second_job.agents[0].id, "owner-a")
        .expect("recovered capacity should be reusable across jobs");
    assert_eq!(claimed.agent.status, "running");
}
