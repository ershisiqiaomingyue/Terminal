use super::*;

impl HistoryStore {
    pub fn upsert(&self, session: &HistorySession) -> Result<(), String> {
        self.upsert_record(&HistorySessionRecord::from(session))
    }

    pub fn upsert_input(&self, session: HistorySessionInput) -> Result<(), String> {
        self.upsert_record(&session.into())
    }

    pub(super) fn upsert_record(&self, session: &HistorySessionRecord) -> Result<(), String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法开始保存历史会话: {error}"))?;
        match self.write_record(session) {
            Ok(()) => self.commit_transaction("无法提交历史会话"),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn write_record(&self, session: &HistorySessionRecord) -> Result<(), String> {
        if let Some(binding) = self.session_binding(&session.id)? {
            if binding.storage_id != session.id
                && !binding.deleted
                && session
                    .metadata
                    .get("_storageBindingId")
                    // 改名前的键也认，否则老快照会被误判成过期版本。
                    .or_else(|| session.metadata.get("_dshStorageId"))
                    .and_then(Value::as_str)
                    != Some(binding.storage_id.as_str())
            {
                return Err("对话历史版本已变更，未用旧界面快照覆盖，请重新打开该会话。".into());
            }
        }
        let events = collect_session_events(session);
        // The event table owns full history, including imported backups. Keep
        // the compatibility JSON column small and exclude lazy process items.
        let event_snapshot = session.events.as_ref().map(|value| match value.as_array() {
            Some(events) => {
                let mut recent = events
                    .iter()
                    .rev()
                    .filter(|event| {
                        event.get("type").and_then(Value::as_str) != Some("process_item")
                    })
                    .take(MAX_RESTORED_SESSION_EVENTS)
                    .cloned()
                    .collect::<Vec<_>>();
                recent.reverse();
                Value::Array(recent)
            }
            None => value.clone(),
        });
        // Reads return a bounded event window. Saving that window must never
        // delete events the client did not load; deletion belongs to delete().
        let merge_persisted_events = session.events.is_some()
            || matches!(&session.messages, Value::Array(messages) if messages.iter().any(|message| message.get("events").is_some()));
        self.ensure_not_deleted(&session.id)?;
        let affected = self
                .connection
                .execute(
                    "INSERT INTO sessions
                        (id, title, updated_at, archived, jira_context, artifacts, messages, metadata, report, usage, schema_version, events, message_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(id) DO UPDATE SET
                        title = excluded.title,
                        updated_at = excluded.updated_at,
                        archived = excluded.archived,
                        jira_context = excluded.jira_context,
                        artifacts = excluded.artifacts,
                        messages = excluded.messages,
                        metadata = excluded.metadata,
                        report = excluded.report,
                        usage = excluded.usage,
                        schema_version = MAX(excluded.schema_version, sessions.schema_version),
                        events = COALESCE(excluded.events, sessions.events),
                        message_count = excluded.message_count
                     WHERE sessions.updated_at <= excluded.updated_at",
                    params![
                        session.id,
                        session.title,
                        session.updated_at,
                        session.archived,
                        encode_optional(&session.jira_context)?,
                        encode(&session.artifacts)?,
                        encode(&session.messages)?,
                        encode(&session.metadata)?,
                        encode_optional(&session.report)?,
                        encode_optional(&session.usage)?,
                        normalize_schema_version(session.schema_version),
                        encode_optional(&event_snapshot)?,
                        json_array_len(&session.messages),
                    ],
                )
                .map_err(|error| format!("无法保存历史会话: {error}"))?;
        if merge_persisted_events && affected > 0 {
            self.merge_events(&session.id, &events)?;
        }
        // §8.9 阶段一：messages 分行双写。sessions.messages 列仍全量写（回滚
        // 安全网）；session_messages 只写有变化的行——大对话的写放大从
        // 「1MB×每轮」降为「几十 KB×每轮」发生在阶段二（读切换）之后。
        if affected > 0 {
            self.merge_session_messages(&session.id, &session.messages)?;
        }
        self.ensure_conversation_thread(&session.id)?;
        Ok::<(), String>(())
    }

    /// §8.9：按 message_id 做增量 diff——新/变的行 upsert，消失的行删除，
    /// seq 保存会话内顺序。**id 异常兜底**：数组里出现缺失 id 或重复 id 时
    /// 整体跳过（该会话保持列权威），绝不写出行序错乱的数据。
    pub(super) fn merge_session_messages(
        &self,
        session_id: &str,
        messages: &Value,
    ) -> Result<(), String> {
        let Some(array) = messages.as_array() else {
            return Ok(());
        };
        // id 完整性与唯一性检查（一次遍历）。
        let mut seen_ids: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(array.len());
        let mut rows: Vec<(usize, String, &Value)> = Vec::with_capacity(array.len());
        for (seq, message) in array.iter().enumerate() {
            let Some(id) = message.get("id").and_then(Value::as_str) else {
                return Ok(());
            };
            if id.is_empty() || !seen_ids.insert(id.to_string()) {
                return Ok(());
            }
            rows.push((seq, id.to_string(), message));
        }
        if rows.is_empty() {
            // 空会话（全部删除）：清掉该会话的行。
            self.connection
                .execute("DELETE FROM session_messages WHERE session_id = ?1", params![session_id])
                .map_err(|error| format!("无法清理分行消息: {error}"))?;
            return Ok(());
        }
        let existing: std::collections::HashSet<String> = {
            let mut statement = self
                .connection
                .prepare("SELECT message_id FROM session_messages WHERE session_id = ?1")
                .map_err(|error| format!("无法读取分行消息: {error}"))?;
            let ids = statement
                .query_map(params![session_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取分行消息: {error}"))?;
            ids.collect::<Result<std::collections::HashSet<_>, _>>()
                .map_err(|error| format!("无法读取分行消息: {error}"))?
        };
        let mut upsert = self
            .connection
            .prepare(
                "INSERT INTO session_messages(session_id, message_id, seq, message)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_id, message_id) DO UPDATE SET
                    seq = excluded.seq, message = excluded.message
                 WHERE session_messages.message <> excluded.message
                    OR session_messages.seq <> excluded.seq",
            )
            .map_err(|error| format!("无法准备分行消息写入: {error}"))?;
        for (seq, id, message) in &rows {
            upsert
                .execute(params![session_id, id, seq, serde_json::to_string(message).unwrap_or_default()])
                .map_err(|error| format!("无法写入分行消息: {error}"))?;
        }
        // 删除消失的行（消息被编辑/回退）。
        for id in existing.difference(&seen_ids) {
            self.connection
                .execute(
                    "DELETE FROM session_messages WHERE session_id = ?1 AND message_id = ?2",
                    params![session_id, id],
                )
                .map_err(|error| format!("无法删除分行消息: {error}"))?;
        }
        Ok(())
    }

    /// §8.9 读切换：新表有该会话的行时按 seq 聚合返回；空（旧会话/未迁移）
    /// 返回 None，调用方继续用 messages 列（下一次写会自动把新表补全）。
    pub(super) fn session_messages_rows(
        &self,
        session_id: &str,
    ) -> Result<Option<Value>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT message FROM session_messages WHERE session_id = ?1 ORDER BY seq ASC",
            )
            .map_err(|error| format!("无法读取分行消息: {error}"))?;
        let rows = statement
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取分行消息: {error}"))?;
        let mut messages = Vec::new();
        for row in rows {
            let text = row.map_err(|error| format!("无法读取分行消息: {error}"))?;
            match serde_json::from_str::<Value>(&text) {
                Ok(message) => messages.push(message),
                Err(_) => {
                    // 任何一行损坏都不部分返回——整体回退列权威。
                    return Ok(None);
                }
            }
        }
        Ok((!messages.is_empty()).then(|| Value::Array(messages)))
    }

    pub fn list(&self, archived: bool) -> Result<Vec<HistorySession>, String> {
        self.list_records(archived)
            .map(|records| records.into_iter().map(Into::into).collect())
    }

    pub fn list_records(&self, archived: bool) -> Result<Vec<HistorySessionRecord>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, title, updated_at, archived, jira_context, artifacts, messages, metadata, report, usage, schema_version, events
                 FROM sessions WHERE archived = ?1 ORDER BY updated_at DESC LIMIT 200",
            )
            .map_err(|error| format!("无法读取历史会话: {error}"))?;
        let rows = statement
            .query_map(params![archived], row_to_session_record)
            .map_err(|error| format!("无法读取历史会话: {error}"))?;
        let mut records = rows
            .map(|row| row.map_err(|error| format!("历史会话格式无效: {error}")))
            .collect::<Result<Vec<_>, _>>()?;
        self.hydrate_events(&mut records)?;
        Ok(records)
    }

