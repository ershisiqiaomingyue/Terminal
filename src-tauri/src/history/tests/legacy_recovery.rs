use super::support::*;
use super::*;
#[test]
fn interrupted_history_stops_message_timers_without_finishing_the_resumable_run() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "paused");
    let mut request = queued_request("paused", "check configuration");
    request.turn_id = Some("t".into());
    store.start_agent_run("paused", "t", &request).unwrap();
    store.connection.execute("UPDATE agent_runs SET started_at = 1000 WHERE session_id = 'paused'", []).unwrap();
    let messages = serde_json::json!([
        {"id":"assistant:t","role":"assistant","turnId":"t","content":"已读取配置。","pending":true,
         "events":[{"type":"reasoning_delta","text":"检查版本。","timestamp_ms":2000}]},
        {"id":"live","role":"assistant","turnId":"live","content":"","pending":true}
    ]);
    store.connection.execute("UPDATE sessions SET messages = ?1 WHERE id = 'paused'", [messages.to_string()]).unwrap();
    // §8.9：直写列模拟外部状态时清掉行表——复现「未迁移的旧会话」，
    // 读路径以列兜底（生产代码没有绕过 Rust 写路径的列直写）。
    store.connection.execute("DELETE FROM session_messages WHERE session_id = 'paused'", []).unwrap();
    store.mark_agent_run_interrupted("paused", "t").unwrap();
    let record = store.get_record("paused").unwrap().unwrap();
    let paused = record.messages.as_array().unwrap().iter().find(|m| m["id"] == "assistant:t").unwrap();
    assert_eq!(paused["pending"], false);
    assert_eq!(paused["finishedAt"], 2000);
    assert_eq!(paused["content"], "已读取配置。");
    assert_eq!(paused["events"][0]["text"], "检查版本。");
    assert_eq!(record.messages[1]["pending"], true);
    assert!(record.agent_runs[0].finished_at.is_none());
    store.start_agent_run("paused", "t", &request).unwrap();
    assert_eq!(store.agent_run("paused", "t").unwrap().unwrap().summary.resume_count, 1);
}

#[test]
fn deleted_attachment_cleanup_preserves_checkpoint_references_and_survives_retry() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "delete");
    seed_thread(&store, "keep");
    let root = Path::new("C:/cache/agent-attachments");
    let shared = "C:/cache/agent-attachments/shared.md";
    let exclusive = "C:/cache/agent-attachments/exclusive.md";
    store.connection.execute("UPDATE sessions SET artifacts = ?1 WHERE id = 'delete'",
            [serde_json::json!([{"path":shared}, {"path":exclusive}, {"path":"C:/project/original.md"}]).to_string()]).unwrap();
    store
        .connection
        .execute(
            "UPDATE sessions SET artifacts = ?1 WHERE id = 'keep'",
            [serde_json::json!([{"path":shared}]).to_string()],
        )
        .unwrap();
    store.create_checkpoint("keep", None, None, None).unwrap();
    store
        .connection
        .execute("UPDATE sessions SET artifacts = '[]' WHERE id = 'keep'", [])
        .unwrap();
    let (deleted, live) = store
        .attachment_references(root, Some("delete"), None)
        .unwrap();
    assert_eq!(
        deleted,
        HashSet::from(["shared.md".into(), "exclusive.md".into()])
    );
    assert_eq!(live, HashSet::from(["shared.md".into()]));
    store.delete_with_attachments("delete", &deleted).unwrap();
    assert_eq!(
        store.pending_attachment_cleanup(Some("delete")).unwrap(),
        deleted
    );
    assert_eq!(store.pending_session_cleanups().unwrap(), vec!["delete"]);
    let (_, live_after) = store.attachment_references(root, None, None).unwrap();
    assert_eq!(live_after, live);
    store.finish_attachment_cleanup(Some("delete")).unwrap();
    assert!(store.pending_session_cleanups().unwrap().is_empty());
    assert!(store.pending_attachment_cleanup(None).unwrap().is_empty());
}

