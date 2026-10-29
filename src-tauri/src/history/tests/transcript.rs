use super::support::*;
use super::*;

#[test]
fn restored_user_directories_remain_local_sources_even_without_message_text() {
    for field in ["artifacts", "attachments"] {
        let mut user = serde_json::json!({"id":"user:t", "turnId":"t", "role":"user", "content":""});
        user[field] = serde_json::json!([{"kind":"directory", "name":"out", "path":"D:/CarAdapter/out", "content":"do not duplicate"}]);
        let turn = raw_agent_context_turn(&user).unwrap();
        assert_eq!(turn.artifacts.len(), 1);
        assert!(turn.artifacts[0].content.is_none());
        let message = agent_core::context_turn_message(&turn).unwrap();
        assert!(message.content.contains("D:/CarAdapter/out"));
        assert!(message.content.contains("\"environment\":\"windows\""));
        assert!(!message.content.contains("do not duplicate"));
        let serialized = serde_json::to_value(&turn).unwrap();
        assert_eq!(serialized["artifacts"][0]["path"], "D:/CarAdapter/out");
        user["role"] = serde_json::json!("assistant");
        assert!(raw_agent_context_turn(&user).is_none());
    }
}

#[test]
fn message_edit_rewinds_same_thread_and_rejects_stale_history_saves() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "edit");
    let mut original = store.get_record("edit").unwrap().unwrap();
    original.messages = serde_json::json!([
        {"id":"u1","role":"user","content":"keep question","createdAt":10,"turnId":"t1"},
        {"id":"a1","role":"assistant","content":"keep answer","createdAt":11,"turnId":"t1"},
        {"id":"u2","role":"user","content":"edit question","createdAt":20,"turnId":"t2"},
        {"id":"a2","role":"assistant","content":"old answer","createdAt":21,"turnId":"t2"},
        {"id":"u3","role":"user","content":"later question","createdAt":30,"turnId":"t3"}
    ]);
    original.metadata =
        serde_json::json!({"contextCheckpoint":{"summary":"old answer"},"scope":"workspace"});
    store.upsert_record(&original).unwrap();
    for (index, turn) in ["t1", "t2", "t3"].iter().enumerate() {
        store
            .start_agent_run("edit", turn, &queued_request("edit", "question"))
            .unwrap();
        store
            .finish_agent_run("edit", turn, "completed", None)
            .unwrap();
        store.append_events("edit", 40, &[serde_json::json!({"type":"completed","event_id":format!("e{index}"),"turn_id":turn,"text":format!("answer {turn}"),"timestamp_ms":index as i64 * 10 + 10})]).unwrap();
    }
    let edited = store
        .rewind_before_message("edit", "u2", Some("t2"))
        .unwrap();
    assert_eq!(edited.id, "edit");
    assert_eq!(edited.title, original.title);
    assert_eq!(edited.messages.as_array().unwrap().len(), 2);
    assert!(edited.metadata.get("contextCheckpoint").is_none());
    assert_eq!(edited.metadata["scope"], "workspace");
    assert_eq!(edited.agent_runs.len(), 1);
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
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id = 'edit'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let context = serde_json::to_string(&raw_agent_context_from_record(&edited)).unwrap();
    assert!(!context.contains("old answer"));
    assert!(!context.contains("edit question"));
    assert!(!context.contains("later question"));
    assert!(store.upsert_record(&original).is_err());
    store.upsert_record(&edited).unwrap();
    let first_edit = store
        .rewind_before_message("edit", "projected-user-id", Some("t1"))
        .unwrap();
    assert_eq!(first_edit.messages, serde_json::json!([]));
    assert!(raw_agent_context_from_record(&first_edit).is_empty());
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn message_edit_missing_target_preserves_history() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "edit");
    let before = store.get_record("edit").unwrap().unwrap();
    assert!(store
        .rewind_before_message("edit", "missing", None)
        .is_err());
    assert_eq!(store.get_record("edit").unwrap().unwrap(), before);
    assert!(store.session_binding("edit").unwrap().is_none());
}

#[test]
fn message_edit_of_steer_discards_shared_turn_events_but_keeps_earlier_text() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "edit");
    let mut original = store.get_record("edit").unwrap().unwrap();
    original.messages = serde_json::json!([
        {"id":"u1","role":"user","content":"initial question","turnId":"t"},
        {"id":"a1","role":"assistant","content":"earlier answer","turnId":"t","events":[{"type":"completed","text":"later answer"}]},
        {"id":"steer","role":"user","content":"edit steer","createdAt":20,"turnId":"t"},
        {"id":"a2","role":"assistant","content":"later answer","turnId":"t"}
    ]);
    store.upsert_record(&original).unwrap();
    store.append_events("edit", 30, &[serde_json::json!({"type":"completed","turn_id":"t","text":"later answer","timestamp_ms":30})]).unwrap();
    let edited = store
        .rewind_before_message("edit", "steer", Some("t"))
        .unwrap();
    assert_eq!(edited.messages.as_array().unwrap().len(), 2);
    assert!(edited.messages[1].get("events").is_none());
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let context = serde_json::to_string(&raw_agent_context_from_record(&edited)).unwrap();
    assert!(context.contains("earlier answer"));
    assert!(!context.contains("later answer"));
    assert!(!context.contains("edit steer"));
}

