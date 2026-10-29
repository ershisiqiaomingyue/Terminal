use super::*;

impl HistoryStore {
    pub(super) fn list_agent_run_summaries(
        &self,
        session_id: &str,
    ) -> Result<Vec<AgentRunSummary>, String> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, turn_id, status, status_message, started_at, updated_at, finished_at, resume_count
             FROM agent_runs WHERE session_id = ?1 ORDER BY started_at DESC",
        ).map_err(|error| format!("无法读取 Agent 运行摘要: {error}"))?;
        let rows = statement.query_map(params![session_id], |row| {
            Ok(AgentRunSummary {
                session_id: row.get(0)?,
                turn_id: row.get(1)?,
                status: row.get(2)?,
                status_message: row.get(3)?,
                started_at: row.get(4)?,
                updated_at: row.get(5)?,
                finished_at: row.get(6)?,
                resume_count: row.get(7)?,
            })
        }).map_err(|error| format!("Agent 运行摘要格式无效: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("Agent 运行摘要格式无效: {error}")))
            .collect()
    }

    pub(super) fn start_agent_run_locked(
        &self,
        session_id: &str,
        turn_id: &str,
        request: &AgentChatRequest,
    ) -> Result<(), String> {
        let request = serde_json::to_value(request)
            .map_err(|error| format!("无法保存 Agent 运行请求: {error}"))?;
        self.ensure_conversation_thread(session_id)?;
        self.upsert_agent_turn_messages(
            session_id,
            turn_id,
            request
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            None,
        )?;
        let existing = self
            .connection
            .query_row(
                "SELECT status, finished_at FROM agent_runs WHERE session_id = ?1 AND turn_id = ?2",
                params![session_id, turn_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .map_err(|error| format!("无法检查 Agent 运行记录: {error}"))?;
        let timestamp = now_ms();
        match existing {
            None => {
                self.connection
                    .execute(
                        "INSERT INTO agent_runs
                            (session_id, turn_id, status, status_message, request, started_at, updated_at, finished_at, resume_count)
                         VALUES (?1, ?2, 'preparing', NULL, ?3, ?4, ?4, NULL, 0)",
                        params![session_id, turn_id, encode(&request)?, timestamp],
                    )
                    .map_err(|error| format!("无法创建 Agent 运行记录: {error}"))?;
            }
            Some((status, None)) if status == "interrupted" => {
                self.connection
                    .execute(
                        "UPDATE agent_runs
                         SET status = 'preparing', status_message = NULL, request = ?1,
                             updated_at = ?2, finished_at = NULL, resume_count = resume_count + 1
                         WHERE session_id = ?3 AND turn_id = ?4
                           AND status = 'interrupted' AND finished_at IS NULL",
                        params![encode(&request)?, timestamp, session_id, turn_id],
                    )
                    .map_err(|error| format!("无法继续 Agent 运行记录: {error}"))?;
            }
            Some((status, finished_at)) => {
                return Err(if finished_at.is_some() {
                    format!("Agent 轮次已经结束，不能重新执行（{status}）")
                } else {
                    format!("Agent 轮次已经在执行（{status}）")
                });
            }
        }
        Ok(())
    }

    pub fn start_agent_run(
        &self,
        session_id: &str,
        turn_id: &str,
        request: &AgentChatRequest,
    ) -> Result<(), String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 运行记录: {error}"))?;
        let result = self.start_agent_run_locked(session_id, turn_id, request);
        match result {
            Ok(()) => self.commit_transaction("无法提交 Agent 运行记录"),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn start_agent_run_from_queue(
        &self,
        session_id: &str,
        turn_id: &str,
        request: &AgentChatRequest,
        queue_id: &str,
        lease_id: &str,
        lease_owner: &str,
    ) -> Result<AgentQueueItem, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定排队 Agent 运行记录: {error}"))?;
        let result = (|| {
            let valid = self
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM agent_queue
                     WHERE session_id = ?1 AND id = ?2 AND status = 'claimed'
                       AND lease_id = ?3 AND lease_owner = ?4)",
                    params![session_id, queue_id, lease_id, lease_owner],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| format!("无法验证 Agent 队列 lease: {error}"))?;
            if !valid {
                return Err("Agent 队列 lease 已失效或不属于当前调度器".to_string());
            }
            self.start_agent_run_locked(session_id, turn_id, request)?;
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE agent_queue
                     SET status = 'dispatched', lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?1, completed_at = ?1
                     WHERE session_id = ?2 AND id = ?3 AND status = 'claimed'
                       AND lease_id = ?4 AND lease_owner = ?5",
                    params![timestamp, session_id, queue_id, lease_id, lease_owner],
                )
                .map_err(|error| format!("无法确认排队 Agent 运行记录: {error}"))?;
            if changed != 1 {
                return Err("Agent 队列 lease 在启动事务中失效".to_string());
            }
            self.agent_queue_item(session_id, queue_id)?
                .ok_or_else(|| "Agent 队列消息不存在".to_string())
        })();
        match result {
            Ok(item) => {
                self.commit_transaction("无法提交排队 Agent 运行记录")?;
                Ok(item)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn update_agent_run_status(
        &self,
        session_id: &str,
        turn_id: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE agent_runs
                 SET status = ?1, status_message = ?2, updated_at = ?3
                 WHERE session_id = ?4 AND turn_id = ?5 AND finished_at IS NULL",
                params![status, message, now_ms(), session_id, turn_id],
            )
            .map_err(|error| format!("无法更新 Agent 运行状态: {error}"))?;
        Ok(())
    }

    pub(super) fn finish_agent_run_locked(
        &self,
        session_id: &str,
        turn_id: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<(), String> {
        let timestamp = now_ms();
        let changed = self
            .connection
            .execute(
                "UPDATE agent_runs
                 SET status = ?1, status_message = ?2, updated_at = ?3, finished_at = ?3
                 WHERE session_id = ?4 AND turn_id = ?5 AND finished_at IS NULL",
                params![status, message, timestamp, session_id, turn_id],
            )
            .map_err(|error| format!("无法结束 Agent 运行记录: {error}"))?;
        if changed == 0 {
            let existing = self
                .connection
                .query_row(
                    "SELECT status, finished_at FROM agent_runs WHERE session_id = ?1 AND turn_id = ?2",
                    params![session_id, turn_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .optional()
                .map_err(|error| format!("无法检查 Agent 运行终态: {error}"))?
                .ok_or_else(|| "Agent 运行记录不存在".to_string())?;
            if existing.0 != status || existing.1.is_none() {
                return Err("Agent 运行记录已处于另一终态".to_string());
            }
        }
        Ok(())
    }

    pub fn finish_agent_run(
        &self,
        session_id: &str,
        turn_id: &str,
        status: &str,
        message: Option<&str>,
    ) -> Result<(), String> {
        self.finish_agent_run_locked(session_id, turn_id, status, message)
    }

    pub fn finish_agent_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        status: &str,
        message: Option<&str>,
        assistant_content: Option<&str>,
    ) -> Result<(), String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定 Agent 终态: {error}"))?;
        let result = (|| {
            self.upsert_agent_turn_messages(session_id, turn_id, "", assistant_content)?;
            self.finish_agent_run_locked(session_id, turn_id, status, message)
        })();
        match result {
            Ok(()) => self.commit_transaction("无法提交 Agent 终态"),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn upsert_agent_turn_messages(
        &self,
        session_id: &str,
        turn_id: &str,
        user_content: &str,
        assistant_content: Option<&str>,
    ) -> Result<(), String> {
        let Some(stored_messages) = self
            .connection
            .query_row(
                "SELECT messages FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("无法读取 Agent 对话消息: {error}"))?
        else {
            return Ok(());
        };
        let mut messages = decode(stored_messages)
            .as_array()
            .cloned()
            .unwrap_or_default();
        let timestamp = now_ms();
        let user_exists = messages.iter().any(|message| {
            message.get("role").and_then(Value::as_str) == Some("user")
                && message
                    .get("turnId")
                    .or_else(|| message.get("turn_id"))
                    .and_then(Value::as_str)
                    == Some(turn_id)
        });
        if !user_exists && !user_content.trim().is_empty()
            && !turn_id.starts_with(agent_core::background::BACKGROUND_RESULT_PREFIX) {
            messages.push(serde_json::json!({ "id": format!("user:{turn_id}"), "role": "user", "content": user_content, "createdAt": timestamp, "turnId": turn_id }));
        }
        let assistant_index = messages.iter().position(|message| {
            message.get("role").and_then(Value::as_str) == Some("assistant")
                && message
                    .get("turnId")
                    .or_else(|| message.get("turn_id"))
                    .and_then(Value::as_str)
                    == Some(turn_id)
        });
        if let Some(index) = assistant_index {
            if let Some(content) = assistant_content {
                if let Some(message) = messages[index].as_object_mut() {
                    message.insert("content".to_string(), Value::String(content.to_string()));
                    message.insert("pending".to_string(), Value::Bool(false));
                    message.insert("finishedAt".to_string(), Value::from(timestamp));
                }
            }
        } else {
            messages.push(serde_json::json!({ "id": format!("assistant:{turn_id}"), "role": "assistant", "content": assistant_content.unwrap_or_default(), "createdAt": timestamp, "startedAt": timestamp, "pending": assistant_content.is_none(), "turnId": turn_id }));
        }
        self.connection
            .execute(
                "UPDATE sessions SET messages = ?1, updated_at = ?2 WHERE id = ?3",
                params![encode(&Value::Array(messages.clone()))?, timestamp, session_id],
            )
            .map_err(|error| format!("无法保存 Agent 对话消息: {error}"))?;
        // §8.9：终态补写 assistant 消息是列直写路径，同步分行表。
        self.merge_session_messages(session_id, &Value::Array(messages))?;
        Ok(())
    }

    pub(super) fn list_agent_runs(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<StoredAgentRun>, String> {
        let query = if session_id.is_some() {
            "SELECT session_id, turn_id, status, status_message, request, started_at, updated_at, finished_at, resume_count
             FROM agent_runs WHERE session_id = ?1 ORDER BY started_at DESC"
        } else {
            "SELECT session_id, turn_id, status, status_message, request, started_at, updated_at, finished_at, resume_count
             FROM agent_runs ORDER BY started_at DESC"
        };
        let mut statement = self
            .connection
            .prepare(query)
            .map_err(|error| format!("无法读取 Agent 运行记录: {error}"))?;
        let decode_row = |row: &rusqlite::Row<'_>| {
            Ok(StoredAgentRun {
                summary: AgentRunSummary {
                    session_id: row.get(0)?,
                    turn_id: row.get(1)?,
                    status: row.get(2)?,
                    status_message: row.get(3)?,
                    started_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    finished_at: row.get(7)?,
                    resume_count: row.get(8)?,
                },
                request: decode(row.get::<_, String>(4)?),
            })
        };
        let rows = if let Some(session_id) = session_id {
            statement.query_map(params![session_id], decode_row)
        } else {
            statement.query_map([], decode_row)
        }
        .map_err(|error| format!("Agent 运行记录格式无效: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("Agent 运行记录格式无效: {error}")))
            .collect()
    }

    pub(super) fn agent_run(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<StoredAgentRun>, String> {
        let runs = self.list_agent_runs(Some(session_id))?;
        let result = runs.into_iter().find(|run| run.summary.turn_id == turn_id);
        Ok(result)
    }

    pub(super) fn mark_agent_run_interrupted(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, String> {
        let affected = self
            .connection
            .execute(
                "UPDATE agent_runs
                 SET status = 'interrupted',
                     status_message = '应用关闭前该任务未收到终态；可基于当前工作区继续处理。',
                     updated_at = ?1
                 WHERE session_id = ?2 AND turn_id = ?3 AND finished_at IS NULL
                   AND status NOT IN ('interrupted', 'completed', 'cancelled', 'failed')",
                params![now_ms(), session_id, turn_id],
            )
            .map_err(|error| format!("无法标记中断的 Agent 运行: {error}"))?;
        Ok(affected > 0)
    }
}
