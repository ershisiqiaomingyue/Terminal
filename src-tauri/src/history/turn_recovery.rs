use super::*;

pub(crate) fn session_binding_for_open(app: &AppHandle, id: &str) -> Result<SessionBinding, String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    let store = open_store(app)?;
    store.ensure_not_deleted(id)?;
    store
        .connection
        .execute(
            "INSERT OR IGNORE INTO session_bindings(session_id,storage_id) VALUES(?1,?1)",
            [id],
        )
        .map_err(|error| error.to_string())?;
    store.session_binding(id)?.ok_or("会话存储身份缺失".into())
}

pub(super) fn initialize_turn_recovery(connection: &Connection) -> Result<(), String> {
    let exists = |connection: &Connection| {
        connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'turn_recovery')",
        [], |row| row.get::<_, bool>(0)).map_err(|e|e.to_string())
    };
    if exists(connection)? {
        return Ok(());
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(connection, rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
    // Recheck under the write transaction; concurrent opens must not bootstrap twice.
    if !exists(&transaction)? {
        transaction
            .execute_batch(
                "CREATE TABLE turn_recovery (
            session_id TEXT PRIMARY KEY NOT NULL, needs_recovery INTEGER NOT NULL DEFAULT 1,
            settled_receipt TEXT)",
            )
            .map_err(|e| e.to_string())?;
        // Creating the table and seeding old sessions commit together.
        transaction
            .execute(
                "INSERT INTO turn_recovery(session_id)
            SELECT DISTINCT session_id FROM turn_outbox",
                [],
            )
            .map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())
}

pub(crate) fn put_turn_outbox(
    app: &AppHandle,
    session: &str,
    mode: &str,
    message: &Value,
) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    let store = open_store(app)?;
    put_turn_outbox_record(&store.connection, session, mode, message)
}

pub(super) fn put_turn_outbox_record(
    connection: &Connection,
    session: &str,
    mode: &str,
    message: &Value,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    let deleted: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_bindings WHERE session_id = ?1 AND deleted = 1)",
            [session],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if deleted {
        return Err("该对话已删除，未保存追加消息".into());
    }
    let id = message["id"].as_str().ok_or("入站消息缺少 ID")?;
    let previous: Option<String> = connection
        .query_row(
            "SELECT message FROM turn_outbox WHERE session_id = ?1 AND message_id = ?2",
            params![session, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let encoded = encode(message)?;
    if let Some(previous) = previous {
        if previous != encoded {
            return Err("入站消息 ID 已绑定另一条内容".into());
        }
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO turn_outbox(session_id,message_id,mode,message) VALUES(?1,?2,?3,?4)",
            params![session, id, mode, encoded],
        )
        .map_err(|e| e.to_string())?;
    transaction
        .execute(
            "INSERT INTO turn_recovery(session_id,needs_recovery) VALUES(?1,1)
        ON CONFLICT(session_id) DO UPDATE SET needs_recovery = 1, settled_receipt = NULL",
            [session],
        )
        .map_err(|e| e.to_string())?;
    transaction.commit().map_err(|e| e.to_string())
}

pub(crate) fn pending_turn_outbox(
    app: &AppHandle,
) -> Result<Vec<(String, String, Result<Value, String>)>, String> {
    let store = open_store(app)?;
    let mut statement = store
        .connection
        .prepare(
            "SELECT session_id,mode,message FROM turn_outbox WHERE accepted = 0 ORDER BY sequence",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(session, mode, message)| {
            (
                session,
                mode,
                serde_json::from_str(&message).map_err(|e| e.to_string()),
            )
        })
        .collect())
}

pub(crate) fn session_turn_requests(
    app: &AppHandle,
) -> Result<Vec<(String, Result<AgentChatRequest, String>)>, String> {
    let store = open_store(app)?;
    turn_recovery_requests(&store.connection)
}