#[test]
fn session_usage_totals_normalize_protocols_deduplicate_attempts_and_ignore_restore_limit() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "usage");
    let mut events = vec![
        serde_json::json!({"type":"completed","turn_id":"a","execution_started_at":10,"durable_terminal":true,"timestamp_ms":20,"event_id":"a1","usage":{"providerKind":"anthropic","inputTokens":100,"cachedInputTokens":800,"cacheCreationInputTokens":100,"outputTokens":20}}),
        serde_json::json!({"type":"completed","turn_id":"a","execution_started_at":10,"durable_terminal":true,"timestamp_ms":21,"event_id":"a2","usage":{"providerKind":"anthropic","inputTokens":100,"cachedInputTokens":800,"cacheCreationInputTokens":100,"outputTokens":30}}),
        serde_json::json!({"type":"completed","turn_id":"a","execution_started_at":30,"durable_terminal":true,"timestamp_ms":40,"event_id":"resume","usage":{"providerKind":"open_ai_responses","inputTokens":2000,"cachedInputTokens":1000,"outputTokens":40,"reasoningTokens":12}}),
        serde_json::json!({"type":"cancelled","turn_id":"b","execution_started_at":50,"durable_terminal":true,"timestamp_ms":60,"event_id":"cancel","usage":{"providerKind":"open_ai_responses","inputTokens":300,"outputTokens":50,"reasoning_tokens":3}}),
        serde_json::json!({"type":"error","turn_id":"unknown","durable_terminal":true,"timestamp_ms":70,"event_id":"unknown"}),
    ];
    events.extend((0..1100).map(|index| serde_json::json!({"type":"status","event_id":format!("noise-{index}"),"timestamp_ms":100+index,"message":"transient"})));
    // Imported histories may already contain duplicate terminals or resumed executions.
    for (index, event) in events.iter().enumerate() {
        store.connection.execute(
                "INSERT INTO session_events(session_id,event_key,timestamp_ms,payload) VALUES(?1,?2,?3,?4)",
                params!["usage", format!("import-{index}"), event["timestamp_ms"].as_i64(), encode(event).unwrap()],
            ).unwrap();
    }
    let record = store.get_record("usage").unwrap().unwrap();
    assert!(
        record.events.as_ref().unwrap().as_array().unwrap().len() <= MAX_RESTORED_SESSION_EVENTS
    );
    let total = record.usage_totals.unwrap();
    assert_eq!(total.input_tokens, Some(3300));
    assert_eq!(total.output_tokens, Some(120));
    // Reported reasoning accumulates across turns, snake_case and camelCase alike;
    // turns that never reported it stay absent rather than counting as 0.
    assert_eq!(total.reasoning_tokens, Some(15));
    assert_eq!(
        total
            .executions
            .iter()
            .filter(|execution| execution.reasoning_tokens.is_none())
            .count(),
        2
    );
    assert_eq!(total.executions.len(), 4);
    assert_eq!(
        total
            .executions
            .iter()
            .filter(|execution| execution.input_tokens.is_none())
            .count(),
        1
    );
    assert_eq!(
        store
            .get_record("usage")
            .unwrap()
            .unwrap()
            .usage_totals
            .unwrap(),
        total
    );
    assert_eq!(
        history_usage_tokens(
            &serde_json::json!({"providerKind":"anthropic","cachedInputTokens":500})
        )
        .0,
        None
    );
    // The reasoning counter is read back through both spellings.
    assert_eq!(
        history_usage_tokens(&serde_json::json!({"reasoningTokens":9})).2,
        Some(9)
    );
    assert_eq!(
        history_usage_tokens(&serde_json::json!({"reasoning_tokens":4})).2,
        Some(4)
    );
    assert_eq!(
        history_usage_tokens(&serde_json::json!({"outputTokens":50})).2,
        None
    );
}

#[test]
fn usage_restoration_is_independent_of_timeline_limits_and_empty_webview_saves() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "usage-restore");
    let terminal = serde_json::json!({"type":"completed","turn_id":"a","execution_started_at":10,
            "durable_terminal":true,"timestamp_ms":20,"event_id":"a1",
            "usage":{"providerKind":"open_ai_responses","inputTokens":13188,"outputTokens":494,
                     "cachedInputTokens":12928,"totalCost":0.03,"currency":"CNY"}});
    store.connection.execute("INSERT INTO session_events(session_id,event_key,timestamp_ms,payload) VALUES(?1,?2,?3,?4)",
            params!["usage-restore", "terminal", 20, encode(&terminal).unwrap()]).unwrap();
    for index in 0..1100 {
        store.connection.execute("INSERT INTO session_events(session_id,event_key,timestamp_ms,payload) VALUES(?1,?2,?3,?4)",
                params!["usage-restore",format!("noise-{index}"),100+index,encode(&serde_json::json!({"type":"status"})).unwrap()]).unwrap();
    }
    let mut record = store.get_record("usage-restore").unwrap().unwrap();
    assert!(record
        .events
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["type"] != "completed"));
    assert_eq!(record.usage.as_ref().unwrap()["inputTokens"], 13188);
    assert_eq!(record.usage.as_ref().unwrap()["totalCost"], 0.03);
    assert_eq!(record.metadata["usageTurnId"], "a");
    record.updated_at = 2000;
    record.usage = None;
    store.upsert_record(&record).unwrap();
    assert_eq!(
        store
            .get_record("usage-restore")
            .unwrap()
            .unwrap()
            .usage
            .unwrap()["outputTokens"],
        494
    );
    let unknown = serde_json::json!({"type":"cancelled","turn_id":"b","durable_terminal":true,
            "timestamp_ms":3000,"usage":{"inputTokens":null,"outputTokens":null}});
    store.connection.execute("INSERT INTO session_events(session_id,event_key,timestamp_ms,payload) VALUES(?1,?2,?3,?4)",
            params!["usage-restore","unknown",3000,encode(&unknown).unwrap()]).unwrap();
    let record = store.get_record("usage-restore").unwrap().unwrap();
    assert!(record.usage.is_none());
    assert_eq!(record.metadata["usageTurnId"], "b");
    assert_eq!(record.usage_totals.unwrap().input_tokens, Some(13188));
}

#[test]
fn usage_totals_accept_legacy_terminals_and_keep_known_fields_on_duplicate_empty_reports() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "usage-legacy");
    let events = [
        serde_json::json!({"type":"completed","turnId":"a","sequence":1,"eventId":"legacy","timestamp_ms":1,
                "usage":{"inputTokens":100,"outputTokens":20}}),
        serde_json::json!({"type":"completed","turn_id":"a","durable_terminal":true,"timestamp_ms":2,
                "usage":{"inputTokens":null,"outputTokens":30}}),
        serde_json::json!({"type":"completed","turn_id":"b","durable_terminal":false,"timestamp_ms":3,
                "usage":{"inputTokens":999}}),
        serde_json::json!({"type":"error","turn_id":"c","durable_terminal":true,"terminal_persistence_failed":true,"timestamp_ms":4,
                "usage":{"inputTokens":999}}),
    ];
    for (index, event) in events.iter().enumerate() {
        store.connection.execute("INSERT INTO session_events(session_id,event_key,timestamp_ms,payload) VALUES(?1,?2,?3,?4)",
                params!["usage-legacy",format!("event-{index}"),index as i64,encode(event).unwrap()]).unwrap();
    }
    let totals = store.session_usage_totals("usage-legacy").unwrap();
    assert_eq!(totals.input_tokens, Some(100));
    assert_eq!(totals.output_tokens, Some(30));
    assert_eq!(totals.executions.len(), 1);
}

