use super::*;

pub(super) fn parse_agent_context_checkpoint(value: &Value) -> Option<AgentContextCheckpoint> {
    serde_json::from_value(value.clone()).ok()
}

pub(super) fn agent_context_checkpoint_candidates(
    record: &HistorySessionRecord,
) -> Vec<AgentContextCheckpointCandidate> {
    let mut candidates = Vec::new();
    let mut seen_ids = HashSet::new();
    if let Some(events) = record.events.as_ref().and_then(Value::as_array) {
        for event in events.iter().rev() {
            let durable_completion = HistoryStore::terminal_type(event) == Some("completed")
                && HistoryStore::is_durable_terminal(event);
            let completed_compaction = event["type"] == "context"
                && event["compaction_status"] == "completed";
            if !durable_completion && !completed_compaction {
                continue;
            }
            let Some(checkpoint) = event
                .get("contextCheckpoint")
                .or_else(|| event.get("context_checkpoint"))
                .and_then(parse_agent_context_checkpoint)
            else {
                continue;
            };
            if seen_ids.insert(checkpoint.id.clone()) {
                candidates.push(AgentContextCheckpointCandidate { checkpoint });
            }
        }
    }
    if let Some(checkpoint) = record
        .metadata
        .get("contextCheckpoint")
        .or_else(|| record.metadata.get("context_checkpoint"))
        .and_then(parse_agent_context_checkpoint)
    {
        if seen_ids.insert(checkpoint.id.clone()) {
            candidates.push(AgentContextCheckpointCandidate { checkpoint });
        }
    }
    candidates
}

pub(super) fn checkpoint_tail_start(
    record: &HistorySessionRecord,
    candidate: &AgentContextCheckpointCandidate,
) -> Option<usize> {
    let messages = ordered_context_messages(record);
    let covered_id = candidate
        .checkpoint
        .covered_through_message_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let exact_boundary = covered_id.and_then(|covered_id| {
        messages
            .iter()
            .position(|message| message.get("id").and_then(Value::as_str) == Some(covered_id))
            .map(|index| index + 1)
    });
    if exact_boundary.is_some() {
        return exact_boundary;
    }

    // Older optimistic/runtime messages can receive a different durable ID
    // after restart. Checkpoint entries retain source IDs from the covered
    // prefix, so use the latest one still present as a conservative boundary.
    // This may replay a small amount of raw history, but avoids discarding the
    // checkpoint and rebuilding a large tool transcript.
    let source_ids = candidate
        .checkpoint
        .payload
        .as_ref()
        .into_iter()
        .flat_map(|payload| payload.entries.iter())
        .filter_map(|entry| match entry {
            agent_core::AgentContextCheckpointEntry::SystemConstraint { source_message_id, .. }
            | agent_core::AgentContextCheckpointEntry::UserConstraint { source_message_id, .. }
            | agent_core::AgentContextCheckpointEntry::AssistantConclusion { source_message_id, .. }
            | agent_core::AgentContextCheckpointEntry::ToolEvidence { source_message_id, .. } =>
                source_message_id.as_deref(),
        })
        .collect::<HashSet<_>>();
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.get("id").and_then(Value::as_str)
                .is_some_and(|id| source_ids.contains(id))
        })
        .map(|(index, _)| index + 1)
        .max()
}

