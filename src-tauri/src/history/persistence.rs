use super::*;

pub(super) fn row_to_session_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<HistorySessionRecord> {
    let jira_context = decode_optional(row.get::<_, Option<String>>(4)?);
    let artifacts = decode(row.get::<_, String>(5)?);
    let messages = decode(row.get::<_, String>(6)?);
    let metadata = decode(row.get::<_, String>(7)?);
    let report = decode_optional(row.get::<_, Option<String>>(8)?);
    let usage = decode_optional(row.get::<_, Option<String>>(9)?);
    let events = decode_optional(row.get::<_, Option<String>>(11)?);
    Ok(HistorySessionRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        updated_at: row.get(2)?,
        archived: row.get(3)?,
        schema_version: normalize_schema_version(row.get(10)?),
        jira_context,
        artifacts,
        messages,
        metadata,
        report,
        usage,
        events,
        agent_runs: Vec::new(),
        usage_totals: None,
    })
}

pub(super) fn row_to_agent_queue_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentQueueItem> {
    let request_json = row.get::<_, String>(3)?;
    let mut request = serde_json::from_str::<AgentChatRequest>(&request_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let execution_target = row
        .get::<_, Option<String>>(4)?
        .map(|value| {
            serde_json::from_str::<AgentExecutionTarget>(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    // The dedicated column is immutable queue metadata. It deliberately wins
    // over any target nested in a malformed or manually edited request blob.
    request.execution_target = execution_target;
    Ok(AgentQueueItem {
        id: row.get(0)?,
        session_id: row.get(1)?,
        idempotency_key: row.get(2)?,
        request,
        status: row.get(5)?,
        sequence: row.get(6)?,
        attempt_count: row.get(7)?,
        last_error: row.get(8)?,
        enqueued_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

pub(super) fn row_to_session_summary(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<HistorySessionSummary> {
    Ok(HistorySessionSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        updated_at: row.get(2)?,
        archived: row.get(3)?,
        jira_context: decode_optional(row.get::<_, Option<String>>(4)?),
        metadata: decode(row.get::<_, String>(5)?),
        message_count: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
    })
}

pub(super) fn collect_session_events(session: &HistorySessionRecord) -> Vec<Value> {
    let mut events = Vec::new();
    if let Some(Value::Array(values)) = session.events.as_ref() {
        events.extend(values.iter().cloned().filter(Value::is_object));
    }
    if let Value::Array(messages) = &session.messages {
        for message in messages {
            if let Some(Value::Array(values)) = message.get("events") {
                events.extend(values.iter().cloned().filter(Value::is_object));
            }
        }
    }
    let mut seen = HashSet::new();
    events
        .into_iter()
        .enumerate()
        .filter(|(index, event)| seen.insert(history_event_key(event, *index)))
        .map(|(_, event)| event)
        .collect()
}

pub(super) fn history_event_key(event: &Value, index: usize) -> String {
    if let Some(event_id) = event.get("event_id").and_then(Value::as_str) {
        return format!("id:{event_id}");
    }
    if let Some(sequence) = event.get("sequence").and_then(Value::as_i64) {
        let turn_id = event
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return format!("sequence:{turn_id}:{sequence}");
    }
    format!(
        "legacy:{}:{}",
        index,
        serde_json::to_string(event).unwrap_or_default()
    )
}

pub(super) fn encode(value: &Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("无法序列化历史数据: {error}"))
}

pub(super) fn encode_optional(value: &Option<Value>) -> Result<Option<String>, String> {
    value.as_ref().map(encode).transpose()
}

pub(super) fn json_array_len(value: &Value) -> i64 {
    value
        .as_array()
        .map(|items| items.len() as i64)
        .unwrap_or(0)
}

pub(super) fn decode(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or(Value::Null)
}

pub(super) fn decode_optional(value: Option<String>) -> Option<Value> {
    value.map(decode)
}

pub(super) fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("agent-history.sqlite3"))
        .map_err(|error| format!("无法定位历史数据库目录: {error}"))
}

/// 命令层的连接复用（codex StateRuntime 同款思想）：此前每条命令都重建
/// 连接并重跑全套 PRAGMA/DDL 初始化（80+ 处调用、单次数 ms 级、每 turn
/// 几十次），在数百 MB 的库上全部串在进程级写锁里重复消耗。改为进程内
/// 单例连接 + Mutex 守卫——写并发本来就是 1（history_write_lock 串行），
/// 串行化读是可接受的代价，后续需要读并发时再加只读连接。
///
/// 守卫规则：调用方持有期间不得再调 `open_store`（嵌套=自锁），不得跨
/// await 持有（Guard 非 Send，编译器兜底）。测试用 `HistoryStore::open`
/// 直连不受影响。
pub(super) fn open_store(
    app: &AppHandle,
) -> Result<std::sync::MutexGuard<'static, HistoryStore>, String> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<HistoryStore>> = std::sync::OnceLock::new();
    if let Some(store) = STORE.get() {
        // Poisoned 也继续用（项目惯例）：持锁线程崩掉后连接仍可用，
        // SQLite 自身保证未提交事务已回滚。
        return Ok(store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
    }
    let store = {
        let path = database_path(app)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建历史数据库目录: {error}"))?;
        }
        std::sync::Mutex::new(HistoryStore::open(path)?)
    };
    // 初始化竞争：后到者丢弃自己的实例，用先到者的（同一文件，无害）。
    let store = STORE.get_or_init(|| store);
    Ok(store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

pub(crate) fn schedule_storage_maintenance(app: AppHandle) {
    static SCHEDULED: AtomicBool = AtomicBool::new(false);
    if SCHEDULED.swap(true, Ordering::Relaxed) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let maintenance_app = app.clone();
            let result = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
                let Ok(_write_guard) = history_write_lock().try_lock() else {
                    return Ok(false);
                };
                let state = maintenance_app.state::<AgentRuntimeState>();
                let Ok(requests) = state.requests.try_lock() else {
                    return Ok(false);
                };
                if !requests.is_empty() {
                    return Ok(false);
                }
                let store = open_store(&maintenance_app)?;
                let cache_root = maintenance_app
                    .path()
                    .app_cache_dir()
                    .map_err(|error| error.to_string())?
                    .join("agent-attachments");
                if cache_root.exists() {
                    let (_, live) = store.attachment_references(&cache_root, None, None)?;
                    let pending = store.pending_attachment_cleanup(None)?;
                    crate::attachment_cache::prune_attachments(
                        &cache_root,
                        &pending,
                        &live,
                        SystemTime::now(),
                    )?;
                }
                for session_id in store.pending_session_cleanups()? {
                    crate::file_changes::delete_session_file_backups(
                        &maintenance_app,
                        &session_id,
                    )?;
                }
                store.finish_attachment_cleanup(None)?;
                store.reclaim_deleted_space()?;
                Ok(true)
            })
            .await;
            match result {
                Ok(Ok(false)) => continue,
                Ok(Ok(true)) => {}
                Ok(Err(error)) => eprintln!("存储维护未完成: {error}"),
                Err(error) => eprintln!("存储维护任务失败: {error}"),
            }
            SCHEDULED.store(false, Ordering::Relaxed);
            break;
        }
    });
}

/// Native durable outbox only. The renderer is the sole inbox/scheduling owner.
pub(crate) fn existing_session_binding(
    app: &AppHandle,
    id: &str,
) -> Result<Option<SessionBinding>, String> {
    open_store(app)?.session_binding(id)
}