    pub fn list_summaries(
        &self,
        archived: Option<bool>,
    ) -> Result<Vec<HistorySessionSummary>, String> {
        let (query, has_archived_filter) = if archived.is_some() {
            (
                "SELECT id, title, updated_at, archived, jira_context, metadata, message_count
              FROM sessions WHERE archived = ?1 ORDER BY updated_at DESC LIMIT 400",
                true,
            )
        } else {
            (
                "SELECT id, title, updated_at, archived, jira_context, metadata, message_count
              FROM sessions ORDER BY updated_at DESC LIMIT 400",
                false,
            )
        };
        let mut statement = self
            .connection
            .prepare(query)
            .map_err(|error| format!("无法读取历史会话摘要: {error}"))?;
        let rows = if has_archived_filter {
            statement
                .query_map(params![archived.unwrap_or(false)], row_to_session_summary)
                .map_err(|error| format!("无法读取历史会话摘要: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
        } else {
            statement
                .query_map([], row_to_session_summary)
                .map_err(|error| format!("无法读取历史会话摘要: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
        };
        rows.map_err(|error| format!("历史会话摘要格式无效: {error}"))
    }

    pub fn get_record(&self, id: &str) -> Result<Option<HistorySessionRecord>, String> {
        let Some(mut record) = self.get_record_raw(id)? else {
            return Ok(None);
        };
        // §8.9 读切换：分行表有该会话的行时以它为准（写侧已增量维护）；
        // 空则旧列兜底（旧会话首次写会自动迁移到分行表）。
        if let Some(messages) = self.session_messages_rows(id)? {
            record.messages = messages;
        }
        // Restore the bounded raw event stream as well as the message text.
        // Without hydration, reopening a conversation loses its tool timeline
        // even though the events were saved in session_events.
        self.hydrate_events(std::slice::from_mut(&mut record))?;
        Self::settle_interrupted_record_messages(&mut record);
        Ok(Some(record))
    }

    /// Read the durable session columns without walking the event table.
    /// History mutations use this form because they already know which event
    /// rows they are deleting and should not block on rebuilding a large tool
    /// timeline just to find the message being edited.
    pub(super) fn get_record_raw(&self, id: &str) -> Result<Option<HistorySessionRecord>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, title, updated_at, archived, jira_context, artifacts, messages, metadata, report, usage, schema_version, NULL AS events
                 FROM sessions WHERE id = ?1",
            )
            .map_err(|error| format!("无法读取历史会话: {error}"))?;
        let record = statement
            .query_row(params![id], row_to_session_record)
            .optional()
            .map_err(|error| format!("无法读取历史会话: {error}"))?;
        let Some(mut record) = record else {
            return Ok(None);
        };
        if let Some(binding) = self.session_binding(id)? {
            if binding.storage_id != id && record.metadata.is_object() {
                record.metadata["_storageBindingId"] = Value::String(binding.storage_id);
            }
        }
        record.agent_runs = self.list_agent_run_summaries(id)?;
        record.usage_totals = Some(self.session_usage_totals(id)?);
        Self::settle_interrupted_record_messages(&mut record);
        Ok(Some(record))
    }

    pub(super) fn settle_interrupted_record_messages(record: &mut HistorySessionRecord) {
        let Some(messages) = record.messages.as_array_mut() else {
            return;
        };
        for message in messages {
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let turn_id = message.get("turnId").or_else(|| message.get("turn_id")).and_then(Value::as_str);
            let Some(run) = record.agent_runs.iter().find(|run| {
                Some(run.turn_id.as_str()) == turn_id && run.status == "interrupted"
            }) else {
                continue;
            };
            // Freeze the display at the last received event. Keep the durable
            // run unfinished so explicit resume can still reuse its request.
            let last_event_time = message.get("events").and_then(Value::as_array)
                .into_iter().flatten()
                .filter_map(|event| event.get("timestamp_ms").or_else(|| event.get("timestampMs")).and_then(Value::as_i64))
                .max();
            let Some(message) = message.as_object_mut() else {
                continue;
            };
            message.insert("pending".into(), Value::Bool(false));
            if message.get("finishedAt").is_none_or(Value::is_null) {
                message.insert("finishedAt".into(), Value::from(last_event_time.unwrap_or(run.started_at).max(run.started_at)));
            }
        }
    }

    pub fn search(&self, query: &str) -> Result<Vec<HistorySession>, String> {
        self.search_records(query)
            .map(|records| records.into_iter().map(Into::into).collect())
    }

    pub fn search_records(&self, query: &str) -> Result<Vec<HistorySessionRecord>, String> {
        let pattern = format!("%{}%", query.trim());
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, title, updated_at, archived, jira_context, artifacts, messages, metadata, report, usage, schema_version, events
                 FROM sessions
                 WHERE title LIKE ?1 OR jira_context LIKE ?1 OR messages LIKE ?1 OR metadata LIKE ?1 OR report LIKE ?1 OR events LIKE ?1
                 ORDER BY updated_at DESC LIMIT 200",
            )
            .map_err(|error| format!("无法搜索历史会话: {error}"))?;
        let rows = statement
            .query_map(params![pattern], row_to_session_record)
            .map_err(|error| format!("无法搜索历史会话: {error}"))?;
        let mut records = rows
            .map(|row| row.map_err(|error| format!("历史会话格式无效: {error}")))
            .collect::<Result<Vec<_>, _>>()?;
        self.hydrate_events(&mut records)?;
        Ok(records)
    }

    pub fn set_archived(&self, id: &str, archived: bool) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE sessions SET archived = ?1 WHERE id = ?2",
                params![archived, id],
            )
            .map_err(|error| format!("无法更新历史会话: {error}"))?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.delete_with_attachments(id, &HashSet::new())
    }

    pub(super) fn delete_with_attachments(
        &self,
        id: &str,
        attachments: &HashSet<String>,
    ) -> Result<(), String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO session_bindings(session_id,storage_id,deleted) VALUES(?1,?1,1)
            ON CONFLICT(session_id) DO UPDATE SET deleted = 1",
                [id],
            )
            .map_err(|error| error.to_string())?;
        self.clear_turn_pending(id)?;
        let files = serde_json::to_string(attachments).map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO attachment_cleanup(session_id, files) VALUES(?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET files = excluded.files",
                params![id, files],
            )
            .map_err(|error| error.to_string())?;
        self.install_model_transcript(id, None)?;
        self.connection
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|error| format!("无法删除历史会话: {error}"))?;
        self.connection
            .execute(
                "DELETE FROM session_events WHERE session_id = ?1",
                params![id],
            )
            .map_err(|error| format!("无法删除历史事件: {error}"))?;
        self.connection
            .execute("DELETE FROM agent_runs WHERE session_id = ?1", params![id])
            .map_err(|error| format!("无法删除 Agent 运行记录: {error}"))?;
        self.connection
            .execute(
                "DELETE FROM conversation_checkpoints WHERE thread_id = ?1",
                params![id],
            )
            .map_err(|error| format!("无法删除对话检查点: {error}"))?;
        self.connection
            .execute(
                "DELETE FROM conversation_threads WHERE thread_id = ?1",
                params![id],
            )
            .map_err(|error| format!("无法删除对话线程记录: {error}"))?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub(super) fn pending_attachment_cleanup(
        &self,
        session_id: Option<&str>,
    ) -> Result<HashSet<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT files FROM attachment_cleanup cleanup JOIN session_bindings session
             ON session.session_id = cleanup.session_id AND session.deleted = 1
             WHERE ?1 IS NULL OR cleanup.session_id = ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([session_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut files = HashSet::new();
        for row in rows {
            files.extend(
                serde_json::from_str::<HashSet<String>>(&row.map_err(|error| error.to_string())?)
                    .map_err(|error| format!("附件清理记录无效: {error}"))?,
            );
        }
        Ok(files)
    }

    pub(super) fn finish_attachment_cleanup(&self, session_id: Option<&str>) -> Result<(), String> {
        self.connection
            .execute(
                "DELETE FROM attachment_cleanup WHERE (?1 IS NULL OR session_id = ?1)
             AND session_id IN (SELECT session_id FROM session_bindings WHERE deleted = 1)",
                [session_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(super) fn pending_session_cleanups(&self) -> Result<Vec<String>, String> {
        let mut statement = self.connection.prepare(
            "SELECT cleanup.session_id FROM attachment_cleanup cleanup JOIN session_bindings session
             ON session.session_id = cleanup.session_id AND session.deleted = 1"
        ).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.map(|row| row.map_err(|error| error.to_string()))
            .collect()
    }

    pub(super) fn attachment_references(
        &self,
        cache_root: &Path,
        deleting_session: Option<&str>,
        only_session: Option<&str>,
    ) -> Result<(HashSet<String>, HashSet<String>), String> {
        let prefix = cache_root.to_string_lossy().replace('\\', "/");
        let prefix = prefix.trim_end_matches('/');
        // SQLite walks JSON values in place: large base64 attachments and
        // transcripts are not deserialized or copied into the cleanup worker.
        let mut statement = self.connection.prepare(
            "WITH documents(owner, payload) AS (
                SELECT id, artifacts FROM sessions UNION ALL
                SELECT id, messages FROM sessions UNION ALL
                SELECT id, metadata FROM sessions UNION ALL
                SELECT id, events FROM sessions UNION ALL
                SELECT session_id, payload FROM session_events UNION ALL
                SELECT session_id, request FROM agent_runs UNION ALL
                SELECT session_id, request FROM agent_queue UNION ALL
                SELECT session_id, request FROM agent_steers UNION ALL
                SELECT thread_id, messages FROM conversation_checkpoints UNION ALL
                SELECT thread_id, artifacts FROM conversation_checkpoints UNION ALL
                SELECT thread_id, metadata FROM conversation_checkpoints UNION ALL
                SELECT thread_id, events FROM conversation_checkpoints UNION ALL
                SELECT session_id, message FROM turn_outbox
            )
            SELECT documents.owner, tree.value FROM documents, json_tree(documents.payload) tree
            WHERE (?2 IS NULL OR documents.owner = ?2) AND tree.type = 'text'
                AND substr(replace(tree.value, '\\', '/'), 1, length(?1) + 1) = (?1 || '/') COLLATE NOCASE"
        ).map_err(|error| format!("无法核查附件引用: {error}"))?;
        let rows = statement
            .query_map(params![prefix, only_session], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("无法读取附件引用: {error}"))?;
        let mut deleted = HashSet::new();
        let mut live = HashSet::new();
        for row in rows {
            let (owner, path) =
                row.map_err(|error| format!("附件引用数据无效，未执行清理: {error}"))?;
            if let Some(name) = crate::attachment_cache::attachment_name(cache_root, &path) {
                if deleting_session == Some(owner.as_str()) {
                    deleted.insert(name);
                } else {
                    live.insert(name);
                }
            }
        }
        Ok((deleted, live))
    }

    pub(super) fn reclaim_deleted_space(&self) -> Result<(), String> {
        let free_pages: i64 = self
            .connection
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if free_pages == 0 {
            return Ok(());
        }
        let mode: i64 = self
            .connection
            .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if mode == 2 {
            // Each step releases a page; consume all rows so reclamation is
            // completed instead of finalizing the pragma after its first row.
            let mut statement = self.connection.prepare("PRAGMA incremental_vacuum")
                .map_err(|error| error.to_string())?;
            let mut rows = statement.query([]).map_err(|error| error.to_string())?;
            while rows.next().map_err(|error| error.to_string())?.is_some() {}
            // WAL 模式下释放的页先进写前日志，主库文件要等 checkpoint 之后
            // 才真正缩小——「归还磁盘空间」的语义必须包含这一步。对非 WAL
            // 库 checkpoint 是无害的空操作。
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(|error| error.to_string())?;
            Ok(())
        } else {
            // Existing databases need one rebuild to enable incremental
            // reclamation; subsequent deletions do not rebuild the whole DB.
            // WAL 模式下 VACUUM 的重建结果同样先进写前日志，主库文件要
            // checkpoint 之后才缩小。
            self.connection
                .execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        }
        .map_err(|error| format!("历史空间回收失败，原数据仍保留: {error}"))
    }

    pub(super) fn session_binding(&self, id: &str) -> Result<Option<SessionBinding>, String> {
        let stored = self
            .connection
            .query_row(
                "SELECT storage_id,seed_from,deleted FROM session_bindings WHERE session_id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((storage_id, seed_from, deleted)) = stored {
            let seed_from = seed_from
                .map(|cut| serde_json::from_str(&cut).map_err(|error| error.to_string()))
                .transpose()?;
            return Ok(Some(SessionBinding {
                storage_id,
                seed_from,
                deleted,
            }));
        }
        let legacy: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM turn_outbox WHERE session_id = ?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        Ok(legacy.then(|| SessionBinding {
            storage_id: id.into(),
            seed_from: None,
            deleted: false,
        }))
    }

    pub(super) fn ensure_not_deleted(&self, id: &str) -> Result<(), String> {
        let deleted: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_bindings WHERE session_id = ?1 AND deleted = 1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if deleted {
            Err("该对话已删除，未恢复旧记录或重放消息。".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn install_session_binding(
        &self,
        id: &str,
        cut: Option<&CheckpointCut>,
    ) -> Result<(), String> {
        self.ensure_not_deleted(id)?;
        self.connection.execute("INSERT INTO session_bindings(session_id,storage_id,seed_from) VALUES(?1,?2,?3)
            ON CONFLICT(session_id) DO UPDATE SET storage_id = excluded.storage_id, seed_from = excluded.seed_from",
            params![id,next_history_id("session-storage"),cut.map(serde_json::to_string).transpose().map_err(|error|error.to_string())?]).map_err(|error|error.to_string())?;
        Ok(())
    }

    pub(super) fn clear_turn_pending(&self, id: &str) -> Result<(), String> {
        for query in [
            "DELETE FROM turn_outbox WHERE session_id = ?1",
            "DELETE FROM turn_recovery WHERE session_id = ?1",
            "DELETE FROM agent_queue WHERE session_id = ?1",
            "DELETE FROM agent_steers WHERE session_id = ?1",
        ] {
            self.connection
                .execute(query, [id])
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