#[test]
fn usage_restoration_merges_normalized_saved_fields_with_native_report_aliases() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "usage-alias");
    let mut record = store.get_record("usage-alias").unwrap().unwrap();
    record.usage = Some(serde_json::json!({"input":100,"output":20,"cacheRead":50,"totalCost":0.01,"reasoning_tokens":7}));
    record.metadata["usageTurnId"] = Value::String("a".into());
    store.upsert_record(&record).unwrap();
    let event = serde_json::json!({"type":"completed","turn_id":"a","durable_terminal":true,
            "usage":{"inputTokens":120,"outputTokens":null,"cachedInputTokens":60}});
    store
        .connection
        .execute(
            "INSERT INTO session_events(session_id,event_key,payload) VALUES(?1,?2,?3)",
            params!["usage-alias", "terminal", encode(&event).unwrap()],
        )
        .unwrap();
    let usage = store
        .get_record("usage-alias")
        .unwrap()
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(usage["inputTokens"], 120);
    assert!(usage.get("input").is_none());
    assert_eq!(usage["outputTokens"], 20);
    assert_eq!(usage["cachedInputTokens"], 60);
    assert_eq!(usage["totalCost"], 0.01);
    // A restored usage that only had the snake_case spelling normalizes once and
    // survives a terminal report that does not repeat the counter.
    assert_eq!(usage["reasoningTokens"], 7);
    assert!(usage.get("reasoning_tokens").is_none());
}

#[test]
fn usage_reasoning_tokens_persist_restore_and_never_default_to_zero() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "usage-reasoning");
    let mut record = store.get_record("usage-reasoning").unwrap().unwrap();
    record.usage = Some(serde_json::json!({"providerKind":"open_ai_responses","inputTokens":1000,"outputTokens":200,"reasoningTokens":75}));
    record.metadata["usageTurnId"] = Value::String("a".into());
    store.upsert_record(&record).unwrap();
    let reported = serde_json::json!({"type":"completed","turn_id":"a","durable_terminal":true,
            "usage":{"providerKind":"open_ai_responses","inputTokens":1000,"outputTokens":200,"reasoning_tokens":75}});
    store
        .connection
        .execute(
            "INSERT INTO session_events(session_id,event_key,payload) VALUES(?1,?2,?3)",
            params!["usage-reasoning", "terminal", encode(&reported).unwrap()],
        )
        .unwrap();
    let restored = store.get_record("usage-reasoning").unwrap().unwrap();
    let usage = restored.usage.expect("restored usage");
    // The provider's number survives restore under one canonical spelling.
    assert_eq!(usage["reasoningTokens"], 75);
    assert!(usage.get("reasoning_tokens").is_none());
    assert_eq!(restored.usage_totals.unwrap().reasoning_tokens, Some(75));

    // A session whose provider never reported reasoning stores nothing: no key, no zero.
    seed_thread(&store, "usage-reasoning-absent");
    let mut plain = store
        .get_record("usage-reasoning-absent")
        .unwrap()
        .unwrap();
    plain.usage =
        Some(serde_json::json!({"providerKind":"anthropic","inputTokens":10,"outputTokens":2}));
    plain.metadata["usageTurnId"] = Value::String("a".into());
    store.upsert_record(&plain).unwrap();
    let absent = store
        .get_record("usage-reasoning-absent")
        .unwrap()
        .unwrap();
    assert!(absent.usage.unwrap().get("reasoningTokens").is_none());
    assert!(absent
        .usage_totals
        .map_or(true, |total| total.reasoning_tokens.is_none()));
}

#[test]
fn migrates_legacy_schema_without_changing_session_data() {
    let connection = Connection::open_in_memory().expect("legacy database should open");
    connection
        .execute_batch(
            r#"CREATE TABLE sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    title TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0,
                    jira_context TEXT,
                    artifacts TEXT NOT NULL,
                    messages TEXT NOT NULL,
                    metadata TEXT NOT NULL,
                    report TEXT,
                    usage TEXT
                );
                CREATE TABLE collaboration_jobs (
                    id TEXT PRIMARY KEY NOT NULL,
                    thread_id TEXT NOT NULL,
                    parent_turn_id TEXT,
                    goal TEXT NOT NULL,
                    workspace_path TEXT,
                    permission_mode TEXT NOT NULL,
                    execution_target TEXT,
                    agent_config TEXT NOT NULL DEFAULT '{}',
                    aggregate_summary TEXT,
                    status TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    finished_at INTEGER
                );
                CREATE TABLE collaboration_agents (
                    id TEXT PRIMARY KEY NOT NULL,
                    job_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    instruction TEXT NOT NULL,
                    workspace_path TEXT,
                    permission_mode TEXT NOT NULL,
                    session_id TEXT NOT NULL UNIQUE,
                    turn_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    summary TEXT,
                    error TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    finished_at INTEGER
                );
                INSERT INTO sessions
                    (id, title, updated_at, archived, artifacts, messages, metadata)
                VALUES ('legacy', 'Legacy session', 10, 0, '[]', '[{"id":"m1"}]', '{}');"#,
        )
        .expect("legacy schema should be created");
    let store = HistoryStore { connection };

    store.initialize().expect("legacy schema should migrate");
    let records = store
        .list_records(false)
        .expect("legacy session should load");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].schema_version, LEGACY_HISTORY_SCHEMA_VERSION);
    assert_eq!(records[0].id, "legacy");
    assert_eq!(records[0].messages, serde_json::json!([{ "id": "m1" }]));
    assert!(records[0].events.is_none());
    let collaboration_columns = {
        let mut statement = store
            .connection
            .prepare("PRAGMA table_info(collaboration_agents)")
            .expect("collaboration columns should load");
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("collaboration columns should be readable")
            .collect::<rusqlite::Result<HashSet<_>>>()
            .expect("collaboration columns should be valid")
    };
    for column in [
        "attempt",
        "lease_id",
        "lease_owner",
        "lease_expires_at",
        "cancel_requested",
    ] {
        assert!(collaboration_columns.contains(column));
    }
}