#[test]
fn failed_deletion_rolls_back_attachment_cleanup_journal() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "delete");
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_delete BEFORE DELETE ON sessions
            BEGIN SELECT RAISE(ABORT, 'injected delete failure'); END;",
        )
        .unwrap();
    assert!(store
        .delete_with_attachments("delete", &HashSet::from(["owned.md".into()]))
        .is_err());
    assert!(store.get_record("delete").unwrap().is_some());
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM attachment_cleanup", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn deleted_database_space_is_reclaimed_without_losing_other_sessions() {
    for legacy in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "android-tool-space-gc-{}-{}-{}.sqlite3",
            std::process::id(),
            legacy,
            now_ms()
        ));
        let store = HistoryStore::open(&path).unwrap();
        if legacy {
            store
                .connection
                .execute_batch("PRAGMA auto_vacuum = NONE; VACUUM;")
                .unwrap();
        }
        seed_thread(&store, "delete");
        seed_thread(&store, "keep");
        store
            .connection
            .execute(
                "UPDATE sessions SET metadata = ?1 WHERE id = 'delete'",
                [serde_json::json!({"padding":"x".repeat(2 * 1024 * 1024)}).to_string()],
            )
            .unwrap();
        // WAL 模式下新写入先进 -wal 文件；先把日志并回主库再量基线，
        // 否则 before 偏小，回收断言在 WAL 库上必然失败。
        store
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        store.delete("delete").unwrap();
        store.reclaim_deleted_space().unwrap();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(after < before, "space-gc legacy={legacy} before={before} after={after}");
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA auto_vacuum", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(store.get_record("keep").unwrap().is_some());
        assert!(store.get_record("delete").unwrap().is_none());
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn terminal_lookup_retains_continuation_provenance_and_is_scoped_to_the_run() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "continued");
    for (index, event) in [
            serde_json::json!({"type":"completed","turn_id":"first","text":"old","termination_reason":"completed"}),
            serde_json::json!({"type":"completed","turn_id":"first","text":"retained partial + continuation",
                "termination_reason":"output_continued","original_termination_reason":"max-tokens",
                "usage":{"inputTokens":100,"outputTokens":30}}),
            serde_json::json!({"type":"completed","turnId":"other","text":"not this run"}),
            serde_json::json!({"type":"status","turn_id":"first","text":"not terminal"}),
        ].into_iter().enumerate() {
            store.connection.execute("INSERT INTO session_events(session_id,event_key,payload) VALUES(?1,?2,?3)",
                params!["continued",format!("event-{index}"),encode(&event).unwrap()]).unwrap();
        }
    let terminal = completed_terminal(&store.connection, "continued", "first")
        .unwrap()
        .unwrap();
    assert_eq!(terminal["termination_reason"], "output_continued");
    assert_eq!(terminal["original_termination_reason"], "max-tokens");
    assert_eq!(terminal["text"], "retained partial + continuation");
    assert_eq!(terminal["usage"]["outputTokens"], 30);
    assert_eq!(
        completed_terminal(&store.connection, "continued", "other")
            .unwrap()
            .unwrap()["text"],
        "not this run"
    );
    assert!(
        completed_terminal(&store.connection, "continued", "missing")
            .unwrap()
            .is_none()
    );
    assert!(
        completed_terminal(&store.connection, "different", "first")
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_steers_keep_frozen_permissions_media_and_retry_identity() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "legacy");
    let mut original = queued_request("legacy", "initial");
    original.permission_mode = Some("ask".into());
    store
        .start_agent_run("legacy", "original-turn", &original)
        .unwrap();
    let steer = AgentSteerRequest {
        message: "additional requirement".into(),
        attachments: vec![agent_core::AgentImageAttachment {
            path: None,
            name: "image.png".into(),
            media_type: "image/png".into(),
            data_url: "data:image/png;base64,aGVsbG8=".into(),
        }],
        artifacts: vec![],
        source_message_id: Some("visible-user".into()),
    };
    let saved = store
        .enqueue_agent_steer("legacy", "original-turn", &steer)
        .unwrap();
    store
        .update_agent_steer_status(&saved.id, "delivered")
        .unwrap();
    let first = legacy_steer_records(&store.connection).unwrap();
    let second = legacy_steer_records(&store.connection).unwrap();
    let request = first[0].2.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        serde_json::to_value(second[0].2.as_ref().unwrap()).unwrap()
    );
    assert_eq!(
        request.turn_id.as_deref(),
        Some(format!("original-turn:steer:{}", saved.id).as_str())
    );
    assert_eq!(request.message, steer.message);
    assert_eq!(request.attachments.len(), 1);
    assert_eq!(
        request.attachments[0].data_url,
        steer.attachments[0].data_url
    );
    assert_eq!(request.model_id, original.model_id);
    assert_eq!(request.permission_mode, original.permission_mode);
    assert_eq!(
        serde_json::to_value(&request.execution_target).unwrap(),
        serde_json::to_value(&original.execution_target).unwrap()
    );
    assert!(request.context.is_empty());
    retire_legacy_steer_record(&store.connection, &saved.id).unwrap();
    retire_legacy_steer_record(&store.connection, &saved.id).unwrap();
    assert!(legacy_steer_records(&store.connection)
        .unwrap()
        .is_empty());
    assert!(retire_legacy_steer_record(&store.connection, "missing").is_err());
}

