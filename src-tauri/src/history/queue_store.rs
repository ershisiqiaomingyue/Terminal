use super::*;

impl HistoryStore {
    pub(super) fn agent_queue_item(
        &self,
        session_id: &str,
        queue_id: &str,
    ) -> Result<Option<AgentQueueItem>, String> {
        self.connection
            .query_row(
                "SELECT id, session_id, idempotency_key, request, execution_target, status,
                        sequence, attempt_count, last_error, enqueued_at, updated_at
                 FROM agent_queue WHERE session_id = ?1 AND id = ?2",
                params![session_id, queue_id],
                row_to_agent_queue_item,
            )
            .optional()
            .map_err(|error| format!("无法读取 Agent 队列消息: {error}"))
    }

    pub(super) fn list_agent_queue_items(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<AgentQueueItem>, String> {
        let (sql, bind_session) = if session_id.is_some() {
            (
                "SELECT id, session_id, idempotency_key, request, execution_target, status,
                        sequence, attempt_count, last_error, enqueued_at, updated_at
                 FROM agent_queue
                 WHERE session_id = ?1 AND status IN ('pending', 'claimed')
                 ORDER BY sequence ASC",
                true,
            )
        } else {
            (
                "SELECT id, session_id, idempotency_key, request, execution_target, status,
                        sequence, attempt_count, last_error, enqueued_at, updated_at
                 FROM agent_queue
                 WHERE status IN ('pending', 'claimed')
                 ORDER BY sequence ASC",
                false,
            )
        };
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(|error| format!("无法读取 Agent 队列: {error}"))?;
        let mapped = if bind_session {
            statement.query_map(params![session_id], row_to_agent_queue_item)
        } else {
            statement.query_map([], row_to_agent_queue_item)
        }
        .map_err(|error| format!("无法读取 Agent 队列: {error}"))?;
        mapped
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("Agent 队列数据无效: {error}"))
    }