#[test]
fn legacy_upsert_keeps_new_replay_data_and_version() {
    let store = HistoryStore::open_memory().expect("history database should open");
    let input = HistorySessionInput {
        id: "replay".to_string(),
        title: "Replay session".to_string(),
        updated_at: 20,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: serde_json::json!([]),
        messages: serde_json::json!([]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
        events: Some(serde_json::json!([
            { "type": "status", "sequence": 1, "timestamp": "2026-07-17T00:00:00Z" }
        ])),
    };
    store
        .upsert_input(input)
        .expect("versioned session should save");

    let legacy = HistorySession {
        id: "replay".to_string(),
        title: "Replay session updated".to_string(),
        updated_at: 21,
        archived: false,
        jira_context: None,
        artifacts: serde_json::json!([]),
        messages: serde_json::json!([]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
    };
    store.upsert(&legacy).expect("legacy session should update");

    let record = &store.list_records(false).expect("session should load")[0];
    assert_eq!(record.schema_version, HISTORY_SCHEMA_VERSION);
    assert!(record.events.is_some());
}

#[test]
fn complete_export_keeps_lazy_process_rows_and_events_outside_restore_window() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "export");
    let total = MAX_RESTORED_SESSION_EVENTS + 25;
    let events = (0..total).map(|index| serde_json::json!({
            "type":"process_item", "turn_id":"t", "event_id":format!("e{index}"),
            "item":{"kind":"agent_message", "id":format!("a{index}"), "content":"persisted", "pending":false},
        })).collect::<Vec<_>>();
    store.append_events("export", 1, &events).unwrap();
    assert!(store
        .get_record("export")
        .unwrap()
        .unwrap()
        .events
        .is_none());
    let backup: Vec<HistorySessionRecord> =
        serde_json::from_str(&store.export_json().unwrap()).unwrap();
    assert_eq!(backup[0].events, Some(Value::Array(events.clone())));
    let restored = HistoryStore::open_memory().unwrap();
    restored.upsert_record(&backup[0]).unwrap();
    assert!(restored
        .get_record("export")
        .unwrap()
        .unwrap()
        .events
        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty)));
    let mut page = restored.turn_event_page("export", "t", 0, None).unwrap();
    let mut all = page.events.clone();
    while let Some(cursor) = page.next_cursor {
        page = restored
            .turn_event_page("export", "t", cursor, Some(page.through_row_id))
            .unwrap();
        all.extend(page.events.clone());
    }
    assert_eq!(all, events);
}

#[test]
fn process_pagination_reads_beyond_restore_window_and_keeps_fixed_boundary() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "paged");
    seed_thread(&store, "other");
    let total = MAX_RESTORED_SESSION_EVENTS + 27;
    let events = (0..total)
        .map(|index| {
            serde_json::json!({
                "type":"tool_call_finished", "turn_id":"turn", "event_id":format!("e{index}"),
                "timestamp_ms":index, "sequence":index,
            })
        })
        .collect::<Vec<_>>();
    store.append_events("paged", 1, &events).unwrap();
    store.append_events("other", 1, &events[..1]).unwrap();
    store.append_events("paged", 1, &[serde_json::json!({ "type":"status", "turn_id":"unrelated", "event_id":"unrelated" })]).unwrap();
    assert_eq!(
        store
            .get_record("paged")
            .unwrap()
            .unwrap()
            .events
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        MAX_RESTORED_SESSION_EVENTS
    );
    let mut page = store.turn_event_page("paged", "turn", 0, None).unwrap();
    assert_eq!(page.events.len(), HISTORY_PROCESS_PAGE_SIZE);
    let through = page.through_row_id;
    store
        .append_events(
            "paged",
            2,
            &[serde_json::json!({ "type":"status", "turn_id":"turn", "event_id":"late" })],
        )
        .unwrap();
    let mut restored = page.events.clone();
    while let Some(cursor) = page.next_cursor {
        page = store
            .turn_event_page("paged", "turn", cursor, Some(through))
            .unwrap();
        assert_eq!(page.through_row_id, through);
        restored.extend(page.events.clone());
    }
    assert_eq!(restored, events);
    assert_eq!(
        store
            .turn_event_page("other", "turn", 0, None)
            .unwrap()
            .events
            .len(),
        1
    );
    assert!(store
        .turn_event_page("missing", "turn", 0, None)
        .unwrap()
        .events
        .is_empty());
}