#[test]
fn legacy_steer_restart_preserves_visible_ids_and_earlier_cutover_receipts() {
    let path = temporary_history_path("legacy-steer-receipt");
    let saved_id;
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "legacy");
        store
            .start_agent_run("legacy", "turn", &queued_request("legacy", "original"))
            .unwrap();
        let steer = AgentSteerRequest {
            message: "update".into(),
            attachments: vec![],
            artifacts: vec![],
            source_message_id: None,
        };
        let saved = store.enqueue_agent_steer("legacy", "turn", &steer).unwrap();
        saved_id = saved.id;
        let request = legacy_steer_records(&store.connection)
            .unwrap()
            .remove(0)
            .2
            .unwrap();
        assert_eq!(
            format!("user:{}", request.turn_id.unwrap()),
            format!("user:turn:steer:{saved_id}")
        );
        let old_id = format!("user:legacy-steer-{saved_id}");
        put_turn_outbox_record(
            &store.connection,
            "legacy",
            "steer",
            &serde_json::json!({"id":old_id, "content":[{"type":"text","text":"update"}]}),
        )
        .unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        let request = legacy_steer_records(&store.connection)
            .unwrap()
            .remove(0)
            .2
            .unwrap();
        assert_eq!(
            request.turn_id.as_deref(),
            Some(format!("legacy-steer-{saved_id}").as_str())
        );
        ack_turn_outbox_record(
            &store.connection,
            "legacy",
            &format!("user:legacy-steer-{saved_id}"),
        )
        .unwrap();
        assert_eq!(
            legacy_steer_records(&store.connection)
                .unwrap()
                .remove(0)
                .2
                .unwrap()
                .turn_id,
            request.turn_id
        );
        retire_legacy_steer_record(&store.connection, &saved_id).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        assert!(legacy_steer_records(&store.connection)
            .unwrap()
            .is_empty());
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn legacy_steer_parse_errors_are_isolated_and_terminal_turns_are_not_replayed() {
    let store = HistoryStore::open_memory().unwrap();
    let steer = AgentSteerRequest {
        message: "update".into(),
        attachments: vec![],
        artifacts: vec![],
        source_message_id: None,
    };
    for session in ["bad", "good", "terminal", "missing"] {
        seed_thread(&store, session);
        if session != "missing" {
            store
                .start_agent_run(session, "turn", &queued_request(session, "original"))
                .unwrap();
        }
        let saved = store.enqueue_agent_steer(session, "turn", &steer).unwrap();
        if session == "bad" {
            store
                .connection
                .execute(
                    "UPDATE agent_steers SET request = 'broken' WHERE id = ?1",
                    [saved.id],
                )
                .unwrap();
        }
        if session == "terminal" {
            store
                .finish_agent_run(session, "turn", "completed", None)
                .unwrap();
        }
    }
    let records = legacy_steer_records(&store.connection).unwrap();
    assert_eq!(records.len(), 3);
    assert!(records
        .iter()
        .find(|row| row.0 == "bad")
        .unwrap()
        .2
        .is_err());
    assert!(records
        .iter()
        .find(|row| row.0 == "missing")
        .unwrap()
        .2
        .is_err());
    assert!(records
        .iter()
        .find(|row| row.0 == "good")
        .unwrap()
        .2
        .is_ok());
    assert!(!records.iter().any(|row| row.0 == "terminal"));
}

