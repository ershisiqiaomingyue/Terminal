use super::*;

impl HistoryStore {
    /// Startup owns this repair before any view preloads or saves history.
    /// One transaction commits both run state and the persisted message flags.
    pub(super) fn recover_interrupted_runs(
        &self,
        active: &HashSet<(String, String)>,
    ) -> Result<Vec<AgentRunSummary>, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法开始会话恢复: {error}"))?;
        let result = (|| {
            // N12-A：先补偿「终态没落库」的回合，再做常规 interrupted 判定——
            // 顺序不能反，否则这些 run 会被当成 interrupted 盖掉真实终态。
            for terminal in self.unpersisted_terminal_turns(active)? {
                match self.repair_unpersisted_terminal(&terminal) {
                    Ok(true) => crate::diagnostics::record(
                        "warn",
                        "agent.turn_terminal_repair",
                        "native.history",
                        serde_json::json!({
                            "session_id": terminal.session_id,
                            "turn_id": terminal.turn_id,
                            "status": terminal.status,
                        }),
                    ),
                    Ok(false) => {} // 已被其他路径收口（例如正常 finish 抢先落地）
                    Err(error) => eprintln!("无法补偿 Agent 终态: {error}"),
                }
            }
            // Startup needs state only; avoid decoding every historical request/context.
            let runs = {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT session_id, turn_id, status, status_message, started_at,
                     updated_at, finished_at, resume_count FROM agent_runs",
                    )
                    .map_err(|error| error.to_string())?;
                let rows = statement
                    .query_map([], |row| {
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
                    })
                    .map_err(|error| error.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| error.to_string())?;
                rows
            };
            let mut recovered = Vec::new();
            for summary in &runs {
                if summary.finished_at.is_some()
                    || matches!(
                        summary.status.as_str(),
                        "completed" | "cancelled" | "failed"
                    )
                    || active.contains(&(summary.session_id.clone(), summary.turn_id.clone()))
                {
                    continue;
                }
                if self.mark_agent_run_interrupted(&summary.session_id, &summary.turn_id)? {
                    let mut summary = summary.clone();
                    summary.status = "interrupted".into();
                    summary.status_message =
                        Some("应用关闭前该任务未收到终态；可基于当前工作区继续处理。".into());
                    summary.updated_at = now_ms();
                    recovered.push(summary);
                }
            }
            // Also repair older/orphaned pending rows without a run record.
            // The SQL predicate avoids decoding histories with no stale flags.
            let pending_sessions = {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT id, messages FROM sessions WHERE EXISTS(
                        SELECT 1 FROM json_each(sessions.messages) message
                        WHERE json_extract(message.value, '$.role') = 'assistant'
                          AND json_extract(message.value, '$.pending') = 1)",
                    )
                    .map_err(|error| error.to_string())?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|error| error.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| error.to_string())?;
                rows
            };
            let run_index: HashMap<_, _> = runs
                .iter()
                .map(|run| ((run.session_id.as_str(), run.turn_id.as_str()), run))
                .collect();
            for (session_id, raw) in pending_sessions {
                let mut messages = decode(raw);
                let mut changed = false;
                for message in messages.as_array_mut().into_iter().flatten() {
                    if message.get("role").and_then(Value::as_str) != Some("assistant")
                        || message.get("pending").and_then(Value::as_bool) != Some(true)
                    {
                        continue;
                    }
                    let turn_id = message
                        .get("turnId")
                        .or_else(|| message.get("turn_id"))
                        .and_then(Value::as_str);
                    let live = active.iter().any(|(session, turn)| {
                        session == &session_id && turn_id.is_none_or(|id| id == turn)
                    });
                    if live {
                        continue;
                    }
                    let run = turn_id
                        .and_then(|turn| run_index.get(&(session_id.as_str(), turn)).copied());
                    let saved_time: Option<i64> = if let Some(turn_id) = turn_id {
                        self.connection.query_row(
                            "SELECT MAX(timestamp_ms) FROM session_events WHERE session_id = ?1
                             AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2",
                            params![session_id, turn_id], |row| row.get(0),
                        ).map_err(|error| error.to_string())?
                    } else {
                        None
                    };
                    let inline_time = message
                        .get("events")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|event| {
                            event
                                .get("timestamp_ms")
                                .or_else(|| event.get("timestampMs"))
                                .and_then(Value::as_i64)
                        })
                        .max();
                    let started_at = message
                        .get("startedAt")
                        .or_else(|| message.get("createdAt"))
                        .and_then(Value::as_i64)
                        .or_else(|| run.map(|run| run.started_at))
                        .unwrap_or_default();
                    let finished_at = run
                        .and_then(|run| run.finished_at)
                        .or(saved_time.into_iter().chain(inline_time).max())
                        .unwrap_or(started_at)
                        .max(started_at);
                    let Some(message) = message.as_object_mut() else {
                        continue;
                    };
                    message.insert("pending".into(), Value::Bool(false));
                    if message.get("finishedAt").is_none_or(Value::is_null) {
                        message.insert("finishedAt".into(), Value::from(finished_at));
                    }
                    changed = true;
                }
                if changed {
                    // Do not reorder navigation or rewrite any text/events.
                    self.connection
                        .execute(
                            "UPDATE sessions SET messages = ?1 WHERE id = ?2",
                            params![encode(&messages)?, session_id],
                        )
                        .map_err(|error| format!("无法恢复历史消息状态: {error}"))?;
                    // §8.9：恢复消息状态是列直写路径，同步分行表。
                    self.merge_session_messages(&session_id, &messages)?;
                }
            }
            Ok(recovered)
        })();
        match result {
            Ok(recovered) => {
                self.commit_transaction("无法提交会话恢复")?;
                Ok(recovered)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}