#[test]
fn process_updates_preserve_cursor_and_ignore_stale_replays_after_reopen() {
    let path = temporary_history_path("process-replay");
    {
        let store = HistoryStore::open(&path).unwrap();
        seed_thread(&store, "process");
        let row = |revision, text| {
            serde_json::json!({
                "type":"process_item", "turn_id":"t", "event_id":"process:1:a", "snapshot_revision":revision,
                "item":{"kind":"agent_message", "id":"a", "content":text, "pending":false},
            })
        };
        store
            .append_events("process", 1, &[row(1, "partial")])
            .unwrap();
        let first = store.turn_event_page("process", "t", 0, None).unwrap();
        store
            .append_events(
                "process",
                2,
                &[row(3, "complete"), row(2, "stale"), row(3, "replay")],
            )
            .unwrap();
        let page = store.turn_event_page("process", "t", 0, None).unwrap();
        assert_eq!(page.through_row_id, first.through_row_id);
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0]["item"]["content"], "complete");
        let mut record = store.get_record("process").unwrap().unwrap();
        assert!(record.events.is_none());
        record.events = Some(serde_json::json!([row(99, "stale client save")]));
        store.upsert_record(&record).unwrap();
        store.append_events("process", 2, &[serde_json::json!({ "type":"status", "turn_id":"t", "event_id":"normal", "message":"original" })]).unwrap();
        store.append_events("process", 3, &[serde_json::json!({ "type":"status", "turn_id":"t", "event_id":"normal", "message":"replayed" })]).unwrap();
    }
    {
        let store = HistoryStore::open(&path).unwrap();
        let page = store.turn_event_page("process", "t", 0, None).unwrap();
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0]["item"]["content"], "complete");
        assert_eq!(page.events[1]["message"], "original");
        assert_eq!(
            store
                .get_record("process")
                .unwrap()
                .unwrap()
                .events
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn process_pages_bound_bytes_without_truncating_oversized_items() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "bytes");
    let medium = "x".repeat(HISTORY_PROCESS_PAGE_BYTES / 2);
    let oversized = "y".repeat(HISTORY_PROCESS_PAGE_BYTES + 100);
    let events = vec![
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"one", "text":medium }),
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"two", "text":medium }),
        serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"three", "text":oversized }),
        serde_json::json!({ "type":"completed", "turn_id":"t", "event_id":"four", "text":"done", "durable_terminal":true }),
    ];
    store.append_events("bytes", 1, &events).unwrap();
    let mut page = store.turn_event_page("bytes", "t", 0, None).unwrap();
    let mut restored = page.events.clone();
    assert_eq!(page.events.len(), 1);
    while let Some(cursor) = page.next_cursor {
        page = store
            .turn_event_page("bytes", "t", cursor, Some(page.through_row_id))
            .unwrap();
        assert_eq!(page.events.len(), 1);
        restored.extend(page.events.clone());
    }
    assert_eq!(restored, events);
}

#[test]
fn durable_display_text_restores_only_result_and_retains_aggregate() {
    let mut record = HistorySessionRecord {
        id: "display".into(),
        title: "Display".into(),
        updated_at: 1,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: serde_json::json!([]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
        agent_runs: Vec::new(),
        usage_totals: None,
        messages: serde_json::json!([{ "role":"assistant", "turnId":"t", "content":"Before.Final.", "pending":true }]),
        events: Some(
            serde_json::json!([{ "type":"completed", "turn_id":"t", "text":"Before.Final.", "display_text":"Final.", "durable_terminal":true }]),
        ),
    };
    HistoryStore::hydrate_record_messages(&mut record);
    assert_eq!(record.messages[0]["content"], "Final.");
    assert_eq!(record.messages[0]["events"][0]["text"], "Before.Final.");
    assert_eq!(record.messages[0]["pending"], false);
}

#[test]
fn saving_a_bounded_history_window_preserves_unloaded_events() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "bounded");
    let total = MAX_RESTORED_SESSION_EVENTS + 25;
    let events = (0..total)
        .map(|index| {
            serde_json::json!({
                "type":"tool_call_finished", "event_id":format!("e{index}"),
                "sequence":index, "timestamp_ms":index, "output":"saved evidence"
            })
        })
        .collect::<Vec<_>>();
    store
        .append_events("bounded", total as i64, &events)
        .unwrap();
    let mut loaded = store.get_record("bounded").unwrap().unwrap();
    assert_eq!(
        loaded.events.as_ref().unwrap().as_array().unwrap().len(),
        MAX_RESTORED_SESSION_EVENTS
    );
    loaded.updated_at += 1;
    store.upsert_record(&loaded).unwrap();
    let count: usize = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_id = 'bounded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, total);
    loaded.events = Some(serde_json::json!([]));
    loaded.updated_at += 1;
    store.upsert_record(&loaded).unwrap();
    let oldest: String = store.connection.query_row("SELECT payload FROM session_events WHERE session_id = 'bounded' AND event_key = 'id:e0'", [], |row| row.get(0)).unwrap();
    assert_eq!(decode(oldest)["output"], "saved evidence");
}

#[test]
fn summaries_keep_scope_and_message_count_without_hydrating_messages() {
    let store = HistoryStore::open_memory().expect("history database should open");
    store
        .upsert_input(HistorySessionInput {
            id: "summary".to_string(),
            title: "Summary session".to_string(),
            updated_at: 30,
            archived: false,
            schema_version: HISTORY_SCHEMA_VERSION,
            jira_context: None,
            artifacts: serde_json::json!([]),
            messages: serde_json::json!([
                { "role": "user", "content": "one" },
                { "role": "assistant", "content": "two" }
            ]),
            metadata: serde_json::json!({
                "scope": "workspace",
                "workspacePath": "D:/workspace/demo"
            }),
            report: None,
            usage: None,
            events: None,
        })
        .expect("session should save");

    let summaries = store.list_summaries(None).expect("summaries should load");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].message_count, 2);
    assert_eq!(summaries[0].metadata["workspacePath"], "D:/workspace/demo");

    let record = store
        .get_record("summary")
        .expect("session should load")
        .expect("session should exist");
    assert_eq!(record.messages.as_array().map(|items| items.len()), Some(2));
}