#[test]
fn legacy_seed_excludes_only_identified_unapplied_user_messages() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "legacy");
    store
        .start_agent_run("legacy", "turn", &queued_request("legacy", "original"))
        .unwrap();
    let steer = AgentSteerRequest {
        message: "same text".into(),
        attachments: vec![],
        artifacts: vec![],
        source_message_id: Some("pending".into()),
    };
    let saved = store.enqueue_agent_steer("legacy", "turn", &steer).unwrap();
    let message = |role: &str, id: &str| AgentContextTurn {
        role: role.into(),
        content: "same text".into(),
        message_id: Some(id.into()),
        source: None,
        turn_id: None,
        model_id: None,
        attachments: vec![],
        artifacts: vec![],
    };
    let source_id = format!("user:turn:steer:{}", saved.id);
    let original = vec![
        message("user", &source_id),
        message("user", "different"),
        message("assistant", &source_id),
    ];
    let mut filtered = original.clone();
    filter_pending_legacy_steer_context(&store.connection, "legacy", &mut filtered).unwrap();
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].message_id.as_deref(), Some("different"));
    assert_eq!(filtered[1].role, "assistant");
    store
        .update_agent_steer_status(&saved.id, "applied")
        .unwrap();
    let mut applied = original.clone();
    filter_pending_legacy_steer_context(&store.connection, "legacy", &mut applied).unwrap();
    assert_eq!(applied.len(), 3);
    store.enqueue_agent_steer("legacy", "turn", &steer).unwrap();
    store
        .finish_agent_run("legacy", "turn", "cancelled", None)
        .unwrap();
    let mut terminal = original;
    filter_pending_legacy_steer_context(&store.connection, "legacy", &mut terminal).unwrap();
    assert_eq!(terminal.len(), 3);
}

#[test]
fn legacy_queue_recovery_isolates_corrupt_rows_and_preserves_fifo_and_leases() {
    let store = HistoryStore::open_memory().unwrap();
    for (session, id) in [
        ("bad", "corrupt"),
        ("bad", "later"),
        ("good", "good"),
        ("mismatch", "wrong-owner"),
    ] {
        store
            .enqueue_agent_queue_item(session, id, id, &queued_request(session, id))
            .unwrap();
    }
    store
        .claim_agent_queue_item("bad", Some("corrupt"), "old-process")
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE agent_queue SET request = 'broken' WHERE id = 'corrupt'",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE agent_queue SET request = ?1 WHERE id = 'wrong-owner'",
            [serde_json::to_string(&queued_request("other", "wrong-owner")).unwrap()],
        )
        .unwrap();
    let rows = store.recover_legacy_queue_items("new-process").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].0, "bad");
    assert!(rows[0].1.as_ref().unwrap_err().contains("corrupt"));
    assert_eq!(rows[1].1.as_ref().unwrap().id, "later");
    assert_eq!(rows[2].1.as_ref().unwrap().id, "good");
    assert!(rows[3].1.as_ref().unwrap_err().contains("属于另一会话"));
    let lease: (String, Option<String>) = store
        .connection
        .query_row(
            "SELECT status, lease_owner FROM agent_queue WHERE id = 'corrupt'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(lease, ("pending".into(), None));
    assert_eq!(
        store
            .recover_legacy_queue_items("new-process")
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn checkpoint_rewind_restores_display_and_installs_exact_original_cut() {
    let path = temporary_history_path("checkpoint-rewind");
    let cut = CheckpointCut {
        storage_id: "original-log".into(),
        event_count: 17,
    };
    let checkpoint_id;
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "thread");
        store.install_session_binding("thread", None).unwrap();
        let mut record = store.get_record("thread").unwrap().unwrap();
        record.messages = serde_json::json!([{"role":"user","content":"before"}]);
        store.upsert_record(&record).unwrap();
        checkpoint_id = store
            .create_checkpoint("thread", None, None, Some(cut.clone()))
            .unwrap()
            .id;
    }
    let store = HistoryStore::open(&path).unwrap();
    assert_eq!(
        store.checkpoint(&checkpoint_id).unwrap().unwrap().checkpoint_cut,
        Some(cut.clone())
    );
    let before_binding = store.session_binding("thread").unwrap().unwrap();
    let mut record = store.get_record("thread").unwrap().unwrap();
    record.messages = serde_json::json!([{"role":"user","content":"after"}]);
    record.updated_at += 1;
    store.upsert_record(&record).unwrap();
    put_turn_outbox_record(
        &store.connection,
        "thread",
        "followup",
        &serde_json::json!({"id":"pending"}),
    )
    .unwrap();
    let restored = store.restore_checkpoint("thread", &checkpoint_id).unwrap();
    assert_eq!(restored.messages[0]["content"], "before");
    let next = store.session_binding("thread").unwrap().unwrap();
    assert_ne!(next.storage_id, before_binding.storage_id);
    assert_eq!(next.seed_from, Some(cut));
    assert!(!next.deleted);
    // Even a late snapshot with a newer wall clock cannot restore the old generation.
    record.updated_at = restored.updated_at + 1000;
    assert!(store.upsert_record(&record).is_err());
    assert_eq!(restored.metadata["_storageBindingId"], next.storage_id);
    store.upsert_record(&restored).unwrap();
    assert_eq!(
        store.get_record("thread").unwrap().unwrap().messages[0]["content"],
        "before"
    );
    assert!(turn_recovery_requests(&store.connection).unwrap().is_empty());
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM turn_outbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn fork_preserves_checkpoint_or_current_lineage_and_refuses_missing_cuts() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "parent");
    store.install_session_binding("parent", None).unwrap();
    let cut = CheckpointCut {
        storage_id: "parent-log".into(),
        event_count: 8,
    };
    let checkpoint = store
        .create_checkpoint("parent", None, None, Some(cut.clone()))
        .unwrap();
    let current = CheckpointCut {
        event_count: 20,
        ..cut.clone()
    };
    let child = store
        .fork_thread(
            "parent",
            Some(checkpoint.id.clone()),
            None,
            Some(current.clone()),
        )
        .unwrap();
    assert_eq!(
        store.session_binding(&child.id).unwrap().unwrap().seed_from,
        Some(cut)
    );
    assert_eq!(
        store
            .conversation_thread(&child.id)
            .unwrap()
            .unwrap()
            .parent_thread_id
            .as_deref(),
        Some("parent")
    );
    let latest = store
        .fork_thread("parent", None, None, Some(current.clone()))
        .unwrap();
    assert_eq!(
        store.session_binding(&latest.id).unwrap().unwrap().seed_from,
        Some(current)
    );
    assert!(store
        .fork_thread("parent", Some("missing".into()), None, None)
        .is_err());
    let legacy = store.create_checkpoint("parent", None, None, None).unwrap();
    assert!(store.restore_checkpoint("parent", &legacy.id).is_err());
    assert!(store
        .fork_thread("parent", Some(legacy.id), None, None)
        .is_err());
    assert!(store.fork_thread("parent", None, None, None).is_err());
}