pub(super) fn raw_agent_context_turn(message: &Value) -> Option<AgentContextTurn> {
    let value = message.as_object()?;
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let role = value.get("role").and_then(Value::as_str).unwrap_or("user");
    let artifacts = agent_core::user_local_source_metadata(message);
    let attachments = if role == "user" {
        value
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|image| {
                let data_url = image.get("dataUrl").and_then(Value::as_str).unwrap_or_default();
                let path = if data_url.is_empty() {
                    image.get("imagePath").or_else(|| image.get("path")).and_then(Value::as_str)
                } else { None };
                let media_type = image.get("mediaType").and_then(Value::as_str).or_else(|| {
                    data_url
                        .strip_prefix("data:")?
                        .split_once(';')
                        .map(|(mime, _)| mime)
                })?;
                if !media_type.starts_with("image/") {
                    return None;
                }
                Some(agent_core::AgentImageAttachment {
                    path: path.map(str::to_string),
                    name: image
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("image")
                        .to_string(),
                    media_type: media_type.to_string(),
                    data_url: data_url.to_string(),
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if content.is_empty() && attachments.is_empty() && artifacts.is_empty() {
        return None;
    }
    let message_id = value.get("id").and_then(Value::as_str).map(str::to_string);
    let turn_id = value
        .get("turnId")
        .or_else(|| value.get("turn_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let content = if role == "assistant" {
        format!("[历史助手陈述，非工具证据]\n{content}")
    } else if content.is_empty() && !artifacts.is_empty() {
        "[历史用户本机来源]".to_string()
    } else if content.is_empty() {
        "[历史用户图片附件]".to_string()
    } else {
        content.to_string()
    };
    Some(AgentContextTurn {
        role: role.to_string(),
        content,
        source: Some("history".to_string()),
        message_id,
        turn_id,
        model_id: None,
        attachments,
        artifacts,
    })
}

pub(super) fn ordered_context_messages(record: &HistorySessionRecord) -> Vec<Value> {
    let messages = record.messages.as_array().cloned().unwrap_or_default();
    let mut segments_by_turn = HashMap::<String, Vec<Value>>::new();
    for event in record
        .events
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !HistoryStore::is_durable_terminal(event) {
            continue;
        }
        let Some(turn_id) = HistoryStore::terminal_turn_id(event) else {
            continue;
        };
        let Some(segments) = event.get("display_segments").and_then(Value::as_array) else {
            continue;
        };
        if segments.iter().all(|segment| {
            matches!(
                segment.get("role").and_then(Value::as_str),
                Some("user" | "assistant")
            ) && segment.get("content").and_then(Value::as_str).is_some()
        }) && !segments.is_empty()
        {
            segments_by_turn.insert(turn_id, segments.clone());
        }
    }
    let mut emitted = HashSet::new();
    let mut ordered = Vec::new();
    for message in &messages {
        let turn_id = message
            .get("turnId")
            .or_else(|| message.get("turn_id"))
            .and_then(Value::as_str);
        let segments = turn_id.and_then(|turn_id| segments_by_turn.get(turn_id));
        let (Some(turn_id), Some(segments)) = (turn_id, segments) else {
            ordered.push(message.clone());
            continue;
        };
        if !emitted.insert(turn_id.to_string()) {
            continue;
        }
        let mut initial_media = None;
        // Legacy process rows omitted the initial user item. Retain it without
        // duplicating users already present in the authoritative segment list.
        for original in messages
            .iter()
            .filter(|candidate| {
                candidate
                    .get("turnId")
                    .or_else(|| candidate.get("turn_id"))
                    .and_then(Value::as_str)
                    == Some(turn_id)
                    && candidate.get("role").and_then(Value::as_str) == Some("user")
            })
            .take(1)
        {
            let represented = segments.iter().any(|segment| {
                segment.get("role").and_then(Value::as_str) == Some("user")
                    && segment.get("id").is_some()
                    && segment.get("id") == original.get("id")
            }) || segments.first().is_some_and(|segment| {
                segment.get("role").and_then(Value::as_str) == Some("user")
                    && segment.get("content") == original.get("content")
            });
            if !represented {
                ordered.push(original.clone());
            } else {
                initial_media = Some(original);
            }
        }
        for (index, segment) in segments.iter().enumerate() {
            let mut segment = segment.clone();
            if index == 0 && segment.get("role").and_then(Value::as_str) == Some("user") {
                if let Some(original) = initial_media {
                    for field in ["attachments", "artifacts"] {
                        if segment
                            .get(field)
                            .and_then(Value::as_array)
                            .is_none_or(Vec::is_empty)
                        {
                            if let Some(media) = original.get(field) {
                                segment[field] = media.clone();
                            }
                        }
                    }
                }
            }
            segment["turnId"] = Value::String(turn_id.to_string());
            ordered.push(segment);
        }
    }
    ordered
}

pub(super) fn raw_agent_context_from_record(
    record: &HistorySessionRecord,
) -> Vec<AgentContextTurn> {
    ordered_context_messages(record)
        .iter()
        .filter_map(raw_agent_context_turn)
        .collect()
}

pub(super) fn append_raw_agent_message_tail(
    output: &mut Vec<AgentMessage>,
    record: &HistorySessionRecord,
    start: usize,
    current_turn_id: Option<&str>,
) {
    let messages = ordered_context_messages(record);
    let mut evidence_turns = HashSet::new();
    let mut runtime_turns = HashSet::new();
    if start > 0 {
        if let Some(turn) = messages
            .get(start - 1)
            .and_then(|message| message.get("turnId").or_else(|| message.get("turn_id")))
            .and_then(Value::as_str)
        {
            // The checkpoint already summarizes the covered portion of this
            // turn. Replaying the turn-wide tool list would undo compaction.
            evidence_turns.insert(turn.to_string());
        }
    }
    let mut previous_turn: Option<&str> = None;
    for message in messages.iter().skip(start) {
        let turn = message.get("turnId").or_else(|| message.get("turn_id")).and_then(Value::as_str);
        if previous_turn != turn {
            if let Some(previous) = previous_turn {
                append_historical_tool_results(output, record, previous, &mut evidence_turns);
            }
            previous_turn = turn.filter(|turn| Some(*turn) != current_turn_id);
        }
        let belongs_to_current_turn = current_turn_id.is_some_and(|current_turn_id| {
            message
                .get("turnId")
                .or_else(|| message.get("turn_id"))
                .and_then(Value::as_str)
                == Some(current_turn_id)
        });
        if belongs_to_current_turn {
            continue;
        }
        if let Some(turn) = turn {
            if runtime_turns.contains(turn) { continue; }
            // A checkpoint can end inside a turn. Replaying that turn's whole
            // runtime prefix would duplicate its already-covered observations.
            let boundary_inside_turn = start > 0 && messages.get(start - 1)
                .and_then(|message| message.get("turnId").or_else(|| message.get("turn_id")))
                .and_then(Value::as_str) == Some(turn);
            if !boundary_inside_turn {
                if let Some(mut replayed) = agent_core::replay_runtime_turn(
                    record.events.as_ref().and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), turn,
                ) {
                    // Old optimistic user IDs can differ from native IDs after
                    // a crash. Keep image ownership aligned with raw context.
                    for restored in replayed.iter_mut().filter(|message| message.role == agent_core::MessageRole::User) {
                        let original = messages.iter().find(|message| {
                            message.get("turnId").or_else(|| message.get("turn_id")).and_then(Value::as_str) == Some(turn)
                                && message["role"] == "user"
                                && message["id"].as_str() == restored.source_message_id.as_deref()
                        }).or_else(|| {
                            (restored.source_message_id.as_deref() == Some(format!("user:{turn}").as_str()))
                                .then(|| messages.iter().find(|message| {
                                    message.get("turnId").or_else(|| message.get("turn_id")).and_then(Value::as_str) == Some(turn)
                                        && message["role"] == "user"
                                })).flatten()
                        });
                        if let Some(id) = original.and_then(|message| message["id"].as_str()) {
                            restored.source_message_id = Some(id.to_string());
                        }
                    }
                    output.extend(replayed);
                    runtime_turns.insert(turn.to_string());
                    evidence_turns.insert(turn.to_string());
                    continue;
                }
            }
        }
        if let Some(message) =
            raw_agent_context_turn(message).and_then(|turn| agent_core::context_turn_message(&turn))
        {
            output.push(message);
        }
    }
    if let Some(previous) = previous_turn {
        append_historical_tool_results(output, record, previous, &mut evidence_turns);
    }
}

// When a native snapshot is unavailable (older app, changed route, interrupted
// turn), display text alone loses the actual file results. Recover only durable
// successful tool events, as paired messages; never infer evidence from prose
// or reconstruct provider-private thinking/signatures.
fn append_historical_tool_results(
    output: &mut Vec<AgentMessage>,
    record: &HistorySessionRecord,
    turn: &str,
    emitted: &mut HashSet<String>,
) {
    if !emitted.insert(turn.to_string()) { return; }
    agent_core::append_historical_tool_results(output,
        record.events.as_ref().and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), turn);
}

pub(super) fn agent_context_from_record(
    record: &HistorySessionRecord,
    current_model_id: &str,
    evidence_policy_version: u8,
    current_turn_id: Option<&str>,
) -> Option<Vec<AgentMessage>> {
    let mut legacy_fallback = None;
    for candidate in agent_context_checkpoint_candidates(record) {
        if candidate.checkpoint.evidence_policy_version != evidence_policy_version {
            continue;
        }
        let Some(tail_start) = checkpoint_tail_start(record, &candidate) else {
            // A declared boundary that is no longer present cannot be trimmed
            // safely. Ignore this checkpoint rather than duplicate or drop raw
            // transcript messages.
            continue;
        };
        if let Some(mut restored) =
            restore_context_checkpoint(&candidate.checkpoint, Some(current_model_id))
        {
            append_raw_agent_message_tail(&mut restored, record, tail_start, current_turn_id);
            return Some(restored);
        }
        if legacy_fallback.is_none() {
            if let Some(summary) = legacy_context_checkpoint_message(&candidate.checkpoint) {
                legacy_fallback = Some((summary, tail_start));
            }
        }
    }
    legacy_fallback.map(|(summary, tail_start)| {
        let mut restored = vec![summary];
        append_raw_agent_message_tail(&mut restored, record, tail_start, current_turn_id);
        restored
    }).or_else(|| {
        let mut restored = Vec::new();
        append_raw_agent_message_tail(&mut restored, record, 0, current_turn_id);
        let has_runtime_messages = record.events.as_ref().and_then(Value::as_array)
            .is_some_and(|events| events.iter().any(|event| event["type"] == "process_item"
                && event["item"]["kind"] == "agent_message"));
        (has_runtime_messages || restored.iter().any(|message| message.role == agent_core::MessageRole::Tool)).then_some(restored)
    })
}

impl HistoryStore {
    /// The UI restores a small event window. Provider recovery must not use that
    /// window: usage/progress events can evict a successful write or a terminal
    /// even when the corresponding conversation messages are still present.
    pub(super) fn model_context_record(
        &self,
        session_id: &str,
    ) -> Result<Option<HistorySessionRecord>, String> {
        let Some(mut record) = self.get_record(session_id)? else {
            return Ok(None);
        };
        let hydrated = record.events.as_ref().and_then(Value::as_array)
            .into_iter().flatten()
            .enumerate()
            .map(|(index, event)| (history_event_key(event, index), event))
            .collect::<HashMap<_, _>>();
        // Only model-relevant observations, never streaming progress. Context
        // compaction, rather than the UI's event count, applies the token budget.
        let mut statement = self.connection.prepare(
            "SELECT payload FROM session_events WHERE session_id = ?1
             AND (json_extract(payload, '$.type') IN ('tool_call_pending', 'tool_call_started', 'tool_call_finished')
                  OR (json_extract(payload, '$.type') = 'context'
                      AND json_extract(payload, '$.compaction_status') = 'completed'
                      AND json_type(payload, '$.contextCheckpoint') = 'object')
                  OR (json_extract(payload, '$.type') = 'process_item'
                      AND json_extract(payload, '$.item.kind') IN ('user_message', 'agent_message', 'tool_call'))
                  OR (json_extract(payload, '$.type') IN ('completed', 'cancelled', 'error')
                      AND json_extract(payload, '$.durable_terminal') = 1))
             ORDER BY timestamp_ms IS NULL, timestamp_ms, rowid",
        ).map_err(|error| format!("无法读取模型历史证据: {error}"))?;
        let rows = statement.query_map([session_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("无法读取模型历史证据: {error}"))?;
        let events = rows.enumerate().map(|(index, row)| {
            let event = decode(row.map_err(|error| error.to_string())?);
            // Keep recovered display segments and media on recent terminals.
            Ok(hydrated.get(&history_event_key(&event, index))
                .map(|event| (*event).clone()).unwrap_or(event))
        }).collect::<Result<Vec<Value>, String>>()?;
        record.events = Some(Value::Array(events));
        Ok(Some(record))
    }
}

pub(crate) fn restored_agent_messages_for_session(
    app: &AppHandle,
    session_id: &str,
    current_model_id: &str,
    evidence_policy_version: u8,
    current_turn_id: Option<&str>,
) -> Result<Option<Vec<AgentMessage>>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(record) = open_store(app)?.model_context_record(session_id)? else {
        return Ok(None);
    };
    Ok(agent_context_from_record(
        &record,
        current_model_id,
        evidence_policy_version,
        current_turn_id,
    ))
}