#[test]
fn persisted_messages_build_native_agent_context() {
    let record = HistorySessionRecord {
        id: "thread-a".to_string(),
        title: "Thread".to_string(),
        updated_at: 1,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: serde_json::json!([]),
        messages: serde_json::json!([
            { "id": "user-a", "role": "user", "content": "inspect the project", "turnId": "turn-a" },
            { "id": "assistant-a", "role": "assistant", "content": "I found the issue", "turnId": "turn-a" }
        ]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
        events: None,
        agent_runs: vec![],
        usage_totals: None,
    };
    let context = raw_agent_context_from_record(&record);
    assert_eq!(context.len(), 2);
    assert_eq!(context[0].role, "user");
    assert_eq!(context[0].message_id.as_deref(), Some("user-a"));
    assert!(context[1].content.starts_with("[历史助手陈述，非工具证据]"));
}

#[test]
fn typed_checkpoint_survives_history_reopen_with_tool_pairing() {
    let path = temporary_history_path("typed-checkpoint-reopen");
    let checkpoint = typed_agent_checkpoint(
        "checkpoint-a",
        "model-a",
        "assistant-covered",
        CONTEXT_CHECKPOINT_FORMAT_VERSION,
    );
    {
        let store = HistoryStore::open(&path).expect("history database should open");
        store
                .upsert_input(HistorySessionInput {
                    id: "checkpoint-thread".to_string(),
                    title: "Checkpoint thread".to_string(),
                    updated_at: 10,
                    archived: false,
                    schema_version: HISTORY_SCHEMA_VERSION,
                    jira_context: None,
                    artifacts: serde_json::json!([]),
                    messages: serde_json::json!([
                        {"id":"user-covered","role":"user","content":"covered request","turnId":"turn-covered"},
                        {"id":"assistant-covered","role":"assistant","content":"covered answer","turnId":"turn-covered"},
                        {"id":"user-recent","role":"user","content":"recent request","turnId":"turn-recent"},
                        {"id":"assistant-recent","role":"assistant","content":"recent answer","turnId":"turn-recent"},
                        {"id":"user-current","role":"user","content":"current request","turnId":"turn-current"},
                        {"id":"assistant-current","role":"assistant","content":"","pending":true,"turnId":"turn-current"}
                    ]),
                    metadata: serde_json::json!({}),
                    report: None,
                    usage: None,
                    events: None,
                })
                .expect("session should persist");
        store
            .append_events(
                "checkpoint-thread",
                11,
                &[serde_json::json!({
                    "type": "completed",
                    "turn_id": "turn-covered",
                    "durable_terminal": true,
                    "sequence": 1,
                    "timestamp_ms": 11,
                    "text": "covered answer",
                    "contextCheckpoint": checkpoint,
                })],
            )
            .expect("checkpoint event should persist");
    }

    let reopened = HistoryStore::open(&path).expect("history database should reopen");
    let record = reopened
        .get_record("checkpoint-thread")
        .expect("history should load")
        .expect("session should exist");
    let restored = agent_context_from_record(&record, "model-a", 3, Some("turn-current"))
        .expect("typed checkpoint should restore");

    assert_eq!(
        restored
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );
    assert_eq!(restored[3].tool_calls.len(), 1);
    assert_eq!(restored[3].tool_calls[0].id, "call-1");
    assert_eq!(restored[3].tool_calls[0].name, "read_local_file");
    assert_eq!(restored[4].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(restored[4].content, "durable tool evidence");
    assert_eq!(restored[5].content, "recent request");
    assert!(restored[6].content.contains("recent answer"));
    assert!(!restored
        .iter()
        .any(|message| message.content == "current request"));

    drop(reopened);
    std::fs::remove_file(&path).expect("temporary history database should be removable");
}

#[test]
fn checkpoints_keep_portable_evidence_across_models_and_reject_unknown_formats() {
    for (label, checkpoint, current_model) in [
        (
            "format",
            typed_agent_checkpoint(
                "checkpoint-format",
                "model-a",
                "assistant-covered",
                CONTEXT_CHECKPOINT_FORMAT_VERSION + 1,
            ),
            "model-a",
        ),
        (
            "model",
            typed_agent_checkpoint(
                "checkpoint-model",
                "model-a",
                "assistant-covered",
                CONTEXT_CHECKPOINT_FORMAT_VERSION,
            ),
            "model-b",
        ),
    ] {
        let record = HistorySessionRecord {
            id: format!("thread-{label}"),
            title: "Thread".to_string(),
            updated_at: 2,
            archived: false,
            schema_version: HISTORY_SCHEMA_VERSION,
            jira_context: None,
            artifacts: serde_json::json!([]),
            messages: serde_json::json!([
                {"id":"assistant-covered","role":"assistant","content":"covered raw answer","turnId":"turn-covered"},
                {"id":"user-recent","role":"user","content":"recent request","turnId":"turn-recent"}
            ]),
            metadata: serde_json::json!({}),
            report: None,
            usage: None,
            events: Some(serde_json::json!([{
                "type":"completed",
                "turn_id":"turn-covered",
                "durable_terminal":true,
                "contextCheckpoint": checkpoint
            }])),
            agent_runs: vec![],
            usage_totals: None,
        };
        let restored = agent_context_from_record(&record, current_model, 3, None)
            .expect("legacy summary should be a safe fallback");
        if label == "model" {
            assert!(restored.iter().any(|message| message.role == MessageRole::Tool));
            assert!(restored.iter().any(|message| message.content.contains("durable tool evidence")));
            assert_eq!(restored.last().unwrap().content, "recent request");
            continue;
        }
        assert_eq!(restored.len(), 2, "{label}");
        assert_eq!(restored[0].role, MessageRole::Assistant, "{label}");
        assert!(
            restored[0].content.contains("legacy checkpoint summary"),
            "{label}"
        );
        assert!(restored[0].tool_calls.is_empty(), "{label}");
        assert_eq!(restored[1].role, MessageRole::User, "{label}");
        assert_eq!(restored[1].content, "recent request", "{label}");
    }
}

#[test]
fn missing_checkpoint_boundary_fails_closed_to_raw_history() {
    let checkpoint = typed_agent_checkpoint(
        "checkpoint-missing-boundary",
        "model-a",
        "missing-message",
        CONTEXT_CHECKPOINT_FORMAT_VERSION,
    );
    let record = HistorySessionRecord {
        id: "thread-missing-boundary".to_string(),
        title: "Thread".to_string(),
        updated_at: 2,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: serde_json::json!([]),
        messages: serde_json::json!([
            {"id":"user-a","role":"user","content":"raw request","turnId":"turn-a"},
            {"id":"assistant-a","role":"assistant","content":"raw answer","turnId":"turn-a"}
        ]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
        events: Some(serde_json::json!([{
            "type":"completed",
            "turn_id":"turn-a",
            "durable_terminal":true,
            "contextCheckpoint": checkpoint
        }])),
        agent_runs: vec![],
        usage_totals: None,
    };

    assert!(agent_context_from_record(&record, "model-a", 3, None).is_none());
    assert_eq!(raw_agent_context_from_record(&record).len(), 2);
}

#[test]
fn hydrated_history_uses_durable_terminal_text_for_its_turn() {
    let mut record = HistorySessionRecord {
        id: "thread-a".to_string(),
        title: "Thread".to_string(),
        updated_at: 1,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: serde_json::json!([]),
        messages: serde_json::json!([
            { "id": "assistant-a", "role": "assistant", "content": "streamed", "pending": true, "turnId": "turn-a" }
        ]),
        metadata: serde_json::json!({}),
        report: None,
        usage: None,
        events: Some(serde_json::json!([
            { "type": "completed", "turn_id": "turn-a", "text": "final", "durable_terminal": true, "timestamp_ms": 22 }
        ])),
        agent_runs: vec![],
        usage_totals: None,
    };

    HistoryStore::hydrate_record_messages(&mut record);

    let message = &record
        .messages
        .as_array()
        .expect("messages should remain an array")[0];
    assert_eq!(message["content"], "final");
    assert_eq!(message["pending"], false);
    assert_eq!(message["finishedAt"], 22);
    assert_eq!(message["events"].as_array().map(Vec::len), Some(1));
}

#[test]
fn imported_terminal_order_survives_a_process_sequence_reset() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "sequence-reset");
    let mut record = store.get_record("sequence-reset").unwrap().unwrap();
    record.messages =
        serde_json::json!([{ "role":"assistant", "turnId":"t", "content":"old", "pending":true }]);
    record.events = Some(serde_json::json!([
        { "type":"error", "turn_id":"t", "event_id":"old", "sequence":999, "timestamp_ms":1000, "message":"old error", "durable_terminal":true },
        { "type":"completed", "turn_id":"t", "event_id":"new", "sequence":2, "timestamp_ms":2000, "display_text":"New result.", "durable_terminal":true }
    ]));
    store.upsert_record(&record).unwrap();
    let restored = store.get_record("sequence-reset").unwrap().unwrap();
    assert_eq!(restored.messages[0]["content"], "New result.");
    assert_eq!(restored.messages[0]["error"], false);
    assert_eq!(restored.messages[0]["finishedAt"], 2000);
    assert_eq!(
        store
            .persisted_durable_terminal_events("sequence-reset")
            .unwrap()[0]
            .1["type"],
        "completed"
    );
}

#[test]
fn ordered_context_deduplicates_initial_ids_but_retains_a_repeated_legacy_steer() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "context-segments");
    let mut record = store.get_record("context-segments").unwrap().unwrap();
    record.messages = serde_json::json!([
        {"id":"optimistic-user", "role":"user", "content":"Check this.", "turnId":"t", "attachments":[{"name":"i.png", "dataUrl":"data:image/png;base64,aA=="}]},
        {"id":"assistant:t", "role":"assistant", "content":"Done.", "turnId":"t"}
    ]);
    record.events = Some(serde_json::json!([{
        "type":"completed", "turn_id":"t", "durable_terminal":true,
        "display_segments":[
            {"id":"user:t", "role":"user", "content":"Check this."},
            {"id":"assistant:t", "role":"assistant", "content":"Done."}
        ]
    }]));
    let context = raw_agent_context_from_record(&record);
    assert_eq!(context.len(), 2);
    assert_eq!(context[0].content, "Check this.");
    assert_eq!(context[0].message_id.as_deref(), Some("user:t"));
    assert_eq!(context[0].attachments.len(), 1);

    record.events.as_mut().unwrap()[0]["display_segments"] = serde_json::json!([
        {"id":"assistant:t", "role":"assistant", "content":"First finding."},
        {"id":"steer:t", "role":"user", "content":"Check this."},
        {"id":"assistant:t:segment-1", "role":"assistant", "content":"Second finding."}
    ]);
    let context = raw_agent_context_from_record(&record);
    assert_eq!(context.len(), 4);
    assert_eq!(context[0].content, "Check this.");
    assert_eq!(context[2].content, "Check this.");
    assert!(context[2].attachments.is_empty());
}