#[test]
fn history_generation_changes_roll_back_with_display_or_lineage_failure() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "parent");
    store.install_session_binding("parent", None).unwrap();
    let cut = CheckpointCut {
        storage_id: "original".into(),
        event_count: 10,
    };
    let checkpoint = store
        .create_checkpoint("parent", None, None, Some(cut.clone()))
        .unwrap();
    let original = store.session_binding("parent").unwrap().unwrap().storage_id;
    put_turn_outbox_record(
        &store.connection,
        "parent",
        "followup",
        &serde_json::json!({"id":"pending"}),
    )
    .unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_restore BEFORE UPDATE ON sessions
            BEGIN SELECT RAISE(ABORT,'injected restore failure'); END;",
        )
        .unwrap();
    assert!(store.restore_checkpoint("parent", &checkpoint.id).is_err());
    assert_eq!(
        store.session_binding("parent").unwrap().unwrap().storage_id,
        original
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM turn_outbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    store.connection.execute_batch("DROP TRIGGER fail_restore;
            CREATE TRIGGER fail_fork BEFORE INSERT ON conversation_threads WHEN NEW.thread_id != 'parent'
            BEGIN SELECT RAISE(ABORT,'injected fork failure'); END;").unwrap();
    assert!(store.fork_thread("parent", None, None, Some(cut)).is_err());
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM session_bindings", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(store.connection.is_autocommit());
}

#[test]
fn deleted_history_cannot_be_resurrected_by_late_saves_events_or_input() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "thread");
    let record = store.get_record("thread").unwrap().unwrap();
    let input = serde_json::json!({"id":"pending"});
    put_turn_outbox_record(&store.connection, "thread", "followup", &input).unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_delete BEFORE DELETE ON sessions
            BEGIN SELECT RAISE(ABORT,'injected delete failure'); END;",
        )
        .unwrap();
    assert!(store.delete("thread").is_err());
    assert!(!store.session_binding("thread").unwrap().unwrap().deleted);
    assert!(store.get_record("thread").unwrap().is_some());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_delete")
        .unwrap();
    store.delete("thread").unwrap();
    assert!(store.session_binding("thread").unwrap().unwrap().deleted);
    assert!(store.upsert_record(&record).is_err());
    assert!(store
        .append_events(
            "thread",
            now_ms(),
            &[serde_json::json!({"type":"status","event_id":"late"})]
        )
        .is_err());
    assert!(put_turn_outbox_record(&store.connection, "thread", "followup", &input).is_err());
    assert!(store.install_session_binding("thread", None).is_err());
    assert!(store.ensure_conversation_thread("thread").is_err());
    assert!(store.get_record("thread").unwrap().is_none());
    assert!(turn_recovery_requests(&store.connection).unwrap().is_empty());
    store.delete("thread").unwrap();
}

