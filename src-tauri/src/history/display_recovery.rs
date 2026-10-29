use super::*;

impl HistoryStore {
    // Restore visible text independently of a terminal event. Process evidence
    // stays paged; only text/attachment metadata is projected into message rows.
    pub(super) fn hydrate_display_segments(
        &self,
        session_id: &str,
        turn_id: &str,
        event: &mut Value,
    ) -> Result<(), String> {
        if event
            .get("display_segments")
            .and_then(Value::as_array)
            .is_none()
        {
            let mut segment_statement = self.connection.prepare(
            "SELECT json_extract(payload, '$.item.kind'), json_extract(payload, '$.item.id'),
                    COALESCE(json_extract(payload, '$.item.content'), json_extract(payload, '$.item.data.message'), ''),
                    COALESCE(json_extract(payload, '$.process_order'), 0),
                    json_extract(payload, '$.item.data.type'),
                    COALESCE(json_extract(payload, '$.item.attachments'), json_extract(payload, '$.item.data.attachments')),
                    COALESCE(json_extract(payload, '$.item.artifacts'), json_extract(payload, '$.item.data.artifacts'))
             FROM session_events WHERE session_id = ?1 AND json_extract(payload, '$.turn_id') = ?2
               AND json_extract(payload, '$.type') = 'process_item'
               AND json_type(payload, '$.item.kind') = 'text'
               AND json_type(payload, '$.item.id') = 'text'
               AND json_extract(payload, '$.execution_started_at') = (
                   SELECT MAX(json_extract(payload, '$.execution_started_at')) FROM session_events
                   WHERE session_id = ?1 AND json_extract(payload, '$.turn_id') = ?2
                     AND json_extract(payload, '$.type') = 'process_item')
             ORDER BY json_extract(payload, '$.process_order'), rowid",
        ).map_err(|error| error.to_string())?;
            let parts = segment_statement
                .query_map(params![session_id, turn_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let mut segments = Vec::<Value>::new();
            let mut assistant_text = Vec::<String>::new();
            let mut start_order = 0i64;
            let mut has_user = false;
            for (kind, id, text, order, data_type, attachments, artifacts) in &parts {
                let is_user = kind == "user_message"
                    || (kind == "timeline_event" && data_type.as_deref() == Some("steered"));
                if is_user {
                    has_user = true;
                    if *order > start_order {
                        segments.push(serde_json::json!({ "id":format!("assistant:{turn_id}:segment:{}", segments.len()), "role":"assistant", "content":assistant_text.join("\n\n"), "process_start":start_order, "process_end":order }));
                    }
                    assistant_text.clear();
                    let mut artifact_metadata = artifacts
                        .clone()
                        .map(decode)
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    for artifact in &mut artifact_metadata {
                        if let Some(object) = artifact.as_object_mut() {
                            object.remove("content");
                        }
                    }
                    segments.push(serde_json::json!({ "id":if kind == "user_message" { id.clone() } else { format!("user:{turn_id}:steer:{id}") }, "role":"user", "content":text, "attachments":attachments.clone().map(decode).unwrap_or_else(|| serde_json::json!([])), "artifacts":artifact_metadata, "process_start":order, "process_end":order + 1 }));
                    start_order = order + 1;
                } else if kind == "agent_message" && !text.trim().is_empty() {
                    assistant_text.push(text.clone());
                }
            }
            if event.get("type").and_then(Value::as_str) == Some("interrupted") {
                let text = parts
                    .iter()
                    .filter(|part| part.0 == "agent_message")
                    .map(|part| part.2.as_str())
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !text.trim().is_empty() {
                    event["display_text"] = Value::String(text);
                }
            }
            if has_user {
                if let Some((_, _, _, last_order, _, _, _)) = parts.last() {
                    if *last_order >= start_order {
                        segments.push(serde_json::json!({ "id":format!("assistant:{turn_id}:segment:{}", segments.len()), "role":"assistant", "content":assistant_text.join("\n\n"), "process_start":start_order, "process_end":last_order + 1 }));
                    }
                }
                event["display_segments"] = Value::Array(segments);
            }
        }
        Ok(())
    }

    pub(super) fn hydrate_interrupted_display(
        &self,
        record: &mut HistorySessionRecord,
    ) -> Result<(), String> {
        record.agent_runs = self.list_agent_run_summaries(&record.id)?;
        let mut events = record
            .events
            .take()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        // Rebuild read-only recovery projections, never store them as terminals.
        events.retain(|event| event.get("type").and_then(Value::as_str) != Some("interrupted"));
        for message in record.messages.as_array_mut().into_iter().flatten() {
            if let Some(saved) = message.get_mut("events").and_then(Value::as_array_mut) {
                saved.retain(|event| {
                    event.get("type").and_then(Value::as_str) != Some("interrupted")
                });
            }
        }
        for run in record
            .agent_runs
            .iter()
            .filter(|run| run.status == "interrupted")
        {
            if events.iter().any(|event| {
                Self::is_durable_terminal(event)
                    && Self::terminal_turn_id(event).as_deref() == Some(run.turn_id.as_str())
            }) {
                continue;
            }
            let timestamp = events
                .iter()
                .chain(
                    record
                        .messages
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|message| {
                            message.get("turnId").and_then(Value::as_str)
                                == Some(run.turn_id.as_str())
                        })
                        .filter_map(|message| message.get("events").and_then(Value::as_array))
                        .flatten(),
                )
                .filter(|event| {
                    event
                        .get("turn_id")
                        .or_else(|| event.get("turnId"))
                        .and_then(Value::as_str)
                        .is_none_or(|id| id == run.turn_id)
                })
                .filter_map(|event| {
                    event
                        .get("timestamp_ms")
                        .or_else(|| event.get("timestampMs"))
                        .and_then(Value::as_i64)
                })
                .max()
                .unwrap_or(run.started_at)
                .max(run.started_at);
            let mut event = serde_json::json!({
                "type":"interrupted", "turn_id":run.turn_id,
                "event_id":format!("history-interrupted:{}:{}", run.turn_id, run.updated_at),
                "execution_started_at":run.started_at, "timestamp_ms":timestamp,
                "recovery_synthetic":true,
            });
            self.hydrate_display_segments(&record.id, &run.turn_id, &mut event)?;
            if event.get("display_text").and_then(Value::as_str).is_some() {
                // Old WebView snapshots must not override the complete native
                // text when this legacy process has no user/display segments.
                for message in
                    record
                        .messages
                        .as_array_mut()
                        .into_iter()
                        .flatten()
                        .filter(|message| {
                            message
                                .get("turnId")
                                .or_else(|| message.get("turn_id"))
                                .and_then(Value::as_str)
                                == Some(run.turn_id.as_str())
                        })
                {
                    if let Some(saved) = message.get_mut("events").and_then(Value::as_array_mut) {
                        saved.retain(|event| {
                            !matches!(
                                event.get("type").and_then(Value::as_str),
                                Some("snapshot_text" | "text_delta")
                            )
                        });
                    }
                }
                events.retain(|event| {
                    !(event
                        .get("turn_id")
                        .or_else(|| event.get("turnId"))
                        .and_then(Value::as_str)
                        == Some(run.turn_id.as_str())
                        && matches!(
                            event.get("type").and_then(Value::as_str),
                            Some("snapshot_text" | "text_delta")
                        ))
                });
            }
            let has_details: bool = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id = ?1
                 AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
                 AND json_extract(payload, '$.type') = 'process_item')",
                params![record.id, run.turn_id], |row| row.get(0),
            ).map_err(|error| error.to_string())?;
            event["has_execution_details"] = Value::Bool(has_details);
            events.push(event);
        }
        if !events.is_empty() {
            record.events = Some(Value::Array(events));
        }
        Ok(())
    }
}