#[test]
fn restored_user_images_keep_owner_and_support_legacy_frontend_metadata() {
    for image in [
        serde_json::json!({"name":"native.png", "mediaType":"image/png", "dataUrl":"data:image/png;base64,aA=="}),
        serde_json::json!({"name":"browser.png", "kind":"image", "dataUrl":"data:image/png;base64,aA=="}),
    ] {
        let user = serde_json::json!({"id":"user:t:steer:s", "turnId":"t", "role":"user", "content":"", "attachments":[image]});
        let turn = raw_agent_context_turn(&user).unwrap();
        assert_eq!(turn.message_id.as_deref(), Some("user:t:steer:s"));
        assert_eq!(turn.turn_id.as_deref(), Some("t"));
        assert_eq!(turn.attachments.len(), 1);
        assert_eq!(turn.attachments[0].media_type, "image/png");
        assert!(!turn.content.is_empty());
        let mut assistant = user.clone();
        assistant["role"] = serde_json::json!("assistant");
        assistant["content"] = serde_json::json!("answer");
        assert!(raw_agent_context_turn(&assistant)
            .unwrap()
            .attachments
            .is_empty());
    }
    assert!(raw_agent_context_turn(&serde_json::json!({"role":"user", "content":"", "attachments":[{"kind":"file", "name":"f.txt"}]})).is_none());
}

#[test]
fn legacy_terminal_restores_steer_attachments_without_file_content_duplication() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "attachment-display");
    let mut record = store.get_record("attachment-display").unwrap().unwrap();
    record.messages = serde_json::json!([{ "id":"assistant:t", "role":"assistant", "turnId":"t", "content":"Done." }]);
    store.upsert_record(&record).unwrap();
    let image = serde_json::json!({"name":"i.png", "mediaType":"image/png", "dataUrl":"data:image/png;base64,aA=="});
    store.append_events("attachment-display", 100, &[
            serde_json::json!({"type":"process_item", "turn_id":"t", "event_id":"u", "execution_started_at":1, "process_order":0, "item":{"kind":"user_message", "id":"user:t", "content":"Initial", "attachments":[image.clone()]}}),
            serde_json::json!({"type":"process_item", "turn_id":"t", "event_id":"s", "execution_started_at":1, "process_order":1, "item":{"kind":"timeline_event", "id":"s", "data":{"type":"steered", "message":"Look", "attachments":[image], "artifacts":[{"name":"f.txt", "path":"attachment://f.txt", "kind":"file", "content":"do not copy"}]}}}),
            serde_json::json!({"type":"cancelled", "turn_id":"t", "event_id":"stop", "timestamp_ms":100, "durable_terminal":true})
        ]).unwrap();
    let restored = store.get_record("attachment-display").unwrap().unwrap();
    let terminal = restored
        .events
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "cancelled")
        .unwrap();
    let segments = terminal["display_segments"].as_array().unwrap();
    assert_eq!(segments[0]["attachments"][0]["name"], "i.png");
    assert_eq!(segments[1]["attachments"][0]["name"], "i.png");
    assert_eq!(segments[1]["artifacts"][0]["name"], "f.txt");
    assert!(segments[1]["artifacts"][0].get("content").is_none());
    let context = raw_agent_context_from_record(&restored);
    assert_eq!(context[0].message_id.as_deref(), Some("user:t"));
    assert_eq!(context[1].message_id.as_deref(), Some("user:t:steer:s"));
    assert_eq!(context[1].attachments.len(), 1);
}

