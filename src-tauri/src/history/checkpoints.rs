use super::*;

impl HistoryStore {
    pub(super) fn ensure_conversation_thread(&self, thread_id: &str) -> Result<(), String> {
        self.ensure_not_deleted(thread_id)?;
        let timestamp = now_ms();
        self.connection
            .execute(
                "INSERT OR IGNORE INTO conversation_threads
                    (thread_id, root_thread_id, parent_thread_id, forked_from_checkpoint_id, created_at, updated_at)
                 VALUES (?1, ?1, NULL, NULL, ?2, ?2)",
                params![thread_id, timestamp],
            )
            .map_err(|error| format!("无法创建对话线程记录: {error}"))?;
        Ok(())
    }

    pub(super) fn conversation_thread(
        &self,
        thread_id: &str,
    ) -> Result<Option<ConversationThread>, String> {
        let session_exists = self
            .connection
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![thread_id],
                |_row| Ok(()),
            )
            .optional()
            .map_err(|error| format!("无法检查对话线程: {error}"))?
            .is_some();
        if !session_exists {
            return Ok(None);
        }
        self.ensure_conversation_thread(thread_id)?;
        self.connection
            .query_row(
                "SELECT thread_id, root_thread_id, parent_thread_id, forked_from_checkpoint_id, created_at, updated_at
                 FROM conversation_threads WHERE thread_id = ?1",
                params![thread_id],
                |row| {
                    Ok(ConversationThread {
                        thread_id: row.get(0)?,
                        root_thread_id: row.get(1)?,
                        parent_thread_id: row.get(2)?,
                        forked_from_checkpoint_id: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("无法读取对话线程记录: {error}"))
    }

    pub(super) fn save_conversation_thread(
        &self,
        thread: &ConversationThread,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO conversation_threads
                    (thread_id, root_thread_id, parent_thread_id, forked_from_checkpoint_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(thread_id) DO UPDATE SET
                    root_thread_id = excluded.root_thread_id,
                    parent_thread_id = excluded.parent_thread_id,
                    forked_from_checkpoint_id = excluded.forked_from_checkpoint_id,
                    updated_at = excluded.updated_at",
                params![
                    thread.thread_id,
                    thread.root_thread_id,
                    thread.parent_thread_id,
                    thread.forked_from_checkpoint_id,
                    thread.created_at,
                    thread.updated_at,
                ],
            )
            .map_err(|error| format!("无法保存对话线程记录: {error}"))?;
        Ok(())
    }

    pub(super) fn checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<Option<StoredConversationCheckpoint>, String> {
        self.connection
            .query_row(
                "SELECT id, thread_id, turn_id, label, messages, metadata, artifacts, report, usage, events, created_at, checkpoint_cut, model_transcript
                 FROM conversation_checkpoints WHERE id = ?1",
                params![checkpoint_id],
                |row| {
                    let events = decode(row.get::<_, String>(9)?);
                    Ok(StoredConversationCheckpoint {
                        summary: ConversationCheckpointSummary {
                            id: row.get(0)?,
                            thread_id: row.get(1)?,
                            turn_id: row.get(2)?,
                            label: row.get(3)?,
                            created_at: row.get(10)?,
                        },
                        messages: decode(row.get(4)?),
                        metadata: decode(row.get(5)?),
                        artifacts: decode(row.get(6)?),
                        report: decode_optional(row.get(7)?),
                        usage: decode_optional(row.get(8)?),
                        events: events.as_array().cloned().unwrap_or_default(),
                        model_transcript: row.get(12)?,
                        checkpoint_cut: row.get::<_,Option<String>>(11)?.map(|cut|serde_json::from_str(&cut))
                            .transpose().map_err(|error|rusqlite::Error::FromSqlConversionFailure(11,rusqlite::types::Type::Text,Box::new(error)))?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("无法读取对话检查点: {error}"))
    }

    pub(super) fn list_checkpoints(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ConversationCheckpointSummary>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, thread_id, turn_id, label, created_at
                 FROM conversation_checkpoints WHERE thread_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|error| format!("无法读取对话检查点: {error}"))?;
        let rows = statement
            .query_map(params![thread_id], |row| {
                Ok(ConversationCheckpointSummary {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    label: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|error| format!("无法读取对话检查点: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("对话检查点格式无效: {error}")))
            .collect()
    }

    pub(super) fn create_checkpoint(
        &self,
        thread_id: &str,
        turn_id: Option<String>,
        label: Option<String>,
        checkpoint_cut: Option<CheckpointCut>,
    ) -> Result<ConversationCheckpointSummary, String> {
        let Some(record) = self.get_record(thread_id)? else {
            return Err("对话线程不存在，无法创建检查点".to_string());
        };
        let summary = ConversationCheckpointSummary {
            id: next_history_id("checkpoint"),
            thread_id: thread_id.to_string(),
            turn_id: turn_id.filter(|value| !value.trim().is_empty()),
            label: label.filter(|value| !value.trim().is_empty()),
            created_at: now_ms(),
        };
        let events = record.events.unwrap_or(Value::Array(Vec::new()));
        self.connection
            .execute(
                "INSERT INTO conversation_checkpoints
                    (id, thread_id, turn_id, label, messages, metadata, artifacts, report, usage, events, created_at, checkpoint_cut, model_transcript)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    summary.id,
                    summary.thread_id,
                    summary.turn_id,
                    summary.label,
                    encode(&record.messages)?,
                    encode(&record.metadata)?,
                    encode(&record.artifacts)?,
                    encode_optional(&record.report)?,
                    encode_optional(&record.usage)?,
                    encode(&events)?,
                    summary.created_at,
                    checkpoint_cut.as_ref().map(serde_json::to_string).transpose().map_err(|error| error.to_string())?,
                    self.model_transcript_bytes(thread_id)?,
                ],
            )
            .map_err(|error| format!("无法保存对话检查点: {error}"))?;
        self.ensure_conversation_thread(thread_id)?;
        Ok(summary)
    }

    pub(super) fn restore_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: &str,
    ) -> Result<HistorySessionRecord, String> {
        let checkpoint = self
            .checkpoint(checkpoint_id)?
            .ok_or_else(|| "没有找到指定的对话检查点".to_string())?;
        if checkpoint.summary.thread_id != thread_id {
            return Err("该检查点不属于当前对话线程".to_string());
        }
        if self.session_binding(thread_id)?.is_some() && checkpoint.checkpoint_cut.is_none() {
            return Err("该旧检查点没有历史边界，未用不完整历史覆盖模型上下文。".into());
        }
        self.ensure_conversation_thread(thread_id)?;
        let timestamp = now_ms();
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法开始回退对话: {error}"))?;
        let result = (|| {
            self.install_session_binding(thread_id, checkpoint.checkpoint_cut.as_ref())?;
            self.install_model_transcript(thread_id, checkpoint.model_transcript.as_deref())?;
            self.clear_turn_pending(thread_id)?;
            let affected = self
                .connection
                .execute(
                    "UPDATE sessions SET
                        updated_at = ?1,
                        artifacts = ?2,
                        messages = ?3,
                        metadata = ?4,
                        report = ?5,
                        usage = ?6,
                        schema_version = ?7,
                        events = ?8,
                        message_count = ?9
                     WHERE id = ?10",
                    params![
                        timestamp,
                        encode(&checkpoint.artifacts)?,
                        encode(&checkpoint.messages)?,
                        encode(&checkpoint.metadata)?,
                        encode_optional(&checkpoint.report)?,
                        encode_optional(&checkpoint.usage)?,
                        HISTORY_SCHEMA_VERSION,
                        encode(&Value::Array(checkpoint.events.clone()))?,
                        json_array_len(&checkpoint.messages),
                        thread_id,
                    ],
                )
                .map_err(|error| format!("无法回退对话历史: {error}"))?;
            if affected == 0 {
                return Err("对话线程不存在，无法回退".into());
            }
            // §8.9：列直写路径必须同步分行表（读切换后 get_record 以分行为准）。
            self.merge_session_messages(thread_id, &checkpoint.messages)?;
            self.connection
                .execute(
                    "DELETE FROM session_events WHERE session_id = ?1",
                    params![thread_id],
                )
                .map_err(|error| format!("无法回退对话事件: {error}"))?;
            for (index, event) in checkpoint.events.iter().enumerate() {
                self.connection
                    .execute(
                        "INSERT OR IGNORE INTO session_events
                            (session_id, event_key, sequence, timestamp_ms, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            thread_id,
                            history_event_key(event, index),
                            event.get("sequence").and_then(Value::as_i64),
                            event.get("timestamp_ms").and_then(Value::as_i64),
                            encode(event)?,
                        ],
                    )
                    .map_err(|error| format!("无法恢复对话事件: {error}"))?;
            }
            self.connection
                .execute(
                    "DELETE FROM agent_runs WHERE session_id = ?1 AND started_at > ?2",
                    params![thread_id, checkpoint.summary.created_at],
                )
                .map_err(|error| format!("无法清理回退后的运行记录: {error}"))?;
            self.connection
                .execute(
                    "UPDATE conversation_threads SET updated_at = ?1 WHERE thread_id = ?2",
                    params![timestamp, thread_id],
                )
                .map_err(|error| format!("无法更新对话线程: {error}"))?;
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => self
                .connection
                .execute_batch("COMMIT")
                .map_err(|error| format!("无法提交对话回退: {error}"))?,
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        self.get_record(thread_id)?
            .ok_or_else(|| "回退后的对话线程无法读取".to_string())
    }

    pub(super) fn rewind_before_message(
        &self,
        thread_id: &str,
        message_id: &str,
        turn_id: Option<&str>,
    ) -> Result<HistorySessionRecord, String> {
        self.ensure_not_deleted(thread_id)?;
        if self.has_collaboration_jobs(thread_id)? {
            return Err("该对话有关联协作任务，暂不能编辑历史消息。".into());
        }
        // Editing only needs the durable message columns and run summaries.
        // Hydrating up to 1,000 tool events here made the edit request wait
        // behind a full timeline rebuild before the new turn could start.
        let record = self.get_record_raw(thread_id)?.ok_or("对话不存在")?;
        let messages = record.messages.as_array().ok_or("对话消息格式无效")?;
        let is_user =
            |message: &&Value| message.get("role").and_then(Value::as_str) == Some("user");
        let index = messages
            .iter()
            .enumerate()
            .find(|(_, message)| {
                is_user(message) && message.get("id").and_then(Value::as_str) == Some(message_id)
            })
            .map(|(index, _)| index)
            .or_else(|| {
                turn_id.and_then(|turn| {
                    messages
                        .iter()
                        .enumerate()
                        .find(|(_, message)| {
                            is_user(message)
                                && message
                                    .get("turnId")
                                    .or_else(|| message.get("turn_id"))
                                    .and_then(Value::as_str)
                                    == Some(turn)
                        })
                        .map(|(index, _)| index)
                })
            })
            .ok_or("未找到要编辑的消息，请重新打开对话")?;
        let edited_turn = messages[index]
            .get("turnId")
            .or_else(|| messages[index].get("turn_id"))
            .and_then(Value::as_str);
        let mut retained = messages[..index].to_vec();
        // A steer can add another user message to the same turn. Keep the visible
        // prefix, but discard that turn's shared execution events and summaries.
        for message in &mut retained {
            if edited_turn.is_some()
                && message
                    .get("turnId")
                    .or_else(|| message.get("turn_id"))
                    .and_then(Value::as_str)
                    == edited_turn
            {
                if let Some(object) = message.as_object_mut() {
                    for key in ["events", "contextCheckpoint", "finalReport"] {
                        object.remove(key);
                    }
                }
            }
        }
        let retained_turns: Vec<&str> = retained
            .iter()
            .filter_map(|message| {
                message
                    .get("turnId")
                    .or_else(|| message.get("turn_id"))
                    .and_then(Value::as_str)
            })
            .filter(|turn| Some(*turn) != edited_turn)
            .collect();
        let cutoff = messages[index]
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let mut metadata = record.metadata.clone();
        if let Some(object) = metadata.as_object_mut() {
            for key in ["contextCheckpoint", "context_checkpoint", "usageTurnId"] {
                object.remove(key);
            }
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        // Rotate the history generation so delayed WebView saves cannot resurrect the removed turns.
        self.install_session_binding(thread_id, None)?;
        self.install_model_transcript(thread_id, None)?;
        self.clear_turn_pending(thread_id)?;
        self.connection.execute(
            "DELETE FROM session_events WHERE session_id = ?1 AND (
                (COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId'), '') != ''
                 AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) NOT IN (SELECT value FROM json_each(?2)))
                OR (COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId'), '') = ''
                    AND (COALESCE(timestamp_ms, json_extract(payload, '$.timestamp_ms'), 0) = 0
                         OR COALESCE(timestamp_ms, json_extract(payload, '$.timestamp_ms'), 0) >= ?3)))",
            params![thread_id, serde_json::to_string(&retained_turns).map_err(|error| error.to_string())?, cutoff],
        ).map_err(|error| error.to_string())?;
        self.connection.execute("DELETE FROM agent_runs WHERE session_id = ?1 AND turn_id NOT IN (SELECT value FROM json_each(?2))",
            params![thread_id, serde_json::to_string(&retained_turns).map_err(|error| error.to_string())?]).map_err(|error| error.to_string())?;
        self.connection.execute("UPDATE sessions SET messages = ?1, message_count = ?2, metadata = ?3,
            events = '[]', artifacts = '[]', report = NULL, usage = NULL, updated_at = ?4 WHERE id = ?5",
            params![encode(&Value::Array(retained.clone()))?, retained.len() as i64, encode(&metadata)?, now_ms(), thread_id])
            .map_err(|error| error.to_string())?;
        // §8.9：rewind 直写列后同步分行表（删掉被回退的消息行）。
        self.merge_session_messages(thread_id, &Value::Array(retained))?;
        transaction.commit().map_err(|error| error.to_string())?;
        self.get_record_raw(thread_id)?
            .ok_or("编辑后的对话无法读取".into())
    }

    pub(super) fn fork_thread(
        &self,
        source_thread_id: &str,
        checkpoint_id: Option<String>,
        title: Option<String>,
        current_cut: Option<CheckpointCut>,
    ) -> Result<HistorySessionRecord, String> {
        let checkpoint = checkpoint_id
            .as_deref()
            .map(|id| self.checkpoint(id))
            .transpose()?
            .flatten();
        if checkpoint_id.is_some() && checkpoint.is_none() {
            return Err("没有找到指定的分叉检查点，未改用最新历史。".into());
        }
        if let Some(checkpoint) = &checkpoint {
            if checkpoint.summary.thread_id != source_thread_id {
                return Err("该检查点不属于要分叉的对话线程".to_string());
            }
        }
        let source = self
            .get_record(source_thread_id)?
            .ok_or_else(|| "源对话线程不存在".to_string())?;
        let parent_thread = self
            .conversation_thread(source_thread_id)?
            .ok_or_else(|| "源对话线程记录不存在".to_string())?;
        let snapshot = checkpoint.as_ref();
        let checkpoint_cut = snapshot
            .map(|checkpoint| checkpoint.checkpoint_cut.clone())
            .unwrap_or(current_cut);
        if self.session_binding(source_thread_id)?.is_some() && checkpoint_cut.is_none() {
            return Err("缺少可验证的分叉边界，未丢弃原始模型历史。".into());
        }
        let child_id = next_history_id("thread");
        let mut metadata = snapshot
            .map(|value| value.metadata.clone())
            .unwrap_or_else(|| source.metadata.clone());
        if !metadata.is_object() {
            metadata = Value::Object(Default::default());
        }
        if let Some(object) = metadata.as_object_mut() {
            object.insert(
                "conversationLineage".to_string(),
                serde_json::json!({
                    "parentThreadId": source_thread_id,
                    "forkedFromCheckpointId": checkpoint.as_ref().map(|value| value.summary.id.clone()),
                }),
            );
        }
        let fork_events = snapshot
            .map(|value| value.events.clone())
            .unwrap_or_else(|| {
                source
                    .events
                    .as_ref()
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            });
        let timestamp = now_ms();
        let child = HistorySessionRecord {
            id: child_id.clone(),
            title: title
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("{}（分叉）", source.title)),
            updated_at: timestamp,
            archived: false,
            schema_version: HISTORY_SCHEMA_VERSION,
            jira_context: source.jira_context.clone(),
            artifacts: snapshot
                .map(|value| value.artifacts.clone())
                .unwrap_or_else(|| source.artifacts.clone()),
            messages: snapshot
                .map(|value| value.messages.clone())
                .unwrap_or_else(|| source.messages.clone()),
            metadata,
            report: snapshot
                .map(|value| value.report.clone())
                .unwrap_or_else(|| source.report.clone()),
            usage: snapshot
                .map(|value| value.usage.clone())
                .unwrap_or_else(|| source.usage.clone()),
            events: Some(Value::Array(fork_events)),
            agent_runs: Vec::new(),
            usage_totals: None,
        };
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        self.write_record(&child)?;
        let model_transcript = match snapshot {
            Some(checkpoint) => checkpoint.model_transcript.clone(),
            None => self.model_transcript_bytes(source_thread_id)?,
        };
        self.install_model_transcript(&child.id, model_transcript.as_deref())?;
        self.install_session_binding(&child.id, checkpoint_cut.as_ref())?;
        self.save_conversation_thread(&ConversationThread {
            thread_id: child_id,
            root_thread_id: parent_thread.root_thread_id,
            parent_thread_id: Some(source_thread_id.to_string()),
            forked_from_checkpoint_id: checkpoint.map(|value| value.summary.id),
            created_at: timestamp,
            updated_at: timestamp,
        })?;
        transaction.commit().map_err(|error| error.to_string())?;
        self.get_record(&child.id)?
            .ok_or_else(|| "分叉后的对话线程无法读取".to_string())
    }
}