#[test]
fn turn_recovery_clear_is_fenced_by_the_latest_accepted_input() {
    let store = HistoryStore::open_memory().unwrap();
    let connection = &store.connection;
    let message =
        |id| serde_json::json!({"id":id,"nativeRequest":queued_request("session","test")});
    put_turn_outbox_record(connection, "session", "followup", &message("first")).unwrap();
    assert_eq!(turn_recovery_requests(connection).unwrap().len(), 1);
    settle_turn_recovery_record(connection, "session", "first").unwrap();
    assert_eq!(turn_recovery_requests(connection).unwrap().len(), 1);
    connection
        .execute("UPDATE turn_outbox SET accepted = 1", [])
        .unwrap();
    settle_turn_recovery_record(connection, "session", "first").unwrap();
    assert!(turn_recovery_requests(connection).unwrap().is_empty());
    // A duplicate terminal lookup does not make a completed history eager again.
    put_turn_outbox_record(connection, "session", "followup", &message("first")).unwrap();
    assert!(turn_recovery_requests(connection).unwrap().is_empty());
    put_turn_outbox_record(connection, "session", "followup", &message("second")).unwrap();
    connection
        .execute("UPDATE turn_outbox SET accepted = 1", [])
        .unwrap();
    settle_turn_recovery_record(connection, "session", "first").unwrap();
    assert_eq!(turn_recovery_requests(connection).unwrap().len(), 1);
    settle_turn_recovery_record(connection, "session", "second").unwrap();
    assert!(turn_recovery_requests(connection).unwrap().is_empty());
}

#[test]
fn turn_recovery_invalid_request_does_not_hide_other_sessions() {
    let store = HistoryStore::open_memory().unwrap();
    put_turn_outbox_record(
        &store.connection,
        "bad",
        "followup",
        &serde_json::json!({"id":"bad"}),
    )
    .unwrap();
    put_turn_outbox_record(
        &store.connection,
        "good",
        "followup",
        &serde_json::json!({"id":"good","nativeRequest":queued_request("good","test")}),
    )
    .unwrap();
    let requests = turn_recovery_requests(&store.connection).unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].1.is_err());
    assert_eq!(
        requests[1].1.as_ref().unwrap().session_id.as_deref(),
        Some("good")
    );
}

#[test]
fn turn_recovery_handles_idle_before_ack_and_rejects_stale_idle_after_new_input() {
    let store = HistoryStore::open_memory().unwrap();
    let connection = &store.connection;
    let message =
        |id| serde_json::json!({"id":id,"nativeRequest":queued_request("session","test")});
    put_turn_outbox_record(connection, "session", "followup", &message("first")).unwrap();
    settle_turn_recovery_record(connection, "session", "first").unwrap();
    ack_turn_outbox_record(connection, "session", "first").unwrap();
    assert!(turn_recovery_requests(connection).unwrap().is_empty());
    put_turn_outbox_record(connection, "session", "followup", &message("second")).unwrap();
    settle_turn_recovery_record(connection, "session", "first").unwrap();
    ack_turn_outbox_record(connection, "session", "second").unwrap();
    assert_eq!(turn_recovery_requests(connection).unwrap().len(), 1);
    settle_turn_recovery_record(connection, "session", "second").unwrap();
    assert!(turn_recovery_requests(connection).unwrap().is_empty());
}