pub(super) fn turn_recovery_requests(
    connection: &Connection,
) -> Result<Vec<(String, Result<AgentChatRequest, String>)>, String> {
    let mut statement=connection.prepare("SELECT recovery.session_id,outbox.message
        FROM turn_recovery AS recovery JOIN turn_outbox AS outbox
        ON outbox.sequence = (SELECT MAX(sequence) FROM turn_outbox WHERE session_id = recovery.session_id)
        WHERE recovery.needs_recovery = 1 ORDER BY outbox.sequence")
        .map_err(|e|e.to_string())?;
    let messages = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(messages
        .into_iter()
        .map(|(session, message)| {
            let request = serde_json::from_str::<Value>(&message)
                .map_err(|e| e.to_string())
                .and_then(|message| {
                    serde_json::from_value(message["nativeRequest"].clone())
                        .map_err(|e| e.to_string())
                });
            (session, request)
        })
        .collect())
}

pub(crate) fn settle_turn_recovery(
    app: &AppHandle,
    session: &str,
    receipt: &str,
) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    settle_turn_recovery_record(&open_store(app)?.connection, session, receipt)
}

pub(super) fn settle_turn_recovery_record(
    connection: &Connection,
    session: &str,
    receipt: &str,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    transaction
        .execute(
            "UPDATE turn_recovery SET settled_receipt = ?2 WHERE session_id = ?1",
            params![session, receipt],
        )
        .map_err(|e| e.to_string())?;
    clear_settled_turn_recovery(&transaction, session)?;
    transaction.commit().map_err(|e| e.to_string())
}

pub(super) fn clear_settled_turn_recovery(
    connection: &Connection,
    session: &str,
) -> Result<(), String> {
    // A delayed idle frame cannot clear input submitted after that frame.
    connection.execute("UPDATE turn_recovery SET needs_recovery = 0 WHERE session_id = ?1
        AND NOT EXISTS (SELECT 1 FROM turn_outbox WHERE session_id = ?1 AND accepted = 0)
        AND settled_receipt = (SELECT message_id FROM turn_outbox WHERE session_id = ?1 ORDER BY sequence DESC LIMIT 1)",
        params![session]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(crate) fn retired_legacy_queue(app: &AppHandle, id: &str) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    open_store(app)?.connection.execute("UPDATE agent_queue SET status = 'migrated-inbound', lease_id = NULL, lease_owner = NULL, lease_expires_at = NULL WHERE id = ?1",[id])
        .map_err(|e|e.to_string())?;
    Ok(())
}

pub(crate) fn legacy_steer_requests(
    app: &AppHandle,
) -> Result<Vec<(String, String, Result<AgentChatRequest, String>)>, String> {
    legacy_steer_records(&open_store(app)?.connection)
}

pub(super) fn legacy_steer_records(
    connection: &Connection,
) -> Result<Vec<(String, String, Result<AgentChatRequest, String>)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT steer.session_id,steer.id,steer.request,run.request,steer.turn_id,
                EXISTS(SELECT 1 FROM turn_outbox AS outbox WHERE outbox.session_id = steer.session_id
                    AND outbox.message_id = 'user:legacy-steer-' || steer.id)
        FROM agent_steers AS steer LEFT JOIN agent_runs AS run
        ON run.session_id = steer.session_id AND run.turn_id = steer.turn_id
        WHERE steer.status IN ('pending','delivered')
          AND (run.status IS NULL OR run.status NOT IN ('completed','cancelled','failed'))
        ORDER BY steer.created_at,steer.id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(
            |(session, id, steer, original, original_turn, has_old_receipt)| {
                let request = (|| {
                    let steer: AgentSteerRequest =
                        serde_json::from_str(&steer).map_err(|error| error.to_string())?;
                    let mut request: AgentChatRequest = serde_json::from_str(
                        &original.ok_or("追加要求缺少原运行请求，未猜测模型或权限")?,
                    )
                    .map_err(|error| error.to_string())?;
                    if request
                        .session_id
                        .as_deref()
                        .is_some_and(|owner| owner != session)
                    {
                        return Err("追加要求的原请求属于另一会话".into());
                    }
                    request.session_id = Some(session.clone());
                    // Keep the original visible user id. An earlier cutover receipt
                    // must keep its old identity even if its acknowledgement was lost.
                    request.turn_id = Some(if has_old_receipt {
                        format!("legacy-steer-{id}")
                    } else {
                        format!("{original_turn}:steer:{id}")
                    });
                    request.message = steer.message;
                    request.attachments = steer.attachments;
                    for artifact in steer.artifacts {
                        if !request
                            .artifacts
                            .iter()
                            .any(|old| old.path == artifact.path)
                        {
                            request.artifacts.push(artifact);
                        }
                    }
                    request.context.clear();
                    Ok(request)
                })();
                (session, id, request)
            },
        )
        .collect())
}

