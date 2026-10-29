use super::*;

fn decoded_object(value: &Value) -> Value {
    value.as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_else(|| value.clone())
}

fn has_files(event: &Value) -> bool {
    ["entries", "created", "modified", "deleted"].iter()
        .any(|field| event[*field].as_array().is_some_and(|files| !files.is_empty()))
}

impl HistoryStore {
    /// Reply cards are durable state, not lazy execution details or a bounded
    /// event tail. Read only file summaries and explicit write receipts.
    pub(super) fn hydrate_file_change_summaries(&self, record: &mut HistorySessionRecord) -> Result<(), String> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM session_events WHERE session_id = ?1 AND (
                json_extract(payload, '$.type') = 'file_changes' OR
                (json_extract(payload, '$.type') = 'process_item' AND json_extract(payload, '$.item.kind') = 'file_changes') OR
                (json_extract(payload, '$.type') = 'tool_call_finished' AND
                 COALESCE(json_extract(payload, '$.toolName'), json_extract(payload, '$.tool_name')) IN ('apply_patch', 'write_local_file', 'windows__apply_patch', 'windows__write_local_file')) OR
                (json_extract(payload, '$.type') = 'process_item' AND json_extract(payload, '$.item.kind') = 'tool_call' AND
                 json_extract(payload, '$.item.name') IN ('apply_patch', 'write_local_file', 'windows__apply_patch', 'windows__write_local_file'))
             ) ORDER BY COALESCE(timestamp_ms, 0), rowid"
        ).map_err(|error| error.to_string())?;
        let rows = statement.query_map(params![record.id], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?;
        let mut summaries = HashMap::<String, Value>::new();
        let mut writes = HashMap::<String, std::collections::BTreeSet<String>>::new();
        for row in rows {
            let event = decode(row.map_err(|error| error.to_string())?);
            let Some(turn) = event.get("turn_id").or_else(|| event.get("turnId")).and_then(Value::as_str) else { continue; };
            let item = &event["item"];
            if event["type"] == "file_changes" || item["kind"] == "file_changes" {
                let mut data = if event["type"] == "file_changes" { event.clone() } else { item["data"].clone() };
                data["type"] = Value::from("file_changes");
                data["session_id"] = Value::from(record.id.clone());
                data["turn_id"] = Value::from(turn);
                summaries.insert(turn.to_string(), data);
                continue;
            }
            let receipt = if event["type"] == "tool_call_finished" { &event } else { item };
            if ["is_error", "isError", "error"].iter().any(|key| receipt[*key] == true) { continue; }
            let output = decoded_object(&receipt["output"]);
            if !output.is_object() || !output["error"].is_null() || output["skipped"] == true || output["interrupted"] == true { continue; }
            let name = receipt.get("name").or_else(|| receipt.get("toolName")).or_else(|| receipt.get("tool_name")).and_then(Value::as_str).unwrap_or_default();
            if name.ends_with("apply_patch") && output["appliedHunks"].as_u64().unwrap_or(0) == 0 { continue; }
            if let Some(path) = output["path"].as_str().filter(|path| !path.trim().is_empty()) {
                writes.entry(turn.to_string()).or_default().insert(path.to_string());
            }
        }
        for (turn, paths) in writes {
            if summaries.get(&turn).is_some_and(|event| has_files(event) || event["snapshot_complete"] == true) { continue; }
            summaries.insert(turn.clone(), serde_json::json!({
                "type":"file_changes", "session_id":record.id, "turn_id":turn,
                "modified":paths, "review_available":false, "line_counts_available":false,
            }));
        }
        // Index the last assistant segment once instead of scanning a long
        // transcript again for every historical turn.
        let Some(messages) = record.messages.as_array_mut() else { return Ok(()); };
        let owners: HashMap<String, usize> = messages.iter().enumerate().filter_map(|(index, message)| {
            if message["role"] != "assistant" { return None; }
            Some((message.get("turnId").or_else(|| message.get("turn_id"))?.as_str()?.to_string(), index))
        }).collect();
        for (turn, event) in summaries {
            let Some(index) = owners.get(&turn) else { continue; };
            let events = messages[*index].as_object_mut().unwrap().entry("events").or_insert_with(|| serde_json::json!([]));
            if let Some(events) = events.as_array_mut() {
                events.retain(|event| event["type"] != "file_changes");
                events.push(event);
            }
        }
        Ok(())
    }
}