#[test]
fn turn_recovery_marker_survives_reopen_without_reactivating_settled_sessions() {
    let path = temporary_history_path("recovery-marker");
    {
        let store = HistoryStore::open(&path).unwrap();
        put_turn_outbox_record(
            &store.connection,
            "session",
            "followup",
            &serde_json::json!({"id":"first","nativeRequest":queued_request("session","test")}),
        )
        .unwrap();
        store
            .connection
            .execute("UPDATE turn_outbox SET accepted = 1", [])
            .unwrap();
        settle_turn_recovery_record(&store.connection, "session", "first").unwrap();
    }
    let store = HistoryStore::open(&path).unwrap();
    assert!(turn_recovery_requests(&store.connection).unwrap().is_empty());
}

#[test]
fn turn_recovery_bootstrap_covers_older_outbox_only_once() {
    let store = HistoryStore::open_memory().unwrap();
    put_turn_outbox_record(
        &store.connection,
        "old",
        "followup",
        &serde_json::json!({"id":"old","nativeRequest":queued_request("old","test")}),
    )
    .unwrap();
    store
        .connection
        .execute_batch("DROP TABLE turn_recovery")
        .unwrap();
    initialize_turn_recovery(&store.connection).unwrap();
    assert_eq!(turn_recovery_requests(&store.connection).unwrap().len(), 1);
    ack_turn_outbox_record(&store.connection, "old", "old").unwrap();
    settle_turn_recovery_record(&store.connection, "old", "old").unwrap();
    initialize_turn_recovery(&store.connection).unwrap();
    assert!(turn_recovery_requests(&store.connection).unwrap().is_empty());
}

#[test]
fn turn_recovery_marker_and_outbox_ack_fail_atomically() {
    let store = HistoryStore::open_memory().unwrap();
    let connection = &store.connection;
    let message = serde_json::json!({"id":"test","nativeRequest":queued_request("session","test")});
    connection
        .execute_batch(
            "CREATE TRIGGER reject_marker BEFORE INSERT ON turn_recovery
            BEGIN SELECT RAISE(ABORT,'injected marker failure'); END;",
        )
        .unwrap();
    assert!(put_turn_outbox_record(connection, "session", "followup", &message).is_err());
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM turn_outbox", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    connection
        .execute_batch("DROP TRIGGER reject_marker")
        .unwrap();
    put_turn_outbox_record(connection, "session", "followup", &message).unwrap();
    settle_turn_recovery_record(connection, "session", "test").unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_clear BEFORE UPDATE OF needs_recovery ON turn_recovery
            BEGIN SELECT RAISE(ABORT,'injected clear failure'); END;",
        )
        .unwrap();
    assert!(ack_turn_outbox_record(connection, "session", "test").is_err());
    let accepted: bool = connection
        .query_row("SELECT accepted FROM turn_outbox", [], |row| row.get(0))
        .unwrap();
    assert!(!accepted);
    assert_eq!(turn_recovery_requests(connection).unwrap().len(), 1);
}

#[test]
fn turn_outbox_identity_and_revised_receipts_prevent_duplicate_recovery() {
    let store = HistoryStore::open_memory().unwrap();
    let original = serde_json::json!({"id":"message","content":[{"type":"text","text":"old"}],"nativeRequest":{"modelId":"model"}});
    put_turn_outbox_record(&store.connection, "session", "followup", &original).unwrap();
    put_turn_outbox_record(&store.connection, "session", "followup", &original).unwrap();
    let initial: (i64, i64) = store
        .connection
        .query_row("SELECT sequence,accepted FROM turn_outbox", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(initial.1, 0);
    let mut edited = original.clone();
    edited["content"][0]["text"] = serde_json::json!("new");
    assert!(put_turn_outbox_record(&store.connection, "session", "followup", &edited).is_err());
    revise_turn_outbox_record(&store.connection, "session", "message", "steer", &edited).unwrap();
    let revised: (i64, i64, String) = store
        .connection
        .query_row("SELECT sequence,accepted,mode FROM turn_outbox", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(revised, (initial.0, 1, "steer".into()));
    assert!(
        revise_turn_outbox_record(&store.connection, "session", "other", "removed", &edited)
            .is_err()
    );
    assert!(revise_turn_outbox_record(
        &store.connection,
        "missing-session",
        "message",
        "removed",
        &edited
    )
    .is_err());
    for mode in ["removed", "rejected"] {
        revise_turn_outbox_record(&store.connection, "session", "message", mode, &edited).unwrap();
        let pending: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM turn_outbox WHERE accepted=0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }
    let count: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM turn_outbox", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