pub(crate) fn retired_legacy_steer(app: &AppHandle, id: &str) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    retire_legacy_steer_record(&open_store(app)?.connection, id)
}

pub(super) fn retire_legacy_steer_record(
    connection: &Connection,
    id: &str,
) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE agent_steers SET status = 'migrated-inbound',updated_at = ?2
        WHERE id = ?1 AND status IN ('pending','delivered')",
            params![id, now_ms()],
        )
        .map_err(|error| error.to_string())?;
    if changed == 0 {
        let status: Option<String> = connection
            .query_row(
                "SELECT status FROM agent_steers WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if status.as_deref() != Some("migrated-inbound") {
            return Err("旧引导消息已变更，未覆盖其状态".into());
        }
    }
    Ok(())
}

pub(crate) fn legacy_context_for_session(
    app: &AppHandle,
    id: &str,
) -> Result<Vec<AgentContextTurn>, String> {
    let store = open_store(app)?;
    let record = store.get_record(id)?;
    let mut context = record
        .as_ref()
        .map(raw_agent_context_from_record)
        .unwrap_or_default();
    filter_pending_legacy_steer_context(&store.connection, id, &mut context)?;
    Ok(context)
}

pub(super) fn filter_pending_legacy_steer_context(
    connection: &Connection,
    id: &str,
    context: &mut Vec<AgentContextTurn>,
) -> Result<(), String> {
    // Legacy source IDs were runtime-only, derived from the durable row, not its JSON.
    let mut statement = connection.prepare("SELECT 'user:' || steer.turn_id || ':steer:' || steer.id FROM agent_steers AS steer
        WHERE steer.session_id = ?1 AND steer.status IN ('pending','delivered')
          AND NOT EXISTS(SELECT 1 FROM agent_runs AS run WHERE run.session_id = steer.session_id
              AND run.turn_id = steer.turn_id AND run.status IN ('completed','cancelled','failed'))")
        .map_err(|error|error.to_string())?;
    let ids = statement
        .query_map([id], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    context.retain(|message| {
        message.role != "user"
            || !message
                .message_id
                .as_ref()
                .is_some_and(|id| ids.contains(id))
    });
    Ok(())
}

pub(crate) fn completed_response(
    app: &AppHandle,
    session: &str,
    turn: &str,
) -> Result<Option<Result<agent_core::AgentChatResponse, String>>, String> {
    let store = open_store(app)?;
    let Some(run) = store.agent_run(session, turn)? else {
        return Ok(None);
    };
    if run.summary.finished_at.is_none() {
        return Ok(None);
    }
    if run.summary.status != "completed" {
        return Ok(Some(Err(if run.summary.status == "cancelled" {
            "请求已取消".into()
        } else {
            run.summary
                .status_message
                .unwrap_or_else(|| "Agent 执行失败".into())
        })));
    }
    let messages = store
        .get_record(session)?
        .and_then(|record| record.messages.as_array().cloned())
        .unwrap_or_default();
    let terminal = completed_terminal(&store.connection, session, turn)?;
    let text = terminal
        .as_ref()
        .and_then(|value| value["text"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            messages
                .iter()
                .filter(|message| message["role"] == "assistant" && message["turnId"] == turn)
                .filter_map(|message| message["content"].as_str())
                .collect()
        });
    let usage = terminal
        .as_ref()
        .and_then(|value| value.get("usage"))
        .filter(|value| value.is_object())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("终态用量记录无效：{error}"))?;
    Ok(Some(Ok(agent_core::AgentChatResponse {
        session_id: session.into(),
        turn_id: turn.into(),
        text,
        usage,
        report: None,
        report_error: None,
        evidence: None,
        context_checkpoint: None,
        termination_reason: if terminal.as_ref().is_some_and(|value| {
            value["termination_reason"] == "output_continued"
                && value["original_termination_reason"] == "max-tokens"
        }) {
            "output_continued"
        } else {
            "completed"
        }
        .into(),
    })))
}

pub(super) fn completed_terminal(
    connection: &Connection,
    session: &str,
    turn: &str,
) -> Result<Option<Value>, String> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload FROM session_events WHERE session_id = ?1
         AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
         AND json_extract(payload, '$.type') = 'completed' ORDER BY rowid DESC LIMIT 1",
            params![session, turn],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    payload
        .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
        .transpose()
}

