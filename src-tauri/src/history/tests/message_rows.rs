use super::*;
use crate::history::tests::support::seed_thread;
use serde_json::json;

/// §8.9 messages 分行（阶段一：双写 + get_record 读切换）的行为锁。
fn record_with_messages(id: &str, updated_at: i64, messages: Value) -> HistorySessionRecord {
    HistorySessionRecord {
        id: id.to_string(),
        title: "T".to_string(),
        updated_at,
        archived: false,
        schema_version: HISTORY_SCHEMA_VERSION,
        jira_context: None,
        artifacts: json!([]),
        messages,
        metadata: json!({}),
        report: None,
        usage: None,
        events: None,
        agent_runs: Vec::new(),
        usage_totals: None,
    }
}

fn stored_row_count(store: &HistoryStore, session_id: &str) -> i64 {
    store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn message_rows_are_incremental_consistent_and_deleted_correctly() {
    let store = HistoryStore::open_memory().unwrap();
    let first = json!([
        { "id": "user:turn-1", "role": "user", "content": "开始" },
        { "id": "assistant:turn-1", "role": "assistant", "content": "回复一" },
    ]);
    store.upsert_record(&record_with_messages("s", 1, first.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 2);
    // 读切换：get_record 以分行为准，与写入数组一致。
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, first);

    // 追加一轮：只有新增行入库（增量），旧行不变。
    let second = json!([
        { "id": "user:turn-1", "role": "user", "content": "开始" },
        { "id": "assistant:turn-1", "role": "assistant", "content": "回复一" },
        { "id": "user:turn-2", "role": "user", "content": "继续" },
        { "id": "assistant:turn-2", "role": "assistant", "content": "回复二" },
    ]);
    store.upsert_record(&record_with_messages("s", 2, second.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 4);
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, second);

    // 内容变更：同 id 改内容要落库（WHERE 子句放行变化行）。
    let edited = json!([
        { "id": "user:turn-1", "role": "user", "content": "开始（编辑）" },
        { "id": "assistant:turn-1", "role": "assistant", "content": "回复一" },
        { "id": "user:turn-2", "role": "user", "content": "继续" },
        { "id": "assistant:turn-2", "role": "assistant", "content": "回复二" },
    ]);
    store.upsert_record(&record_with_messages("s", 3, edited.clone())).unwrap();
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, edited);

    // 回退删除：消息消失后行被删除，seq 顺序保持。
    let rewound = json!([
        { "id": "user:turn-1", "role": "user", "content": "开始（编辑）" },
    ]);
    store.upsert_record(&record_with_messages("s", 4, rewound.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 1);
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, rewound);

    // 清空：全部删除后行清零，读回退到列（列仍是空数组权威）。
    store.upsert_record(&record_with_messages("s", 5, json!([]))).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 0);
}

#[test]
fn malformed_message_ids_keep_the_column_authoritative() {
    let store = HistoryStore::open_memory().unwrap();
    // 缺 id / 空 id / 重复 id：merge 整体跳过——分行为空，读走旧列兜底。
    let no_id = json!([{ "role": "user", "content": "no id" }]);
    store.upsert_record(&record_with_messages("s", 1, no_id.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 0);
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, no_id);

    let duplicate = json!([
        { "id": "same", "role": "user", "content": "a" },
        { "id": "same", "role": "user", "content": "b" },
    ]);
    store.upsert_record(&record_with_messages("s", 2, duplicate.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "s"), 0);
    assert_eq!(store.get_record("s").unwrap().unwrap().messages, duplicate);
}

#[test]
fn legacy_column_sessions_fall_back_until_first_write() {
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "legacy");
    // 直接把列写成旧形态（绕过 upsert 的分行逻辑），模拟未迁移的旧会话。
    store
        .connection
        .execute(
            "UPDATE sessions SET messages = ?1 WHERE id = 'legacy'",
            params![serde_json::to_string(&json!([
                { "id": "user:old", "role": "user", "content": "旧消息" },
            ])).unwrap()],
        )
        .unwrap();
    // 读兜底走列。
    let record = store.get_record("legacy").unwrap().unwrap();
    assert_eq!(record.messages[0]["id"], "user:old");
    // 下一次正常写入后分行为准（惰性迁移完成）。
    let migrated = json!([
        { "id": "user:old", "role": "user", "content": "旧消息" },
        { "id": "user:new", "role": "user", "content": "新消息" },
    ]);
    store.upsert_record(&record_with_messages("legacy", 2, migrated.clone())).unwrap();
    assert_eq!(stored_row_count(&store, "legacy"), 2);
    assert_eq!(store.get_record("legacy").unwrap().unwrap().messages, migrated);
}
