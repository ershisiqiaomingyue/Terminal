use super::*;

impl HistoryStore {
    pub fn open_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .busy_timeout(HISTORY_WRITE_BUSY_TIMEOUT)
            .map_err(|error| format!("无法设置历史数据库锁等待: {error}"))?;
        let store = Self { connection };
        store.initialize()?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch("PRAGMA auto_vacuum = INCREMENTAL")
            .map_err(|error| format!("无法配置历史空间回收: {error}"))?;
        // WAL（写前日志）让读与写不再互斥。此前 delete 回滚日志模式下，一次
        // 写事务（turn 终态重写 1MB messages + 事件追加）持锁期间，读侧
        // get_history_session 会阻塞到 busy 超时，前端表现为「历史会话无法
        // 初始化／加载对话失败」——在数百 MB 的真实库上尤为频繁。WAL 是
        // 持久设置（首次生效后写进库文件），之后每次打开都是幂等确认。
        connection
            .execute_batch("PRAGMA journal_mode = WAL")
            .map_err(|error| format!("无法启用历史数据库 WAL 模式: {error}"))?;
        connection
            .execute_batch("PRAGMA synchronous = NORMAL")
            .map_err(|error| format!("无法配置历史数据库同步档位: {error}"))?;
        connection
            .busy_timeout(HISTORY_WRITE_BUSY_TIMEOUT)
            .map_err(|error| format!("无法设置历史数据库锁等待: {error}"))?;
        let store = Self { connection };
        store.initialize()?;
        Ok(store)
    }

    pub(super) fn initialize(&self) -> Result<(), String> {
        // 旧库的 dsh_* 表必须在下面的 CREATE TABLE IF NOT EXISTS 之前改名：
        // 否则建表语句先造出一张空的 turn_outbox，迁移看到「目标已存在」就跳过，
        // 老数据会被留在弃用的旧表里。全新库没有旧表，这里就是空操作。
        self.migrate_legacy_table_names()?;
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS model_transcripts (
                    session_id TEXT PRIMARY KEY NOT NULL, payload BLOB NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    title TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0,
                    jira_context TEXT,
                    artifacts TEXT NOT NULL,
                    messages TEXT NOT NULL,
                    metadata TEXT NOT NULL,
                    report TEXT,
                    usage TEXT,
                    schema_version INTEGER NOT NULL DEFAULT 1,
                    events TEXT,
                    message_count INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_sessions_archived ON sessions(archived);
                CREATE TABLE IF NOT EXISTS session_events (
                    session_id TEXT NOT NULL,
                    event_key TEXT NOT NULL,
                    sequence INTEGER,
                    timestamp_ms INTEGER,
                    payload TEXT NOT NULL,
                    PRIMARY KEY(session_id, event_key)
                );
                 CREATE INDEX IF NOT EXISTS idx_session_events_order
                     ON session_events(session_id, sequence, timestamp_ms);
                 CREATE INDEX IF NOT EXISTS idx_session_events_turn
                     ON session_events(session_id, COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')));
                 CREATE INDEX IF NOT EXISTS idx_session_events_terminal
                     ON session_events(session_id)
                     WHERE json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
                       AND json_extract(payload, '$.durable_terminal') = 1;
                 CREATE INDEX IF NOT EXISTS idx_session_events_usage_terminal
                     ON session_events(session_id, COALESCE(timestamp_ms, 0))
                     WHERE json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error');
                 CREATE INDEX IF NOT EXISTS idx_session_events_recent_visual
                     ON session_events(session_id, COALESCE(timestamp_ms, 0))
                     WHERE COALESCE(json_extract(payload, '$.type'), '') != 'process_item';
                 CREATE TABLE IF NOT EXISTS agent_runs (
                     session_id TEXT NOT NULL,
                     turn_id TEXT NOT NULL,
                     status TEXT NOT NULL,
                     status_message TEXT,
                     request TEXT NOT NULL,
                     started_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     finished_at INTEGER,
                     resume_count INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(session_id, turn_id)
                 );
                 CREATE INDEX IF NOT EXISTS idx_agent_runs_recovery
                     ON agent_runs(status, updated_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_agent_runs_session
                     ON agent_runs(session_id, updated_at DESC);
                 CREATE TABLE IF NOT EXISTS agent_queue (
                     id TEXT PRIMARY KEY NOT NULL,
                     session_id TEXT NOT NULL,
                     idempotency_key TEXT NOT NULL,
                     request TEXT NOT NULL,
                     execution_target TEXT,
                     status TEXT NOT NULL,
                     sequence INTEGER NOT NULL UNIQUE,
                     attempt_count INTEGER NOT NULL DEFAULT 0,
                     lease_id TEXT,
                     lease_owner TEXT,
                     lease_expires_at INTEGER,
                     last_error TEXT,
                     enqueued_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     completed_at INTEGER,
                     UNIQUE(session_id, idempotency_key)
                 );
                 CREATE INDEX IF NOT EXISTS idx_agent_queue_pending
                     ON agent_queue(session_id, status, sequence);
                 CREATE TABLE IF NOT EXISTS turn_outbox (
                     session_id TEXT NOT NULL,
                     message_id TEXT NOT NULL,
                     mode TEXT NOT NULL,
                     message TEXT NOT NULL,
                     accepted INTEGER NOT NULL DEFAULT 0,
                     sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                     UNIQUE(session_id, message_id)
                 );
                 CREATE INDEX IF NOT EXISTS idx_turn_outbox_pending
                     ON turn_outbox(accepted, sequence);
                 CREATE INDEX IF NOT EXISTS idx_turn_outbox_latest
                     ON turn_outbox(session_id, sequence DESC);
                 CREATE TABLE IF NOT EXISTS session_bindings (
                     session_id TEXT PRIMARY KEY NOT NULL,
                     storage_id TEXT UNIQUE NOT NULL,
                     seed_from TEXT,
                     deleted INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS idx_agent_queue_lease
                     ON agent_queue(status, lease_expires_at);
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_queue_one_claim_per_session
                     ON agent_queue(session_id) WHERE status = 'claimed';
                 CREATE TABLE IF NOT EXISTS conversation_threads (
                     thread_id TEXT PRIMARY KEY NOT NULL,
                     root_thread_id TEXT NOT NULL,
                     parent_thread_id TEXT,
                     forked_from_checkpoint_id TEXT,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_conversation_threads_root
                     ON conversation_threads(root_thread_id, updated_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_conversation_threads_parent
                     ON conversation_threads(parent_thread_id, created_at DESC);
                 CREATE TABLE IF NOT EXISTS conversation_checkpoints (
                     id TEXT PRIMARY KEY NOT NULL,
                     thread_id TEXT NOT NULL,
                     turn_id TEXT,
                     label TEXT,
                     messages TEXT NOT NULL,
                     metadata TEXT NOT NULL,
                     artifacts TEXT NOT NULL,
                     report TEXT,
                     usage TEXT,
                     events TEXT NOT NULL,
                     created_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_conversation_checkpoints_thread
                     ON conversation_checkpoints(thread_id, created_at DESC);
                 CREATE TABLE IF NOT EXISTS collaboration_jobs (
                     id TEXT PRIMARY KEY NOT NULL,
                     thread_id TEXT NOT NULL,
                     parent_turn_id TEXT,
                     goal TEXT NOT NULL,
                     workspace_path TEXT,
                     permission_mode TEXT NOT NULL,
                     execution_target TEXT,
                     agent_config TEXT NOT NULL DEFAULT '{}',
                     aggregate_summary TEXT,
                     cancel_requested INTEGER NOT NULL DEFAULT 0,
                     status TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     finished_at INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_collaboration_jobs_thread
                     ON collaboration_jobs(thread_id, updated_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_collaboration_jobs_status
                     ON collaboration_jobs(status, updated_at DESC);
                 CREATE TABLE IF NOT EXISTS collaboration_agents (
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
                     attempt INTEGER NOT NULL DEFAULT 0,
                     lease_id TEXT,
                     lease_owner TEXT,
                     lease_expires_at INTEGER,
                     cancel_requested INTEGER NOT NULL DEFAULT 0,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     finished_at INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_collaboration_agents_job
                     ON collaboration_agents(job_id, created_at ASC);
                 CREATE INDEX IF NOT EXISTS idx_collaboration_agents_status
                     ON collaboration_agents(status, updated_at ASC);
                 CREATE TABLE IF NOT EXISTS agent_steers (
                     id TEXT PRIMARY KEY NOT NULL,
                     session_id TEXT NOT NULL,
                     turn_id TEXT NOT NULL,
                     request TEXT NOT NULL,
                     source_queue_id TEXT UNIQUE,
                     status TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     applied_at INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_agent_steers_pending
                     ON agent_steers(session_id, status, created_at);
                 CREATE TABLE IF NOT EXISTS attachment_cleanup (
                     session_id TEXT PRIMARY KEY NOT NULL,
                     files TEXT NOT NULL
                 );
                 -- G1：goal 一等公民（对照 codex thread_goals 六态状态机 + 预算字段；
                 -- parent_id 树是我们对 codex 的增强，供 subagent 场景归属）。
                 CREATE TABLE IF NOT EXISTS goals (
                     id TEXT PRIMARY KEY NOT NULL,
                     session_id TEXT NOT NULL,
                     parent_id TEXT REFERENCES goals(id) ON DELETE SET NULL,
                     objective TEXT NOT NULL,
                     acceptance TEXT NOT NULL DEFAULT '[]',
                     status TEXT NOT NULL CHECK(status IN
                         ('active','paused','blocked','usage_limited','budget_limited','complete','dropped')),
                     blocked_reason TEXT,
                     blocked_rounds INTEGER NOT NULL DEFAULT 0,
                     round INTEGER NOT NULL DEFAULT 0,
                     max_rounds INTEGER NOT NULL DEFAULT 8,
                     token_budget INTEGER,
                     tokens_used INTEGER NOT NULL DEFAULT 0,
                     time_used_seconds INTEGER NOT NULL DEFAULT 0,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_goals_session_status
                     ON goals(session_id, status, created_at);
                 -- §8.9 messages 分行（阶段一：双写 + get_record 读切换）。
                 -- message_id 来自消息 JSON 的 id 字段（user:{turn} /
                 -- assistant:{turn}[:segment-N]），seq 保存会话内顺序。
                 -- sessions.messages 列在过渡期始终完整（回滚安全网）。
                 CREATE TABLE IF NOT EXISTS session_messages (
                     session_id TEXT NOT NULL,
                     message_id TEXT NOT NULL,
                     seq INTEGER NOT NULL,
                     message TEXT NOT NULL,
                     PRIMARY KEY(session_id, message_id)
                 );
                 CREATE INDEX IF NOT EXISTS idx_session_messages_order
                     ON session_messages(session_id, seq);
                 ",
            )
            .map_err(|error| format!("无法初始化历史数据库: {error}"))?;
        self.ensure_column("metadata", "TEXT NOT NULL DEFAULT '{}'")?;
        self.ensure_column("schema_version", "INTEGER NOT NULL DEFAULT 1")?;
        self.ensure_column("events", "TEXT")?;
        initialize_turn_recovery(&self.connection)?;
        self.ensure_table_column("turn_recovery", "settled_receipt", "TEXT")?;
        self.migrate_legacy_column_name("conversation_checkpoints", "dsh_cut", "checkpoint_cut", "TEXT")?;
        self.ensure_table_column("conversation_checkpoints", "checkpoint_cut", "TEXT")?;
        self.ensure_table_column("conversation_checkpoints", "model_transcript", "BLOB")?;
        let message_count_added =
            self.ensure_column("message_count", "INTEGER NOT NULL DEFAULT 0")?;
        if message_count_added {
            self.backfill_message_counts()?;
        }
        self.ensure_table_column(
            "collaboration_jobs",
            "agent_config",
            "TEXT NOT NULL DEFAULT '{}'",
        )?;
        self.ensure_table_column("collaboration_jobs", "aggregate_summary", "TEXT")?;
        // G2 预算归属：单委托 job 关联的 goal——turn 的 token 花费据此回写
        // goals.tokens_used（update_goal_status 之外独立的用量累计通道）。
        self.ensure_table_column("collaboration_jobs", "goal_id", "TEXT")?;
        self.ensure_table_column(
            "collaboration_jobs",
            "cancel_requested",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.ensure_table_column(
            "collaboration_agents",
            "attempt",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.ensure_table_column("collaboration_agents", "lease_id", "TEXT")?;
        self.ensure_table_column("collaboration_agents", "lease_owner", "TEXT")?;
        self.ensure_table_column("collaboration_agents", "lease_expires_at", "INTEGER")?;
        self.ensure_table_column(
            "collaboration_agents",
            "cancel_requested",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.connection
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_collaboration_agents_lease
                 ON collaboration_agents(status, lease_expires_at)",
                [],
            )
            .map_err(|error| format!("无法创建协作 Agent lease 索引: {error}"))?;
        Ok(())
    }

    pub(super) fn commit_transaction(&self, action: &str) -> Result<(), String> {
        if let Err(error) = self.connection.execute_batch("COMMIT") {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(format!("{action}: {error}"));
        }
        Ok(())
    }

    pub(super) fn ensure_column(&self, name: &str, definition: &str) -> Result<bool, String> {
        self.ensure_table_column("sessions", name, definition)
    }

    fn table_exists(&self, table: &str) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))
    }

    /// Rename a legacy table into its current name, once.
    ///
    /// SQLite's `ALTER TABLE ... RENAME TO` carries the rows, indexes and
    /// constraints over, so this is a metadata change rather than a copy. A
    /// table that is already renamed is left alone, and a fresh database never
    /// sees the legacy name at all, so the migration is idempotent.
    fn rename_legacy_table(&self, legacy: &str, current: &str) -> Result<(), String> {
        if !self.table_exists(legacy)? || self.table_exists(current)? {
            return Ok(());
        }
        self.connection
            .execute_batch(&format!("ALTER TABLE {legacy} RENAME TO {current}"))
            .map_err(|error| format!("无法重命名历史数据表 {legacy}: {error}"))
    }

    /// `dsh_*` 是移植期的内部代号，不是对外契约；落库也要用产品自己的名字。
    /// 旧库在这里原地改名，数据与索引都保留。
    pub(super) fn migrate_legacy_table_names(&self) -> Result<(), String> {
        // 字面量必须是旧名，不能被批量重命名波及。
        self.rename_legacy_table("dsh_outbox", "turn_outbox")?;
        self.rename_legacy_table("dsh_sessions", "session_bindings")?;
        self.rename_legacy_table("dsh_recovery", "turn_recovery")
    }

    /// Same idea for a single column. `ADD COLUMN` cannot carry the old value,
    /// so copy it across and only then drop the legacy column.
    ///
    /// Both columns may coexist: `ensure_table_column` elsewhere adds the new
    /// one unconditionally, and a previously interrupted run may have left the
    /// pair behind. So the "already migrated" test is only "no legacy column".
    fn migrate_legacy_column_name(
        &self,
        table: &str,
        legacy: &str,
        current: &str,
        definition: &str,
    ) -> Result<bool, String> {
        if !self.table_exists(table)? {
            return Ok(false);
        }
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?;
        if !columns.iter().any(|column| column == legacy) {
            return Ok(false);
        }
        let add = if columns.iter().any(|column| column == current) {
            String::new()
        } else {
            format!("ALTER TABLE {table} ADD COLUMN {current} {definition};")
        };
        self.connection
            .execute_batch(&format!(
                "{add}
                 UPDATE {table} SET {current} = {legacy} WHERE {legacy} IS NOT NULL;
                 ALTER TABLE {table} DROP COLUMN {legacy}"
            ))
            .map_err(|error| format!("无法重命名历史数据列 {table}.{legacy}: {error}"))?;
        Ok(true)
    }

    pub(super) fn ensure_table_column(
        &self,
        table: &str,
        name: &str,
        definition: &str,
    ) -> Result<bool, String> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("无法检查历史数据库结构: {error}"))?;
        if columns.iter().any(|column| column == name) {
            return Ok(false);
        }
        self.connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
                [],
            )
            .map_err(|error| format!("无法升级历史数据库: {error}"))?;
        Ok(true)
    }

    pub(super) fn backfill_message_counts(&self) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare("SELECT id, messages FROM sessions")
            .map_err(|error| format!("无法读取历史消息数量: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取历史消息数量: {error}"))?;
        let records = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("历史消息数量格式无效: {error}"))?;
        for (id, messages) in records {
            let count = serde_json::from_str::<Value>(&messages)
                .ok()
                .and_then(|value| value.as_array().map(|items| items.len() as i64))
                .unwrap_or(0);
            self.connection
                .execute(
                    "UPDATE sessions SET message_count = ?1 WHERE id = ?2",
                    params![count, id],
                )
                .map_err(|error| format!("无法保存历史消息数量: {error}"))?;
        }
        Ok(())
    }
}
