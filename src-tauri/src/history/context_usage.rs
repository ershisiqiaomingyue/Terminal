use super::*;

impl HistoryStore {
    /// Context totals are state, independent of the bounded visual event tail.
    /// Process checkpoints hold the final streaming reading even though the
    /// per-token context events themselves are deliberately not persisted.
    pub(super) fn hydrate_context_usage(&self, record: &mut HistorySessionRecord) -> Result<(), String> {
        let mut statement = self.connection.prepare(
            "WITH contexts AS (
                SELECT CASE WHEN json_extract(payload, '$.type') = 'context'
                            THEN payload ELSE json_extract(payload, '$.item.data') END AS data,
                       timestamp_ms, rowid AS event_row
                FROM session_events WHERE session_id = ?1 AND (
                    json_extract(payload, '$.type') = 'context' OR
                    (json_extract(payload, '$.type') = 'process_item'
                     AND json_extract(payload, '$.item.kind') = 'context'))
             ), ranked AS (
                SELECT data,
                       COALESCE(json_extract(data, '$.timestamp_ms'), json_extract(data, '$.timestampMs'), timestamp_ms, 0) AS measured_at,
                       event_row,
                       ROW_NUMBER() OVER (
                           PARTITION BY COALESCE(json_extract(data, '$.turn_id'), json_extract(data, '$.turnId')),
                                        json_extract(data, '$.provider_id'), json_extract(data, '$.model_id')
                           ORDER BY COALESCE(json_extract(data, '$.timestamp_ms'), json_extract(data, '$.timestampMs'), timestamp_ms, 0) DESC, event_row DESC
                       ) AS position
                FROM contexts
                WHERE json_type(data, '$.system_prompt_tokens') IN ('integer', 'real')
                  AND json_type(data, '$.tool_definition_tokens') IN ('integer', 'real')
                  AND json_type(data, '$.context_tokens') IN ('integer', 'real')
                  AND json_extract(data, '$.system_prompt_tokens') >= 0
                  AND json_extract(data, '$.tool_definition_tokens') >= 0
                  AND json_extract(data, '$.context_tokens') >= json_extract(data, '$.system_prompt_tokens') + json_extract(data, '$.tool_definition_tokens')
                  AND json_type(data, '$.model_id') = 'text'
                  AND json_type(data, '$.provider_id') = 'text'
             ) SELECT data FROM ranked WHERE position = 1
               ORDER BY measured_at DESC, event_row DESC LIMIT 16"
        ).map_err(|error| format!("无法恢复上下文分类用量: {error}"))?;
        let rows = statement.query_map(params![record.id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut events = rows.map(|row| row.map(decode).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        if !events.is_empty() {
            events.reverse();
            if !record.metadata.is_object() { record.metadata = serde_json::json!({}); }
            record.metadata["contextUsageEvents"] = Value::Array(events);
        }
        Ok(())
    }
}