pub(crate) fn ack_turn_outbox(app: &AppHandle, session: &str, id: &str) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    ack_turn_outbox_record(&open_store(app)?.connection, session, id)
}

pub(super) fn ack_turn_outbox_record(
    connection: &Connection,
    session: &str,
    id: &str,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    transaction
        .execute(
            "UPDATE turn_outbox SET accepted = 1 WHERE session_id = ?1 AND message_id = ?2",
            params![session, id],
        )
        .map_err(|e| e.to_string())?;
    clear_settled_turn_recovery(&transaction, session)?;
    transaction.commit().map_err(|e| e.to_string())
}

pub(crate) fn turn_outbox_accepted(
    app: &AppHandle,
    session: &str,
    id: &str,
) -> Result<bool, String> {
    open_store(app)?
        .connection
        .query_row(
            "SELECT accepted FROM turn_outbox WHERE session_id = ?1 AND message_id = ?2",
            params![session, id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

pub(crate) fn revise_turn_outbox(
    app: &AppHandle,
    session: &str,
    id: &str,
    mode: &str,
    message: &Value,
) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    revise_turn_outbox_record(&open_store(app)?.connection, session, id, mode, message)
}

pub(super) fn revise_turn_outbox_record(
    connection: &Connection,
    session: &str,
    id: &str,
    mode: &str,
    message: &Value,
) -> Result<(), String> {
    if message["id"].as_str() != Some(id) {
        return Err("排队消息修改不能更换 ID".into());
    }
    let affected=connection.execute("UPDATE turn_outbox SET mode = ?1, message = ?2, accepted = 1 WHERE session_id = ?3 AND message_id = ?4",
        params![mode, encode(message)?, session, id]).map_err(|e| e.to_string())?;
    if affected != 1 {
        return Err("原生回执记录不存在，未创建重复消息".into());
    }
    Ok(())
}

pub(crate) fn mirror_guidance(
    app: &AppHandle,
    session: &str,
    turn: &str,
    id: &str,
    request: &AgentChatRequest,
) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .map_err(|_| "历史写入锁不可用")?;
    let store = open_store(app)?;
    store.ensure_conversation_thread(session)?;
    let stored: String = store
        .connection
        .query_row(
            "SELECT messages FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut messages = decode(stored).as_array().cloned().unwrap_or_default();
    if messages.iter().any(|message| message["id"] == id) {
        return Ok(());
    }
    messages.push(serde_json::json!({"id":id,"role":"user","content":request.message,"turnId":turn,"createdAt":now_ms(),
        "attachments":request.attachments,"artifacts":request.artifacts,"source":"steer"}));
    store
        .connection
        .execute(
            "UPDATE sessions SET messages = ?1, updated_at = ?2 WHERE id = ?3",
            params![encode(&serde_json::json!(messages))?, now_ms(), session],
        )
        .map_err(|e| e.to_string())?;
    // §8.9：steer 消息追加是列直写路径，同步分行表。
    store.merge_session_messages(session, &serde_json::json!(messages))?;
    Ok(())
}