#[test]
fn history_disclosure_requires_execution_details_not_terminal_diagnostics() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "disclosure");
    store.append_events("disclosure", 100, &[
            serde_json::json!({"type":"process_item", "turn_id":"reply", "event_id":"reply-text", "execution_started_at":1, "process_order":0, "item":{"kind":"agent_message", "id":"a", "content":"Let me check."}}),
            serde_json::json!({"type":"process_item", "turn_id":"reply", "event_id":"reply-status", "execution_started_at":1, "process_order":1, "item":{"kind":"timeline_event", "id":"s", "data":{"type":"status", "status":"failed", "message":"Unavailable", "terminalDiagnostic":true}}}),
            serde_json::json!({"type":"error", "turn_id":"reply", "event_id":"reply-end", "timestamp_ms":100, "durable_terminal":true}),
            serde_json::json!({"type":"process_item", "turn_id":"tools", "event_id":"tools-call", "execution_started_at":2, "process_order":0, "item":{"kind":"tool_call", "id":"c", "name":"run_local_command", "input":{}, "output":"ok"}}),
            serde_json::json!({"type":"completed", "turn_id":"tools", "event_id":"tools-end", "timestamp_ms":100, "durable_terminal":true}),
            serde_json::json!({"type":"process_item", "turn_id":"reasoning", "event_id":"reasoning-text", "execution_started_at":3, "process_order":0, "item":{"kind":"timeline_event", "id":"r", "data":{"type":"reasoning_delta", "text":"Check evidence"}}}),
            serde_json::json!({"type":"completed", "turn_id":"reasoning", "event_id":"reasoning-end", "timestamp_ms":100, "durable_terminal":true}),
        ]).unwrap();
    let restored = store.get_record("disclosure").unwrap().unwrap();
    let events = restored.events.as_ref().unwrap().as_array().unwrap();
    for (turn, expected) in [("reply", false), ("tools", true), ("reasoning", true)] {
        let terminal = events
            .iter()
            .find(|event| event["turn_id"] == turn && event["durable_terminal"] == true)
            .unwrap();
        assert_eq!(terminal["has_execution_details"], expected, "{turn}");
    }
    assert!(events.iter().all(|event| event["type"] != "process_item"));
}

#[test]
fn cancelled_turn_restores_visible_text_and_execution_clock() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "cancel-display");
    let mut record = store.get_record("cancel-display").unwrap().unwrap();
    record.messages = serde_json::json!([{ "id":"assistant:t", "role":"assistant", "turnId":"t", "content":"已停止。", "startedAt":321_000, "pending":false }]);
    store.upsert_record(&record).unwrap();
    store.append_events("cancel-display", 321_000, &[
            serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"p1", "execution_started_at":1000, "process_order":1, "item":{"id":"a", "kind":"agent_message", "content":"First finding."} }),
            serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"steer", "execution_started_at":1000, "process_order":2, "item":{"id":"steer-note", "kind":"timeline_event", "data":{"type":"steered", "message":"Additional instruction."}} }),
            serde_json::json!({ "type":"process_item", "turn_id":"t", "event_id":"p2", "execution_started_at":1000, "process_order":3, "item":{"id":"b", "kind":"agent_message", "content":"Second finding."} }),
            serde_json::json!({ "type":"cancelled", "turn_id":"t", "event_id":"stop", "execution_started_at":1000, "timestamp_ms":321_000, "durable_terminal":true }),
        ]).unwrap();
    let restored = store.get_record("cancel-display").unwrap().unwrap();
    assert_eq!(
        restored.messages[0]["content"],
        "First finding.\n\nSecond finding."
    );
    assert_eq!(restored.messages[0]["startedAt"], 1000);
    assert_eq!(restored.messages[0]["finishedAt"], 321_000);
    let terminal = restored
        .events
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "cancelled")
        .unwrap();
    assert_eq!(terminal["display_segments"][0]["content"], "First finding.");
    assert_eq!(terminal["display_segments"][1]["role"], "user");
    assert_eq!(
        terminal["display_segments"][1]["content"],
        "Additional instruction."
    );
    assert_eq!(
        terminal["display_segments"][2]["content"],
        "Second finding."
    );
    assert_eq!(terminal["display_segments"][0]["process_end"], 2);
    assert_eq!(terminal["display_segments"][2]["process_start"], 3);
    let context = raw_agent_context_from_record(&restored);
    assert_eq!(context.len(), 3);
    assert_eq!(context[1].role, "user");
    assert_eq!(context[1].content, "Additional instruction.");
    assert!(restored
        .events
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["type"] != "process_item"));
}
#[test]
fn cached_image_context_retains_reference_and_legacy_inline_ignores_original_path() {
    let cached = serde_json::json!({"id":"user:t", "role":"user", "content":"look", "attachments":[{
        "name":"screen.png", "kind":"image", "path":"D:/screen.png", "imagePath":"cache/hash", "mediaType":"image/png"
    }]});
    let turn = raw_agent_context_turn(&cached).unwrap();
    assert_eq!(turn.attachments[0].path.as_deref(), Some("cache/hash"));
    assert!(turn.attachments[0].data_url.is_empty());
    let mut legacy = cached;
    legacy["attachments"][0].as_object_mut().unwrap().remove("imagePath");
    legacy["attachments"][0]["dataUrl"] = "data:image/png;base64,YQ==".into();
    let turn = raw_agent_context_turn(&legacy).unwrap();
    assert!(turn.attachments[0].path.is_none());
}
