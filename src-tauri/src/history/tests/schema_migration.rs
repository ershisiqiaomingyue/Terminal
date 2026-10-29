use super::support::*;
use super::*;

/// `dsh_*` 是移植期的内部代号。老库必须原地改名，数据、索引和约束都要活下来，
/// 而且第二次打开不能再动一次（迁移必须幂等，否则每次启动都多一次写事务）。
#[test]
fn legacy_table_and_column_names_are_renamed_in_place_once() {
    let store = HistoryStore::open_memory().unwrap();
    // 把库倒回旧形态：用旧名建表、灌入数据，确认改完之后这些东西还在。
    store
        .connection
        .execute_batch(
            "DROP TABLE turn_recovery;
             DROP TABLE turn_outbox;
             DROP TABLE session_bindings;
             CREATE TABLE dsh_outbox (
                 session_id TEXT NOT NULL,
                 message_id TEXT NOT NULL,
                 mode TEXT NOT NULL,
                 message TEXT NOT NULL,
                 accepted INTEGER NOT NULL DEFAULT 0,
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 UNIQUE(session_id, message_id)
             );
             CREATE INDEX idx_dsh_outbox_pending ON dsh_outbox(accepted, sequence);
             CREATE TABLE dsh_sessions (
                 session_id TEXT PRIMARY KEY NOT NULL,
                 storage_id TEXT UNIQUE NOT NULL,
                 seed_from TEXT,
                 deleted INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE dsh_recovery (
                 session_id TEXT PRIMARY KEY NOT NULL,
                 needs_recovery INTEGER NOT NULL DEFAULT 1
             );
             INSERT INTO dsh_outbox(session_id,message_id,mode,message)
                 VALUES('legacy','m1','followup','{}');
             INSERT INTO dsh_sessions(session_id,storage_id,seed_from,deleted)
                 VALUES('legacy','legacy','',0);
             INSERT INTO dsh_recovery(session_id,needs_recovery) VALUES('legacy',1);
             ALTER TABLE conversation_checkpoints ADD COLUMN dsh_cut TEXT;
             INSERT INTO conversation_threads(thread_id,root_thread_id,created_at,updated_at)
                 VALUES('thread','thread',1,1);
             INSERT INTO conversation_checkpoints
                 (id,thread_id,messages,metadata,artifacts,events,created_at,dsh_cut)
                 VALUES('cp','thread','[]','{}','[]','[]',1,'cut-9');",
        )
        .unwrap();

    store.initialize().unwrap();

    let table_exists = |name: &str| {
        store
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [name],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    };
    for legacy in ["dsh_outbox", "dsh_sessions", "dsh_recovery"] {
        assert!(!table_exists(legacy), "{legacy} 应已改名");
    }
    for current in ["turn_outbox", "session_bindings", "turn_recovery"] {
        assert!(table_exists(current), "{current} 应存在");
    }
    // 数据跟着表一起搬过去了。
    assert_eq!(
        store
            .connection
            .query_row("SELECT message_id FROM turn_outbox", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "m1"
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT storage_id FROM session_bindings", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "legacy"
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT needs_recovery FROM turn_recovery", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    // 索引被 SQLite 一起带过来，没有在改名中丢掉。
    assert!(store
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_dsh_outbox_pending')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    // 列改名后旧值仍在，新列不再是 NULL。
    assert_eq!(
        store
            .connection
            .query_row("SELECT checkpoint_cut FROM conversation_checkpoints", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "cut-9"
    );
    let columns = store
        .connection
        .prepare("PRAGMA table_info(conversation_checkpoints)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!columns.iter().any(|column| column == "dsh_cut"));
    assert!(columns.iter().any(|column| column == "checkpoint_cut"));

    // 幂等：再跑一次不应报错，也不应改动任何东西。
    store.initialize().unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM turn_outbox", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

/// 恢复标记表在旧库里可能还不存在（更老的库）：改名跳过，随后由
/// `initialize_turn_recovery` 正常建表并从 outbox 播种。
#[test]
fn missing_legacy_recovery_table_is_created_after_the_rename() {
    let store = HistoryStore::open_memory().unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE turn_recovery;
             ALTER TABLE turn_outbox RENAME TO dsh_outbox;
             INSERT INTO dsh_outbox(session_id,message_id,mode,message)
                 VALUES('old','m2','followup','{}');",
        )
        .unwrap();
    store.initialize().unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT session_id FROM turn_recovery", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "old",
        "改名后的 outbox 要参与恢复表的播种"
    );
}

#[test]
fn freshly_created_database_never_uses_the_legacy_names() {
    let store = HistoryStore::open_memory().unwrap();
    let legacy = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'dsh_%' OR name LIKE '%_dsh_%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(legacy, 0, "新建库不应出现任何 dsh 命名的对象");
    let columns = store
        .connection
        .prepare("PRAGMA table_info(conversation_checkpoints)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!columns.iter().any(|column| column == "dsh_cut"));
    assert!(columns.iter().any(|column| column == "checkpoint_cut"));
}

#[test]
fn seeded_thread_is_readable_after_the_rename() {
    // 端到端：改完名之后，既有读写路径仍能正常工作。
    let store = HistoryStore::open_memory().unwrap();
    seed_thread(&store, "renamed");
    assert!(store.get_record("renamed").unwrap().is_some());
}
