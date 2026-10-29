use super::support::*;
use super::*;

fn messages(store: &HistoryStore, session: &str) -> Value {
    let raw: String = store
        .connection
        .query_row(
            "SELECT messages FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn seed_pending(store: &HistoryStore, session: &str, turn: &str) {
    seed_thread(store, session);
    store
        .start_agent_run(session, turn, &queued_request(session, "check"))
        .unwrap();
    store
        .update_agent_run_status(session, turn, "running", None)
        .unwrap();
    let saved = serde_json::json!([
        {"role":"user", "content":"check", "turnId":turn},
        {"role":"assistant", "content":"已保存的正文", "turnId":turn,
         "pending":true, "startedAt":1000, "attachments":[{"path":"a.txt"}],
         "events":[{"type":"text_delta", "timestamp_ms":1200,"text":"正文"}]}
    ]);
    store
        .connection
        .execute(
            "UPDATE sessions SET messages = ?1, updated_at = 42 WHERE id = ?2",
            params![saved.to_string(), session],
        )
        .unwrap();
}

#[test]
fn startup_recovery_commits_flags_and_preserves_history_across_reopen() {
    let path = temporary_history_path("startup-repair");
    let expected;
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_pending(&store, "restart", "t");
        expected = messages(&store, "restart");
        let recovered = store.recover_interrupted_runs(&HashSet::new()).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, "interrupted");
        assert_eq!(
            store
                .agent_run("restart", "t")
                .unwrap()
                .unwrap()
                .summary
                .status,
            "interrupted"
        );
        let mut saved = messages(&store, "restart");
        assert_eq!(saved[1]["pending"], false);
        assert!(saved[1]["finishedAt"].as_i64().unwrap() >= 1200);
        saved[1]["pending"] = Value::Bool(true);
        saved[1].as_object_mut().unwrap().remove("finishedAt");
        assert_eq!(saved, expected);
        let updated: i64 = store
            .connection
            .query_row(
                "SELECT updated_at FROM sessions WHERE id = 'restart'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated, 42);
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        let saved = messages(&store, "restart");
        assert_eq!(saved[1]["pending"], false);
        assert!(store
            .recover_interrupted_runs(&HashSet::new())
            .unwrap()
            .is_empty());
        assert_eq!(messages(&store, "restart"), saved);
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn startup_recovery_protects_live_turn_and_clears_orphan_and_interrupted_flags() {
    let store = HistoryStore::open_memory().unwrap();
    seed_pending(&store, "same-session", "live");
    store
        .start_agent_run(
            "same-session",
            "stale",
            &queued_request("same-session", "old"),
        )
        .unwrap();
    store
        .mark_agent_run_interrupted("same-session", "stale")
        .unwrap();
    let mut saved = messages(&store, "same-session");
    saved.as_array_mut().unwrap().extend([
        serde_json::json!({"role":"assistant","turnId":"orphan","pending":true,"content":"orphan text"}),
        serde_json::json!({"role":"assistant","turn_id":"stale","pending":true,"content":"stale text"}),
        serde_json::json!({"role":"assistant","pending":true,"content":"unidentified live text"}),
    ]);
    store
        .connection
        .execute(
            "UPDATE sessions SET messages = ?1 WHERE id = 'same-session'",
            [saved.to_string()],
        )
        .unwrap();
    seed_thread(&store, "orphan-session");
    store.connection.execute(r#"UPDATE sessions SET messages = '[{"role":"assistant","pending":true,"content":"legacy"}]' WHERE id = 'orphan-session'"#, []).unwrap();
    let active = HashSet::from([("same-session".into(), "live".into())]);
    store.recover_interrupted_runs(&active).unwrap();
    let saved = messages(&store, "same-session");
    assert_eq!(saved[1]["pending"], true);
    let by_content = |content: &str| {
        saved
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["content"] == content)
            .unwrap()["pending"]
            .clone()
    };
    assert_eq!(by_content("orphan text"), false);
    assert_eq!(by_content("stale text"), false);
    assert_eq!(by_content("unidentified live text"), true);
    assert_eq!(messages(&store, "orphan-session")[0]["pending"], false);
    assert_eq!(
        store
            .agent_run("same-session", "live")
            .unwrap()
            .unwrap()
            .summary
            .status,
        "running"
    );
}

/// N12-A：终态落库失败（run 停在 running，live 事件里带着
/// `terminal_persistence_failed`）的回合，重启后必须被补偿成真实终态，
/// 而不是被常规 interrupted 判定盖掉。
#[test]
fn startup_recovery_repairs_terminal_that_failed_to_persist() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "n12a");
    store
        .start_agent_run("n12a", "lost", &queued_request("n12a", "do it"))
        .unwrap();
    store
        .update_agent_run_status("n12a", "lost", "running", None)
        .unwrap();
    // 用户看到的终态是「失败」，但只有 live 事件落了库，run 自身没写进终态。
    store
        .connection
        .execute(
            "INSERT INTO session_events(session_id,event_key,timestamp_ms,payload)
             VALUES(?1,?2,?3,?4)",
            params![
                "n12a",
                "terminal-lost",
                1_000,
                serde_json::json!({
                    "type": "error",
                    "session_id": "n12a",
                    "turn_id": "lost",
                    "message": "模型服务异常",
                    "termination_reason": "provider_error",
                    "durable_terminal": false,
                    "terminal_persistence_failed": true,
                })
                .to_string()
            ],
        )
        .unwrap();
    // 另有一个没有标记的正常活回合：不能被动。
    store
        .start_agent_run("n12a", "plain", &queued_request("n12a", "plain"))
        .unwrap();
    store
        .update_agent_run_status("n12a", "plain", "running", None)
        .unwrap();

    let recovered = store.recover_interrupted_runs(&HashSet::new()).unwrap();
    let lost = store.agent_run("n12a", "lost").unwrap().unwrap().summary;
    assert_eq!(lost.status, "failed");
    assert!(lost.finished_at.is_some());
    // 被修好的回合不再出现在 interrupted 回收列表里。
    assert!(!recovered.iter().any(|run| run.turn_id == "lost"));
    assert!(recovered.iter().any(|run| run.turn_id == "plain"));

    // 幂等：再跑一次不会重复补偿、也不会改回 interrupted。
    store.recover_interrupted_runs(&HashSet::new()).unwrap();
    assert_eq!(
        store.agent_run("n12a", "lost").unwrap().unwrap().summary.status,
        "failed"
    );
}

#[test]
fn startup_recovery_rolls_back_run_state_when_message_write_fails() {
    let store = HistoryStore::open_memory().unwrap();
    seed_pending(&store, "rollback", "t");
    let before = messages(&store, "rollback");
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER reject_repair BEFORE UPDATE OF messages ON sessions
         BEGIN SELECT RAISE(FAIL, 'forced write failure'); END;",
        )
        .unwrap();
    assert!(store.recover_interrupted_runs(&HashSet::new()).is_err());
    assert_eq!(
        store
            .agent_run("rollback", "t")
            .unwrap()
            .unwrap()
            .summary
            .status,
        "running"
    );
    assert_eq!(messages(&store, "rollback"), before);
    assert!(store.connection.is_autocommit());
    store
        .connection
        .execute_batch("DROP TRIGGER reject_repair")
        .unwrap();
    assert_eq!(
        store
            .recover_interrupted_runs(&HashSet::new())
            .unwrap()
            .len(),
        1
    );
}
