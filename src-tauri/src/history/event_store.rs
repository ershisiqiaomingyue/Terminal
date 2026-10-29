use super::*;

impl HistoryStore {
    pub(super) fn terminal_type(event: &Value) -> Option<&str> {
        match event.get("type").and_then(Value::as_str) {
            Some("completed" | "cancelled" | "error") => event.get("type").and_then(Value::as_str),
            _ => None,
        }
    }

    pub(super) fn terminal_turn_id(event: &Value) -> Option<String> {
        Self::terminal_type(event).and_then(|_| {
            event
                .get("turn_id")
                .or_else(|| event.get("turnId"))
                .and_then(Value::as_str)
                .filter(|turn_id| !turn_id.trim().is_empty())
                .map(str::to_string)
        })
    }

    pub(super) fn is_durable_terminal(event: &Value) -> bool {
        Self::terminal_type(event).is_some()
            && event
                .get("durable_terminal")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }

    pub(super) fn persisted_durable_terminal_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, Value)>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT payload, COALESCE(sequence, 0), COALESCE(timestamp_ms, 0), rowid
                 FROM session_events WHERE session_id = ?1
                   AND json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
                   AND json_extract(payload, '$.durable_terminal') = 1",
            )
            .map_err(|error| format!("无法读取持久化终态事件: {error}"))?;
        let rows = statement
            .query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| format!("无法读取持久化终态事件: {error}"))?;
        let mut terminals = Vec::<(String, (i64, i64, i64), Value)>::new();
        for row in rows {
            let (payload, sequence, timestamp_ms, rowid) =
                row.map_err(|error| format!("持久化终态事件格式无效: {error}"))?;
            let event = decode(payload);
            let Some(turn_id) = Self::terminal_turn_id(&event) else {
                continue;
            };
            if !Self::is_durable_terminal(&event) {
                continue;
            }
            // Stream sequence resets on process restart; it cannot order
            // durable terminals from different executions or imported logs.
            let rank = (timestamp_ms, rowid, sequence);
            if let Some((_, existing_rank, existing_event)) = terminals
                .iter_mut()
                .find(|(existing_turn_id, _, _)| existing_turn_id == &turn_id)
            {
                if rank > *existing_rank {
                    *existing_rank = rank;
                    *existing_event = event;
                }
            } else {
                terminals.push((turn_id, rank, event));
            }
        }
        terminals.sort_by_key(|(_, rank, _)| *rank);
        Ok(terminals
            .into_iter()
            .map(|(turn_id, _, event)| (turn_id, event))
            .collect())
    }

    pub(super) fn merge_events(&self, session_id: &str, events: &[Value]) -> Result<(), String> {
        let durable_terminals = self.persisted_durable_terminal_events(session_id)?;
        let durable_turn_ids = durable_terminals
            .iter()
            .map(|(turn_id, _)| turn_id.clone())
            .collect::<HashSet<_>>();
        // A native terminal event is written before the WebView receives it.
        // Discard stale frontend terminal snapshots for the same turn so they
        // cannot override a completed response during restart hydration.
        let mut replacement = events
            .iter()
            .filter(|event| {
                Self::terminal_turn_id(event)
                    .is_none_or(|turn_id| !durable_turn_ids.contains(&turn_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut replacement_keys = replacement
            .iter()
            .enumerate()
            .map(|(index, event)| history_event_key(event, index))
            .collect::<HashSet<_>>();
        for (_, event) in durable_terminals {
            let key = history_event_key(&event, replacement.len());
            if replacement_keys.insert(key) {
                replacement.push(event);
            }
        }
        // Only obsolete volatile terminal claims are superseded by the native
        // durable terminal. All other unloaded rows stay in the append-only log.
        for turn_id in &durable_turn_ids {
            self.connection.execute(
                "DELETE FROM session_events WHERE session_id = ?1
                 AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
                 AND json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
                 AND COALESCE(json_extract(payload, '$.durable_terminal'), 0) = 0",
                params![session_id, turn_id],
            ).map_err(|error| format!("无法更新历史终态事件: {error}"))?;
        }
        for (index, event) in replacement.iter().enumerate() {
            let event_key = history_event_key(event, index);
            let sequence = event.get("sequence").and_then(Value::as_i64);
            let timestamp_ms = event.get("timestamp_ms").and_then(Value::as_i64);
            self.connection
                .execute(
                    "INSERT OR IGNORE INTO session_events
                        (session_id, event_key, sequence, timestamp_ms, payload)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        session_id,
                        event_key,
                        sequence,
                        timestamp_ms,
                        encode(event)?
                    ],
                )
                .map_err(|error| format!("无法保存历史事件: {error}"))?;
        }
        Ok(())
    }

    pub fn append_events(
        &self,
        session_id: &str,
        updated_at: i64,
        events: &[Value],
    ) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法开始追加历史事件: {error}"))?;
        let result = (|| {
            self.ensure_not_deleted(session_id)?;
            let exists = self
                .connection
                .query_row(
                    "SELECT 1 FROM sessions WHERE id = ?1",
                    params![session_id],
                    |_row| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法检查历史会话: {error}"))?;
            if exists.is_none() {
                return Err("历史会话不存在，无法追加事件".to_string());
            }
            // Non-terminal tool/context events cannot supersede a terminal.
            // Avoid scanning and decoding the full history on every tool call.
            let persisted_durable_turns = if events
                .iter()
                .any(|event| Self::terminal_turn_id(event).is_some())
            {
                self.persisted_durable_terminal_events(session_id)?
                    .into_iter()
                    .map(|(turn_id, _)| turn_id)
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            let incoming_durable_turns = events
                .iter()
                .filter(|event| Self::is_durable_terminal(event))
                .filter_map(Self::terminal_turn_id)
                .collect::<HashSet<_>>();
            let mut inserted_durable_turns = HashSet::new();
            for (index, event) in events.iter().enumerate() {
                if let Some(turn_id) = Self::terminal_turn_id(event) {
                    let durable = Self::is_durable_terminal(event);
                    if (!durable
                        && (persisted_durable_turns.contains(&turn_id)
                            || incoming_durable_turns.contains(&turn_id)))
                        || (durable
                            && (persisted_durable_turns.contains(&turn_id)
                                || !inserted_durable_turns.insert(turn_id)))
                    {
                        continue;
                    }
                }
                let event_key = history_event_key(event, index);
                let sequence = event.get("sequence").and_then(Value::as_i64);
                let timestamp_ms = event.get("timestamp_ms").and_then(Value::as_i64);
                self.connection
                    .execute(
                        "INSERT INTO session_events
                            (session_id, event_key, sequence, timestamp_ms, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(session_id, event_key) DO UPDATE SET payload = excluded.payload
                         WHERE json_extract(excluded.payload, '$.type') = 'process_item'
                           AND COALESCE(json_extract(excluded.payload, '$.snapshot_revision'), 0)
                             > COALESCE(json_extract(session_events.payload, '$.snapshot_revision'), 0)",
                        params![
                            session_id,
                            event_key,
                            sequence,
                            timestamp_ms,
                            encode(event)?
                        ],
                    )
                    .map_err(|error| format!("无法追加历史事件: {error}"))?;
            }
            self.connection
                .execute(
                    "UPDATE sessions SET updated_at = MAX(updated_at, ?1) WHERE id = ?2",
                    params![updated_at, session_id],
                )
                .map_err(|error| format!("无法更新历史会话时间: {error}"))?;
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => self
                .connection
                .execute_batch("COMMIT")
                .map_err(|error| format!("无法提交历史事件: {error}")),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn turn_event_page(
        &self,
        session_id: &str,
        turn_id: &str,
        after_row_id: i64,
        through_row_id: Option<i64>,
    ) -> Result<HistoryTurnEventPage, String> {
        let through = match through_row_id {
            Some(value) => value,
            None => self
                .connection
                .query_row(
                    "SELECT COALESCE(MAX(rowid), 0) FROM session_events WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?,
        };
        let mut statement = self.connection.prepare(
            "SELECT rowid, payload FROM session_events
             WHERE session_id = ?1 AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
               AND rowid > ?3 AND rowid <= ?4 ORDER BY rowid LIMIT ?5"
        ).map_err(|error| format!("无法读取执行过程: {error}"))?;
        let rows = statement
            .query_map(
                params![
                    session_id,
                    turn_id,
                    after_row_id.max(0),
                    through,
                    HISTORY_PROCESS_PAGE_SIZE + 1
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut last_row_id = after_row_id.max(0);
        let mut next_cursor = None;
        for row in rows {
            let (row_id, payload) = row.map_err(|error| error.to_string())?;
            // Always admit one oversized item intact, so the cursor can advance.
            if !events.is_empty()
                && (events.len() == HISTORY_PROCESS_PAGE_SIZE
                    || bytes + payload.len() > HISTORY_PROCESS_PAGE_BYTES)
            {
                next_cursor = Some(last_row_id);
                break;
            }
            bytes += payload.len();
            last_row_id = row_id;
            events.push(decode(payload));
        }
        Ok(HistoryTurnEventPage {
            events,
            next_cursor,
            through_row_id: through,
        })
    }

    pub(super) fn hydrate_events(
        &self,
        records: &mut [HistorySessionRecord],
    ) -> Result<(), String> {
        for record in records {
            if let Some(binding) = self.session_binding(&record.id)? {
                if binding.storage_id != record.id && record.metadata.is_object() {
                    record.metadata["_storageBindingId"] = Value::String(binding.storage_id);
                }
            }
            record.usage_totals = Some(self.session_usage_totals(&record.id)?);
            self.hydrate_usage(record)?;
            self.hydrate_context_usage(record)?;
            let mut statement = self
                .connection
                .prepare(
                    "SELECT payload FROM (
                        SELECT payload, sequence, timestamp_ms, rowid
                        FROM session_events
                        WHERE session_id = ?1
                          AND COALESCE(json_extract(payload, '$.type'), '') != 'process_item'
                        ORDER BY COALESCE(timestamp_ms, 0) DESC, rowid DESC
                        LIMIT ?2
                     )
                     ORDER BY timestamp_ms IS NULL, timestamp_ms, rowid",
                )
                .map_err(|error| format!("无法读取历史事件: {error}"))?;
            let rows = statement
                .query_map(params![record.id, MAX_RESTORED_SESSION_EVENTS], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| format!("无法读取历史事件: {error}"))?;
            let mut events = rows
                .map(|row| {
                    row.map(|payload| decode(payload))
                        .map_err(|error| format!("历史事件格式无效: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Older cancelled turns saved the process, but not its visible text.
            // Recover only text columns, leaving large tool evidence lazy-loaded.
            for event in &mut events {
                if !Self::is_durable_terminal(event) {
                    continue;
                }
                let Some(turn_id) = Self::terminal_turn_id(event) else {
                    continue;
                };
                // Keep execution evidence lazy, but distinguish it from replies
                // and terminal diagnostics before offering a disclosure control.
                let has_execution_details: bool = self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_events
                     WHERE session_id = ?1
                       AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
                       AND json_extract(payload, '$.type') = 'process_item'
                       AND (
                         (json_extract(payload, '$.item.kind') = 'tool_call'
                          AND COALESCE(json_extract(payload, '$.item.name'), '') != 'update_task_plan')
                         OR (json_extract(payload, '$.item.data.type') IN ('reasoning_delta', 'steered')
                             AND TRIM(COALESCE(json_extract(payload, '$.item.data.text'), json_extract(payload, '$.item.data.message'), '')) != '')
                         OR json_extract(payload, '$.item.data.type') = 'file_snapshot_error'
                         OR (json_extract(payload, '$.item.data.type') = 'permission_requested'
                             AND COALESCE(json_extract(payload, '$.item.data.requestId'), json_extract(payload, '$.item.data.request_id'), '') != '')
                         OR (json_extract(payload, '$.item.data.type') = 'context'
                             AND COALESCE(json_extract(payload, '$.item.data.context_compressed'), json_extract(payload, '$.item.data.contextCompressed'), 0) = 1)
                       ))",
                    params![record.id, turn_id], |row| row.get(0),
                ).map_err(|error| error.to_string())?;
                event["has_execution_details"] = Value::Bool(has_execution_details);
                if event
                    .get("execution_started_at")
                    .and_then(Value::as_i64)
                    .is_none()
                {
                    if let Some(started_at) = self.connection.query_row(
                        "SELECT started_at FROM agent_runs WHERE session_id = ?1 AND turn_id = ?2",
                        params![record.id, turn_id], |row| row.get::<_, i64>(0),
                    ).optional().map_err(|error| error.to_string())? {
                        event["execution_started_at"] = Value::from(started_at);
                    }
                }
                self.hydrate_display_segments(&record.id, &turn_id, event)?;
                if matches!(Self::terminal_type(event), Some("cancelled" | "error"))
                    && event.get("display_text").and_then(Value::as_str).is_none()
                {
                    let mut text_statement = self.connection.prepare(
                        "SELECT COALESCE(json_extract(payload, '$.item.content'), '') FROM session_events
                         WHERE session_id = ?1 AND json_extract(payload, '$.turn_id') = ?2
                           AND json_extract(payload, '$.type') = 'process_item'
                           AND json_extract(payload, '$.item.kind') = 'agent_message'
                           AND json_extract(payload, '$.execution_started_at') = (
                               SELECT MAX(json_extract(payload, '$.execution_started_at')) FROM session_events
                               WHERE session_id = ?1 AND json_extract(payload, '$.turn_id') = ?2
                                 AND json_extract(payload, '$.type') = 'process_item')
                         ORDER BY json_extract(payload, '$.process_order'), rowid",
                    ).map_err(|error| error.to_string())?;
                    let text = text_statement
                        .query_map(params![record.id, turn_id], |row| row.get::<_, String>(0))
                        .map_err(|error| error.to_string())?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .filter(|part| !part.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if !text.trim().is_empty() {
                        event["display_text"] = Value::String(text);
                    }
                }
            }
            if !events.is_empty() {
                record.events = Some(Value::Array(events));
            }
            self.hydrate_interrupted_display(record)?;
            Self::hydrate_record_messages(record);
            self.hydrate_file_change_summaries(record)?;
        }
        Ok(())
    }

    pub(super) fn hydrate_usage(&self, record: &mut HistorySessionRecord) -> Result<(), String> {
        // Usage is independent of the bounded timeline window and WebView saves.
        let terminal: Option<(Option<String>, Option<String>)> = self.connection.query_row(
            "SELECT COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')),
                    json_extract(payload, '$.usage')
             FROM session_events WHERE session_id = ?1
               AND json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
               AND COALESCE(json_extract(payload, '$.terminal_persistence_failed'), 0) != 1
               AND (json_extract(payload, '$.durable_terminal') = 1 OR
                    (json_type(payload, '$.durable_terminal') IS NULL
                     AND json_type(payload, '$.sequence') = 'integer'
                     AND COALESCE(json_type(payload, '$.eventId'), json_type(payload, '$.event_id')) = 'text'))
             ORDER BY COALESCE(timestamp_ms, 0) DESC, rowid DESC LIMIT 1",
            params![record.id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|error| error.to_string())?;
        let Some((turn_id, usage)) = terminal else {
            return Ok(());
        };
        let same_turn = record
            .metadata
            .get("usageTurnId")
            .and_then(Value::as_str)
            .zip(turn_id.as_deref())
            .is_some_and(|(saved, turn)| saved == turn);
        let mut recovered = if same_turn { record.usage.take() } else { None };
        if let Some(Value::Object(reported)) = usage.map(decode) {
            let mut merged = recovered.map(canonical_history_usage).unwrap_or_default();
            merged.extend(canonical_history_usage(Value::Object(reported)));
            recovered = (!merged.is_empty()).then_some(Value::Object(merged));
        }
        record.usage = recovered;
        if record.metadata.is_object() {
            record.metadata["usageTurnId"] = turn_id.map(Value::String).unwrap_or(Value::Null);
        }
        Ok(())
    }

    pub(super) fn session_usage_totals(
        &self,
        session_id: &str,
    ) -> Result<HistoryUsageTotals, String> {
        // Use the terminal index and extract only usage, never load tool output or transcript text.
        let mut statement = self.connection.prepare(
            "SELECT COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')),
                    COALESCE(json_extract(payload, '$.execution_started_at'), 0),
                    json_extract(payload, '$.usage')
             FROM session_events WHERE session_id = ?1
               AND json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
               AND COALESCE(json_extract(payload, '$.terminal_persistence_failed'), 0) != 1
               AND (json_extract(payload, '$.durable_terminal') = 1 OR
                    (json_type(payload, '$.durable_terminal') IS NULL
                     AND json_type(payload, '$.sequence') = 'integer'
                     AND COALESCE(json_type(payload, '$.eventId'), json_type(payload, '$.event_id')) = 'text'))
             ORDER BY COALESCE(timestamp_ms, 0), rowid"
        ).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut executions =
            std::collections::BTreeMap::<(String, i64), HistoryUsageExecution>::new();
        for row in rows {
            let (turn_id, started_at, usage) = row.map_err(|error| error.to_string())?;
            let Some(turn_id) = turn_id.filter(|id| !id.is_empty()) else {
                continue;
            };
            let (input_tokens, output_tokens, reasoning_tokens) = usage
                .map(decode)
                .as_ref()
                .map(history_usage_tokens)
                .unwrap_or((None, None, None));
            let previous = executions.get(&(turn_id.clone(), started_at));
            let input_tokens =
                input_tokens.or_else(|| previous.and_then(|value| value.input_tokens));
            let output_tokens =
                output_tokens.or_else(|| previous.and_then(|value| value.output_tokens));
            let reasoning_tokens =
                reasoning_tokens.or_else(|| previous.and_then(|value| value.reasoning_tokens));
            executions.insert(
                (turn_id.clone(), started_at),
                HistoryUsageExecution {
                    turn_id,
                    started_at,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                },
            );
        }
        let mut total = HistoryUsageTotals::default();
        for execution in executions.into_values() {
            if let Some(input) = execution.input_tokens {
                total.input_tokens =
                    Some(total.input_tokens.unwrap_or_default().saturating_add(input));
            }
            if let Some(output) = execution.output_tokens {
                total.output_tokens = Some(
                    total
                        .output_tokens
                        .unwrap_or_default()
                        .saturating_add(output),
                );
            }
            // Only turns that actually reported reasoning contribute; a session
            // with no report keeps `None` instead of a misleading 0.
            if let Some(reasoning) = execution.reasoning_tokens {
                total.reasoning_tokens = Some(
                    total
                        .reasoning_tokens
                        .unwrap_or_default()
                        .saturating_add(reasoning),
                );
            }
            total.executions.push(execution);
        }
        Ok(total)
    }

    /// Restore terminal display state. Full execution details are loaded lazily.
    pub(super) fn hydrate_record_messages(record: &mut HistorySessionRecord) {
        let Some(messages) = record.messages.as_array_mut() else {
            return;
        };
        let events = record
            .events
            .as_ref()
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owners: HashMap<String, usize> = messages.iter().enumerate().filter_map(|(index, message)| {
            if message.get("role").and_then(Value::as_str) != Some("assistant") { return None; }
            Some((message.get("turnId").or_else(|| message.get("turn_id"))?.as_str()?.to_string(), index))
        }).collect();
        let mut known_keys = HashMap::<usize, HashSet<String>>::new();
        for (event_index, event) in events.into_iter().enumerate() {
            let turn_id = event
                .get("turn_id")
                .or_else(|| event.get("turnId"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let Some(index) = turn_id.and_then(|turn_id| owners.get(turn_id)).copied() else { continue; };
            let message = &mut messages[index];
            let Some(message) = message.as_object_mut() else {
                continue;
            };
            let event_key = history_event_key(&event, event_index);
            let message_events = message
                .entry("events".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(existing) = message_events.as_array_mut() {
                let keys = known_keys.entry(index).or_insert_with(|| existing.iter().enumerate()
                    .map(|(index, value)| history_event_key(value, index)).collect());
                if keys.insert(event_key) {
                    existing.push(event.clone());
                }
            }
            if !Self::is_durable_terminal(&event)
                && event.get("type").and_then(Value::as_str) != Some("interrupted") {
                continue;
            }
            message.insert("pending".to_string(), Value::Bool(false));
            if let Some(started_at) = event.get("execution_started_at").and_then(Value::as_i64) {
                message.insert("startedAt".to_string(), Value::from(started_at));
            }
            if let Some(timestamp) = event.get("timestamp_ms").and_then(Value::as_i64) {
                message.insert("finishedAt".to_string(), Value::from(timestamp));
            }
            match event.get("type").and_then(Value::as_str) {
                Some("completed" | "interrupted") => {
                    if let Some(text) = event
                        .get("display_text")
                        .or_else(|| event.get("text"))
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        message.insert("content".to_string(), Value::String(text.to_string()));
                    }
                    message.insert("error".to_string(), Value::Bool(false));
                }
                Some("cancelled") => {
                    if let Some(text) = event
                        .get("display_text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        message.insert("content".to_string(), Value::String(text.to_string()));
                    }
                    if message
                        .get("content")
                        .and_then(Value::as_str)
                        .is_none_or(|content| content.trim().is_empty())
                    {
                        message
                            .insert("content".to_string(), Value::String("已停止。".to_string()));
                    }
                }
                Some("error") => {
                    if let Some(text) = event
                        .get("display_text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                    {
                        message.insert("content".to_string(), Value::String(text.to_string()));
                    }
                    if message
                        .get("content")
                        .and_then(Value::as_str)
                        .is_none_or(|content| content.trim().is_empty())
                    {
                        let detail = event
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("任务执行失败。");
                        message.insert("content".to_string(), Value::String(detail.to_string()));
                    }
                    message.insert("error".to_string(), Value::Bool(true));
                }
                _ => {}
            }
        }
    }

    pub fn export_json(&self) -> Result<String, String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| format!("无法开始历史导出: {error}"))?;
        let mut sessions = self.list_records(false)?;
        sessions.extend(self.list_records(true)?);
        for session in &mut sessions {
            let mut statement = self
                .connection
                .prepare("SELECT payload FROM session_events WHERE session_id = ?1 ORDER BY rowid")
                .map_err(|error| format!("无法读取完整历史事件: {error}"))?;
            let events = statement
                .query_map(params![session.id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .map(|row| row.map(decode))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| error.to_string())?;
            if !events.is_empty() {
                session.events = Some(Value::Array(events));
            }
        }
        let result = serde_json::to_string_pretty(&sessions)
            .map_err(|error| format!("无法导出历史会话: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法结束历史导出: {error}"))?;
        Ok(result)
    }
}