    pub(super) fn update_agent_queue_content(
        &self,
        session_id: &str,
        queue_id: &str,
        content: &str,
    ) -> Result<bool, String> {
        if content.trim().is_empty() {
            return Err("排队消息不能为空".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列: {error}"))?;
        let result = (|| {
            let item = self
                .agent_queue_item(session_id, queue_id)?
                .ok_or_else(|| "排队消息不存在".to_string())?;
            if item.status != "pending" {
                return Err("消息已经开始发送，不能再编辑".to_string());
            }
            let mut request = item.request;
            request.message = content.to_string();
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue SET request = ?1, updated_at = ?2
                 WHERE session_id = ?3 AND id = ?4 AND status = 'pending'",
                    params![
                        serde_json::to_string(&request).map_err(|error| error.to_string())?,
                        now_ms(),
                        session_id,
                        queue_id
                    ],
                )
                .map_err(|error| format!("无法编辑排队消息: {error}"))?;
            Ok(changed == 1)
        })();
        match result {
            Ok(changed) => {
                self.commit_transaction("无法提交排队消息编辑")?;
                Ok(changed)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn reorder_agent_queue_items(
        &self,
        session_id: &str,
        queue_ids: &[String],
    ) -> Result<bool, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列: {error}"))?;
        let result = (|| {
            let pending = {
                let mut statement = self.connection.prepare(
                    "SELECT id, sequence FROM agent_queue
                     WHERE session_id = ?1 AND status = 'pending'
                     ORDER BY sequence ASC",
                ).map_err(|error| format!("无法读取待发送队列: {error}"))?;
                let rows = statement.query_map([session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                }).map_err(|error| format!("无法读取待发送队列: {error}"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| format!("待发送队列无效: {error}"))?;
                rows
            };
            if pending.len() != queue_ids.len() {
                return Err("队列正在发送或已变化，请稍后重试。".to_string());
            }
            let original_ids = pending.iter().map(|(id, _)| id.as_str()).collect::<HashSet<_>>();
            let requested_ids = queue_ids.iter().map(String::as_str).collect::<HashSet<_>>();
            if requested_ids.len() != queue_ids.len() || original_ids != requested_ids {
                return Err("待发送队列已变化，请重试。".to_string());
            }
            if pending.iter().map(|(id, _)| id).eq(queue_ids.iter()) {
                return Ok(false);
            }
            let slots = pending.iter().map(|(_, sequence)| *sequence).collect::<Vec<_>>();
            for (id, sequence) in &pending {
                self.connection.execute(
                    "UPDATE agent_queue SET sequence = ?1 WHERE session_id = ?2 AND id = ?3 AND status = 'pending'",
                    params![-sequence, session_id, id],
                ).map_err(|error| format!("无法暂存队列顺序: {error}"))?;
            }
            let timestamp = now_ms();
            for (id, sequence) in queue_ids.iter().zip(slots) {
                self.connection.execute(
                    "UPDATE agent_queue SET sequence = ?1, updated_at = ?2
                     WHERE session_id = ?3 AND id = ?4 AND status = 'pending'",
                    params![sequence, timestamp, session_id, id],
                ).map_err(|error| format!("无法保存队列顺序: {error}"))?;
            }
            Ok(true)
        })();
        match result {
            Ok(changed) => {
                self.commit_transaction("无法提交 Agent 队列顺序")?;
                Ok(changed)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn enqueue_agent_queue_item(
        &self,
        session_id: &str,
        queue_id: &str,
        idempotency_key: &str,
        request: &AgentChatRequest,
    ) -> Result<AgentQueueEnqueueResult, String> {
        let session_id = session_id.trim();
        let queue_id = queue_id.trim();
        let idempotency_key = idempotency_key.trim();
        if session_id.is_empty() || queue_id.is_empty() || idempotency_key.is_empty() {
            return Err("Agent 队列必须包含会话、消息 id 和幂等键".to_string());
        }
        if request
            .session_id
            .as_deref()
            .is_some_and(|request_session| request_session != session_id)
        {
            return Err("Agent 队列请求的会话与队列会话不一致".to_string());
        }
        let mut frozen_request = request.clone();
        frozen_request.session_id = Some(session_id.to_string());
        let request_json = serde_json::to_string(&frozen_request)
            .map_err(|error| format!("无法序列化 Agent 队列请求: {error}"))?;
        let execution_target = frozen_request
            .execution_target
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("无法保存 Agent 队列执行目标: {error}"))?;
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列: {error}"))?;
        let result = (|| {
            let existing = self
                .connection
                .query_row(
                    "SELECT id, session_id, idempotency_key, request, execution_target, status,
                            sequence, attempt_count, last_error, enqueued_at, updated_at
                     FROM agent_queue WHERE session_id = ?1 AND idempotency_key = ?2",
                    params![session_id, idempotency_key],
                    row_to_agent_queue_item,
                )
                .optional()
                .map_err(|error| format!("无法检查 Agent 队列幂等键: {error}"))?;
            if let Some(existing) = existing {
                let same_request = serde_json::to_value(&existing.request).ok()
                    == serde_json::to_value(&frozen_request).ok();
                if !same_request {
                    return Err("Agent 队列幂等键已绑定到另一条请求".to_string());
                }
                return Ok(AgentQueueEnqueueResult {
                    item: existing,
                    inserted: false,
                });
            }
            let active_count = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_queue
                     WHERE session_id = ?1 AND status IN ('pending', 'claimed')",
                    params![session_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| format!("无法检查 Agent 队列容量: {error}"))?;
            if active_count >= MAX_AGENT_QUEUE_ITEMS_PER_SESSION {
                return Err("待发送队列最多保留 20 条消息。".to_string());
            }
            let sequence = self
                .connection
                .query_row(
                    "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_queue",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| format!("无法分配 Agent 队列顺序: {error}"))?;
            let timestamp = now_ms();
            self.connection
                .execute(
                    "INSERT INTO agent_queue
                        (id, session_id, idempotency_key, request, execution_target, status,
                         sequence, attempt_count, enqueued_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, 0, ?7, ?7)",
                    params![
                        queue_id,
                        session_id,
                        idempotency_key,
                        request_json,
                        execution_target,
                        sequence,
                        timestamp,
                    ],
                )
                .map_err(|error| format!("无法保存 Agent 队列消息: {error}"))?;
            Ok(AgentQueueEnqueueResult {
                item: AgentQueueItem {
                    id: queue_id.to_string(),
                    session_id: session_id.to_string(),
                    idempotency_key: idempotency_key.to_string(),
                    request: frozen_request,
                    status: "pending".to_string(),
                    sequence,
                    attempt_count: 0,
                    enqueued_at: timestamp,
                    updated_at: timestamp,
                    last_error: None,
                },
                inserted: true,
            })
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交 Agent 队列消息")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn recover_legacy_orphaned_queue_dispatches_locked(
        &self,
        timestamp: i64,
    ) -> Result<(), String> {
        let orphaned_dispatched = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT q.id, q.session_id, q.request
                     FROM agent_queue q
                     WHERE q.status = 'dispatched'",
                )
                .map_err(|error| format!("无法读取旧版 Agent 队列确认: {error}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| format!("无法读取旧版 Agent 队列确认: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("旧版 Agent 队列确认无效: {error}"))?;
            rows
        };
        for (queue_id, session_id, request_json) in orphaned_dispatched {
            let turn_id = serde_json::from_str::<AgentChatRequest>(&request_json)
                .ok()
                .and_then(|request| request.turn_id)
                .filter(|value| !value.trim().is_empty());
            let has_run = if let Some(turn_id) = turn_id.as_deref() {
                self.connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM agent_runs
                         WHERE session_id = ?1 AND turn_id = ?2)",
                        params![session_id, turn_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|error| format!("无法核对 Agent 队列运行记录: {error}"))?
            } else {
                false
            };
            if !has_run {
                self.connection
                    .execute(
                        "UPDATE agent_queue
                         SET status = 'pending', completed_at = NULL, updated_at = ?1,
                             last_error = '恢复了旧版在运行记录创建前确认的队列消息'
                         WHERE id = ?2 AND session_id = ?3 AND status = 'dispatched'",
                        params![timestamp, queue_id, session_id],
                    )
                    .map_err(|error| format!("无法恢复旧版 Agent 队列消息: {error}"))?;
            }
        }
        Ok(())
    }

    pub(super) fn claim_agent_queue_item(
        &self,
        session_id: &str,
        queue_id: Option<&str>,
        lease_owner: &str,
    ) -> Result<Option<AgentQueueLease>, String> {
        if session_id.trim().is_empty() || lease_owner.trim().is_empty() {
            return Err("领取 Agent 队列消息必须指定会话和 lease owner".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            self.connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'pending', lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?1,
                         last_error = COALESCE(last_error, '队列 lease 超时，已重新排队')
                     WHERE session_id = ?2 AND status = 'claimed'
                       AND lease_expires_at IS NOT NULL AND lease_expires_at <= ?1",
                    params![timestamp, session_id],
                )
                .map_err(|error| format!("无法回收过期 Agent 队列 lease: {error}"))?;
            let already_claimed = self
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM agent_queue
                     WHERE session_id = ?1 AND status = 'claimed')",
                    params![session_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| format!("无法检查 Agent 队列 lease: {error}"))?;
            if already_claimed {
                return Ok(None);
            }
            let selected_id = if let Some(queue_id) = queue_id {
                self.connection
                    .query_row(
                        "SELECT id FROM agent_queue
                         WHERE session_id = ?1 AND id = ?2 AND status = 'pending'",
                        params![session_id, queue_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|error| format!("无法读取指定 Agent 队列消息: {error}"))?
            } else {
                self.connection
                    .query_row(
                        "SELECT id FROM agent_queue
                         WHERE session_id = ?1 AND status = 'pending'
                         ORDER BY sequence ASC LIMIT 1",
                        params![session_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|error| format!("无法读取下一条 Agent 队列消息: {error}"))?
            };
            let Some(selected_id) = selected_id else {
                return Ok(None);
            };
            let lease_id = next_history_id("queue-lease");
            let lease_expires_at = timestamp.saturating_add(AGENT_QUEUE_LEASE_MS);
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'claimed', attempt_count = attempt_count + 1,
                         lease_id = ?1, lease_owner = ?2, lease_expires_at = ?3,
                         last_error = NULL, updated_at = ?4
                     WHERE session_id = ?5 AND id = ?6 AND status = 'pending'",
                    params![
                        lease_id,
                        lease_owner,
                        lease_expires_at,
                        timestamp,
                        session_id,
                        selected_id,
                    ],
                )
                .map_err(|error| format!("无法领取 Agent 队列消息: {error}"))?;
            if changed != 1 {
                return Err("Agent 队列消息已被其他调度器领取".to_string());
            }
            let item = self
                .agent_queue_item(session_id, &selected_id)?
                .ok_or_else(|| "领取后的 Agent 队列消息无法读取".to_string())?;
            Ok(Some(AgentQueueLease {
                item,
                lease_id,
                lease_owner: lease_owner.to_string(),
                lease_expires_at,
            }))
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交 Agent 队列领取")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn ack_agent_queue_item(
        &self,
        session_id: &str,
        queue_id: &str,
        lease_id: &str,
        lease_owner: &str,
        disposition: &str,
    ) -> Result<AgentQueueItem, String> {
        if !matches!(disposition, "dispatched" | "steered" | "removed") {
            return Err("Agent 队列完成状态无效".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列确认: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue
                     SET status = ?1, lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?2, completed_at = ?2
                     WHERE session_id = ?3 AND id = ?4 AND status = 'claimed'
                       AND lease_id = ?5 AND lease_owner = ?6",
                    params![
                        disposition,
                        timestamp,
                        session_id,
                        queue_id,
                        lease_id,
                        lease_owner,
                    ],
                )
                .map_err(|error| format!("无法确认 Agent 队列消息: {error}"))?;
            let item = self
                .agent_queue_item(session_id, queue_id)?
                .ok_or_else(|| "Agent 队列消息不存在".to_string())?;
            if changed == 0 && item.status != disposition {
                return Err("Agent 队列 lease 已失效或不属于当前调度器".to_string());
            }
            Ok(item)
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交 Agent 队列确认")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn nack_agent_queue_item(
        &self,
        session_id: &str,
        queue_id: &str,
        lease_id: &str,
        lease_owner: &str,
        error: Option<&str>,
    ) -> Result<AgentQueueItem, String> {
        let error = error.map(|value| value.chars().take(2_000).collect::<String>());
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列退回: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'pending', lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, last_error = ?1, updated_at = ?2
                     WHERE session_id = ?3 AND id = ?4 AND status = 'claimed'
                       AND lease_id = ?5 AND lease_owner = ?6",
                    params![
                        error,
                        timestamp,
                        session_id,
                        queue_id,
                        lease_id,
                        lease_owner,
                    ],
                )
                .map_err(|error| format!("无法退回 Agent 队列消息: {error}"))?;
            let item = self
                .agent_queue_item(session_id, queue_id)?
                .ok_or_else(|| "Agent 队列消息不存在".to_string())?;
            if changed == 0 && item.status != "pending" {
                return Err("Agent 队列 lease 已失效或不属于当前调度器".to_string());
            }
            Ok(item)
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交 Agent 队列退回")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn recover_agent_queue_items(
        &self,
        lease_owner: &str,
    ) -> Result<Vec<AgentQueueItem>, String> {
        self.recover_agent_queue_leases(lease_owner, |store| store.list_agent_queue_items(None))
    }

    pub(super) fn recover_legacy_queue_items(
        &self,
        lease_owner: &str,
    ) -> Result<Vec<(String, Result<AgentQueueItem, String>)>, String> {
        self.recover_agent_queue_leases(lease_owner, |store| {
            let mut statement = store
                .connection
                .prepare(
                    "SELECT id, session_id, idempotency_key, request, execution_target, status,
                        sequence, attempt_count, last_error, enqueued_at, updated_at
                 FROM agent_queue WHERE status IN ('pending', 'claimed')
                 ORDER BY sequence ASC, id ASC",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    let session = row.get::<_, String>(1)?;
                    let id = row.get::<_, String>(0)?;
                    let item = row_to_agent_queue_item(row)
                        .map_err(|error| format!("{id}: {error}"))
                        .and_then(|item| {
                            if item
                                .request
                                .session_id
                                .as_deref()
                                .is_some_and(|owner| owner != session)
                            {
                                return Err(format!("{id}: 旧队列请求属于另一会话，未迁移"));
                            }
                            Ok(item)
                        });
                    Ok((session, item))
                })
                .map_err(|error| error.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| error.to_string())
        })
    }

    pub(super) fn recover_agent_queue_leases<T>(
        &self,
        lease_owner: &str,
        read: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if lease_owner.trim().is_empty() {
            return Err("恢复 Agent 队列必须指定当前 lease owner".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 队列恢复: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            // Older builds acknowledged `dispatched` before inserting the run.
            // Repair those orphaned rows once during startup recovery rather
            // than scanning the terminal queue history on every claim.
            self.recover_legacy_orphaned_queue_dispatches_locked(timestamp)?;
            self.connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'pending', lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?1,
                         last_error = COALESCE(last_error, '应用重启后恢复未确认的队列消息')
                     WHERE status = 'claimed'
                       AND (lease_owner IS NULL OR lease_owner != ?2
                            OR lease_expires_at IS NULL OR lease_expires_at <= ?1)",
                    params![timestamp, lease_owner],
                )
                .map_err(|error| format!("无法恢复 Agent 队列 lease: {error}"))?;
            read(self)
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交 Agent 队列恢复")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn agent_steer(
        &self,
        steer_id: &str,
    ) -> Result<Option<PersistedAgentSteer>, String> {
        self.connection
            .query_row(
                "SELECT id, session_id, turn_id, request, status, source_queue_id, created_at
                 FROM agent_steers WHERE id = ?1",
                params![steer_id],
                |row| {
                    let request =
                        serde_json::from_str::<AgentSteerRequest>(&row.get::<_, String>(3)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?;
                    Ok(PersistedAgentSteer {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        turn_id: row.get(2)?,
                        request,
                        status: row.get(4)?,
                        source_queue_id: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("无法读取持久化追加要求: {error}"))
    }

    pub(super) fn enqueue_agent_steer(
        &self,
        session_id: &str,
        turn_id: &str,
        request: &AgentSteerRequest,
    ) -> Result<PersistedAgentSteer, String> {
        let steer_id = next_history_id("steer");
        let timestamp = now_ms();
        self.connection
            .execute(
                "INSERT INTO agent_steers
                    (id, session_id, turn_id, request, source_queue_id, status, created_at, updated_at, applied_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, 'pending', ?5, ?5, NULL)",
                params![
                    steer_id,
                    session_id,
                    turn_id,
                    serde_json::to_string(request)
                        .map_err(|error| format!("无法序列化追加要求: {error}"))?,
                    timestamp,
                ],
            )
            .map_err(|error| format!("无法保存追加要求: {error}"))?;
        self.agent_steer(&steer_id)?
            .ok_or_else(|| "追加要求保存后无法读取".to_string())
    }

    pub(super) fn persist_queue_steer_and_ack(
        &self,
        session_id: &str,
        turn_id: &str,
        request: &AgentSteerRequest,
        queue_id: &str,
        lease_id: &str,
        lease_owner: &str,
    ) -> Result<PersistedAgentSteer, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定队列追加要求: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            let steer_id = next_history_id("steer");
            self.connection
                .execute(
                    "INSERT INTO agent_steers
                        (id, session_id, turn_id, request, source_queue_id, status, created_at, updated_at, applied_at)
                     SELECT ?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?6, NULL
                     WHERE EXISTS(
                         SELECT 1 FROM agent_queue
                         WHERE id = ?5 AND session_id = ?2 AND status = 'claimed'
                           AND lease_id = ?7 AND lease_owner = ?8
                     )",
                    params![
                        steer_id,
                        session_id,
                        turn_id,
                        serde_json::to_string(request)
                            .map_err(|error| format!("无法序列化队列追加要求: {error}"))?,
                        queue_id,
                        timestamp,
                        lease_id,
                        lease_owner,
                    ],
                )
                .map_err(|error| format!("无法保存队列追加要求: {error}"))?;
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'steered', lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?1, completed_at = ?1
                     WHERE session_id = ?2 AND id = ?3 AND status = 'claimed'
                       AND lease_id = ?4 AND lease_owner = ?5",
                    params![timestamp, session_id, queue_id, lease_id, lease_owner],
                )
                .map_err(|error| format!("无法确认队列追加要求: {error}"))?;
            if changed != 1 {
                return Err("Agent 队列 lease 已失效或不属于当前调度器".to_string());
            }
            self.agent_steer(&steer_id)?
                .ok_or_else(|| "队列追加要求保存后无法读取".to_string())
        })();
        match result {
            Ok(steer) => {
                self.commit_transaction("无法提交队列追加要求")?;
                Ok(steer)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn pending_agent_steers(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedAgentSteer>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, session_id, turn_id, request, status, source_queue_id, created_at
                 FROM agent_steers AS steer
                 WHERE steer.session_id = ?1 AND steer.status IN ('pending', 'delivered')
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_runs AS run
                       WHERE run.session_id = steer.session_id AND run.turn_id = steer.turn_id
                         AND run.status IN ('completed', 'cancelled', 'failed')
                   )
                 ORDER BY steer.created_at ASC, steer.id ASC",
            )
            .map_err(|error| format!("无法读取待应用追加要求: {error}"))?;
        let rows = statement
            .query_map(params![session_id], |row| {
                let request = serde_json::from_str::<AgentSteerRequest>(&row.get::<_, String>(3)?)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                Ok(PersistedAgentSteer {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    request,
                    status: row.get(4)?,
                    source_queue_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|error| format!("无法读取待应用追加要求: {error}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("待应用追加要求无效: {error}"))
    }

    pub(super) fn update_agent_steer_status(
        &self,
        steer_id: &str,
        status: &str,
    ) -> Result<(), String> {
        if !matches!(status, "delivered" | "applied" | "rejected") {
            return Err("追加要求状态无效".to_string());
        }
        let timestamp = now_ms();
        let changed = self
            .connection
            .execute(
                "UPDATE agent_steers
                 SET status = ?1, updated_at = ?2,
                     applied_at = CASE WHEN ?1 = 'applied' THEN ?2 ELSE applied_at END
                 WHERE id = ?3 AND status IN ('pending', 'delivered')",
                params![status, timestamp, steer_id],
            )
            .map_err(|error| format!("无法更新追加要求状态: {error}"))?;
        if changed == 0 {
            let existing = self.agent_steer(steer_id)?;
            if existing.as_ref().map(|item| item.status.as_str()) != Some(status) {
                return Err("追加要求已处于另一终态".to_string());
            }
        }
        Ok(())
    }

    /// A steer accepted just as its target turn is cancelling has no live
    /// channel to consume it. Retire that row only when the durable turn is
    /// already terminal; interrupted turns remain pending for resume.
    pub(super) fn reject_agent_steer_if_terminal(&self, steer_id: &str) -> Result<bool, String> {
        let changed = self
            .connection
            .execute(
                "UPDATE agent_steers AS steer
                 SET status = 'rejected', updated_at = ?1
                 WHERE steer.id = ?2 AND steer.status IN ('pending', 'delivered')
                   AND EXISTS (
                       SELECT 1 FROM agent_runs AS run
                       WHERE run.session_id = steer.session_id
                         AND run.turn_id = steer.turn_id
                         AND run.status IN ('completed', 'cancelled', 'failed')
                   )",
                params![now_ms(), steer_id],
            )
            .map_err(|error| format!("无法回收已结束轮次的追加要求: {error}"))?;
        Ok(changed == 1)
    }

    /// N6-B：turn 终态落库成功后统一回收该轮所有未应用（pending/delivered，
    /// 从未 applied）的追加要求。此前这些行要等下一轮 replay 才被逐条检查，
    /// 用户停止后不再发消息就永久悬挂。调用方保证 run 已是终态。
    pub(super) fn reject_agent_steers_for_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Vec<String>, String> {
        let steer_ids: Vec<String> = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT steer.id FROM agent_steers AS steer
                     WHERE steer.session_id = ?1 AND steer.turn_id = ?2
                       AND steer.status IN ('pending', 'delivered')",
                )
                .map_err(|error| format!("无法读取待回收的追加要求: {error}"))?;
            let rows = statement
                .query_map(params![session_id, turn_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取待回收的追加要求: {error}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("无法读取待回收的追加要求: {error}"))?
        };
        if steer_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.connection
            .execute(
                "UPDATE agent_steers
                 SET status = 'rejected', updated_at = ?3
                 WHERE session_id = ?1 AND turn_id = ?2 AND status IN ('pending', 'delivered')",
                params![session_id, turn_id, now_ms()],
            )
            .map_err(|error| format!("无法回收追加要求: {error}"))?;
        Ok(steer_ids)
    }

    /// N6-B（缺口补齐）：只读查询某轮尚未收口的追加要求 id。终态落库失败时
    /// 走不了 `reject_agent_steers_for_turn`（同一个库写不进），但「本轮已结束」
    /// 对用户是确定的，仍要逐条广播 `steer_rejected`；库由重启后的对账收口。
    pub(super) fn pending_agent_steer_ids_for_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT steer.id FROM agent_steers AS steer
                 WHERE steer.session_id = ?1 AND steer.turn_id = ?2
                   AND steer.status IN ('pending', 'delivered')
                 ORDER BY steer.created_at ASC, steer.id ASC",
            )
            .map_err(|error| format!("无法读取待回收的追加要求: {error}"))?;
        let rows = statement
            .query_map(params![session_id, turn_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取待回收的追加要求: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法读取待回收的追加要求: {error}"))
    }

    pub(super) fn mark_next_agent_steer_applied(
        &self,
        session_id: &str,
        request: &AgentSteerRequest,
    ) -> Result<Option<String>, String> {
        // The runtime adds a deterministic source_message_id before sending
        // the request through the in-memory channel. That transport-only
        // field is intentionally absent from the durable row, so exclude it
        // from the identity match or every delivered steer would remain
        // pending forever.
        let mut durable_request = request.clone();
        durable_request.source_message_id = None;
        let encoded = serde_json::to_string(&durable_request)
            .map_err(|error| format!("无法序列化已应用的追加要求: {error}"))?;
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定追加要求应用状态: {error}"))?;
        let result = (|| {
            let steer_id = self
                .connection
                .query_row(
                    "SELECT id FROM agent_steers
                     WHERE session_id = ?1 AND request = ?2
                       AND status IN ('pending', 'delivered')
                     ORDER BY created_at ASC, id ASC LIMIT 1",
                    params![session_id, encoded],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| format!("无法查找已应用的追加要求: {error}"))?;
            let Some(steer_id) = steer_id else {
                return Ok(None);
            };
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_steers
                     SET status = 'applied', updated_at = ?1, applied_at = ?1
                     WHERE id = ?2 AND status IN ('pending', 'delivered')",
                    params![now_ms(), steer_id],
                )
                .map_err(|error| format!("无法确认追加要求已应用: {error}"))?;
            if changed != 1 {
                return Err("追加要求在应用时已被其他执行器处理".to_string());
            }
            Ok(Some(steer_id))
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交追加要求应用状态")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}
