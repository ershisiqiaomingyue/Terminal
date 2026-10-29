use super::*;

pub(super) fn ensure_thread_idle(state: &AgentRuntimeState, thread_id: &str) -> Result<(), String> {
    let running = state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .contains_key(thread_id);
    if running {
        Err("该对话线程中的 Agent 仍在执行，停止后才能变更对话历史".to_string())
    } else {
        Ok(())
    }
}

/// Run a history mutation against the native Rust transcript.
///
/// The old implementation fenced a separate JSONL generation before
/// touching SQLite. Native turns persist directly into this store, so the
/// only boundary that must be enforced is an idle Agent request. The optional
/// legacy cut is intentionally `None`; old migration cuts remain readable for
/// migration but are never created by new native runs.
pub(crate) async fn history_operation<T>(
    app: &AppHandle,
    id: &str,
    reset: bool,
    _capture_boundary: bool,
    operation: impl FnOnce(Option<CheckpointCut>) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<AgentRuntimeState>();
    if state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用")?
        .contains_key(id)
    {
        return Err("该对话仍在执行，停止并完成收尾后才能变更历史。".into());
    }
    let result = operation(None)?;
    if reset {
        if let Some(snapshot) = state.reset_idle_thread(id) {
            let _ = app.emit("agent:thread_snapshot", snapshot);
        }
    }
    Ok(result)
}

#[tauri::command]
pub fn get_conversation_thread(
    app: AppHandle,
    thread_id: String,
) -> Result<Option<ConversationThread>, String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.conversation_thread(&thread_id)
}

#[tauri::command]
pub fn list_conversation_checkpoints(
    app: AppHandle,
    thread_id: String,
) -> Result<Vec<ConversationCheckpointSummary>, String> {
    open_store(&app)?.list_checkpoints(&thread_id)
}

#[tauri::command]
pub async fn create_conversation_checkpoint(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    thread_id: String,
    turn_id: Option<String>,
    label: Option<String>,
) -> Result<ConversationCheckpointSummary, String> {
    ensure_thread_idle(&state, &thread_id)?;
    history_operation(&app, &thread_id, false, true, |cut| {
        let _write_guard = history_write_lock()
            .lock()
            .map_err(|_| "历史写入锁不可用")?;
        open_store(&app)?.create_checkpoint(&thread_id, turn_id, label, cut)
    })
    .await
}

#[tauri::command]
pub async fn revert_conversation_to_checkpoint(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    thread_id: String,
    checkpoint_id: String,
) -> Result<HistorySessionRecord, String> {
    ensure_thread_idle(&state, &thread_id)?;
    history_operation(&app, &thread_id, true, true, |_| {
        let _write_guard = history_write_lock()
            .lock()
            .map_err(|_| "历史写入锁不可用")?;
        let store = open_store(&app)?;
        if store.has_collaboration_jobs(&thread_id)? {
            return Err("该对话有关联协作记录，未回退其父任务历史。".into());
        }
        store.restore_checkpoint(&thread_id, &checkpoint_id)
    })
    .await
}

#[tauri::command]
pub async fn fork_conversation_thread(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    source_thread_id: String,
    checkpoint_id: Option<String>,
    title: Option<String>,
) -> Result<HistorySessionRecord, String> {
    ensure_thread_idle(&state, &source_thread_id)?;
    history_operation(&app, &source_thread_id, false, true, |cut| {
        let _write_guard = history_write_lock()
            .lock()
            .map_err(|_| "历史写入锁不可用")?;
        open_store(&app)?.fork_thread(&source_thread_id, checkpoint_id, title, cut)
    })
    .await
}

#[tauri::command]
pub async fn rewind_conversation_before_message(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    thread_id: String,
    message_id: String,
    turn_id: Option<String>,
) -> Result<HistorySessionRecord, String> {
    ensure_thread_idle(&state, &thread_id)?;
    history_operation(&app, &thread_id, true, false, |_| {
        let _write_guard = history_write_lock()
            .lock()
            .map_err(|_| "历史写入锁不可用")?;
        open_store(&app)?.rewind_before_message(&thread_id, &message_id, turn_id.as_deref())
    })
    .await
}

#[tauri::command(async)]
pub fn save_history_session(app: AppHandle, mut session: HistorySessionInput) -> Result<(), String> {
    let root = app.path().app_cache_dir().map_err(|error| error.to_string())?.join("agent-attachments");
    crate::attachment_cache::externalize_images(&root, &mut session.messages)?;
    // Drafts carry PreparedChatAttachment references once ready. Do not rewrite
    // pending browser drafts: their browser path/data URL is needed to retry.
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.upsert_input(session)
}

#[tauri::command(async)]
pub fn append_history_session_events(
    app: AppHandle,
    id: String,
    updated_at: i64,
    events: Vec<Value>,
) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.append_events(&id, updated_at, &events)
}

pub(super) fn is_retryable_history_write_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("database is locked")
        || normalized.contains("database is busy")
        || normalized.contains("database table is locked")
        || normalized.contains("database schema is locked")
        || normalized.contains("sqlite_busy")
        || normalized.contains("sqlite_locked")
}

// Terminal events determine restart recovery. The normal SQLite busy timeout
// handles the common case; retry a small number of times when another
// connection is still releasing its lock.
pub(crate) async fn append_history_terminal_events_with_retry(
    app: AppHandle,
    id: String,
    updated_at: i64,
    events: Vec<Value>,
) -> Result<(), String> {
    for attempt in 0..TERMINAL_HISTORY_WRITE_ATTEMPTS {
        match append_history_session_events(app.clone(), id.clone(), updated_at, events.clone()) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt + 1 < TERMINAL_HISTORY_WRITE_ATTEMPTS
                    && is_retryable_history_write_error(&error) =>
            {
                tokio::time::sleep(Duration::from_millis(120 * (attempt as u64 + 1))).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("terminal history write attempts must return");
}

#[tauri::command(async)]
pub fn list_history_sessions(
    app: AppHandle,
    archived: Option<bool>,
) -> Result<Vec<HistorySessionRecord>, String> {
    open_store(&app)?.list_records(archived.unwrap_or(false))
}

#[tauri::command(async)]
pub fn list_history_session_summaries(
    app: AppHandle,
    archived: Option<bool>,
) -> Result<Vec<HistorySessionSummary>, String> {
    open_store(&app)?.list_summaries(archived)
}

#[tauri::command(async)]
pub fn get_history_session(
    app: AppHandle,
    id: String,
) -> Result<Option<HistorySessionRecord>, String> {
    open_store(&app)?.get_record(&id)
}

#[tauri::command]
pub async fn get_history_turn_events(
    app: AppHandle,
    session_id: String,
    turn_id: String,
    after_row_id: Option<i64>,
    through_row_id: Option<i64>,
) -> Result<HistoryTurnEventPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_store(&app)?.turn_event_page(
            &session_id,
            &turn_id,
            after_row_id.unwrap_or(0),
            through_row_id,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(async)]
pub fn search_history_sessions(
    app: AppHandle,
    query: String,
) -> Result<Vec<HistorySessionRecord>, String> {
    open_store(&app)?.search_records(&query)
}

#[tauri::command]
pub fn archive_history_session(app: AppHandle, id: String, archived: bool) -> Result<(), String> {
    let _write_guard = history_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_store(&app)?.set_archived(&id, archived)
}

#[tauri::command]
pub async fn delete_history_session(
    app: AppHandle,
    state: State<'_, AgentRuntimeState>,
    id: String,
) -> Result<(), String> {
    let request_active = state
        .requests
        .lock()
        .map_err(|_| "请求状态不可用".to_string())?
        .contains_key(&id);
    if request_active {
        return Err("该会话中的 Agent 仍在执行，停止完成后才能删除".to_string());
    }
    let cleanup_app = app.clone();
    let cleanup_id = id.clone();
    let cleanup_result = tauri::async_runtime::spawn_blocking(move || {
        let _write_guard = history_write_lock()
            .lock()
            .map_err(|_| "历史写入锁不可用")?;
        if cleanup_app
            .state::<AgentRuntimeState>()
            .requests
            .lock()
            .map_err(|_| "请求状态不可用")?
            .contains_key(&cleanup_id)
        {
            return Err("该对话仍在执行，停止完成后才能删除".into());
        }
        let store = open_store(&cleanup_app)?;
        if store.has_collaboration_jobs(&cleanup_id)? {
            return Err(
                "该对话有关联的协作作业。请保留对话以维持作业、结果和恢复记录的关联。".into(),
            );
        }
        let cache_root = cleanup_app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?
            .join("agent-attachments");
        let (mut deleted, _) = if cache_root.exists() {
            store.attachment_references(&cache_root, Some(&cleanup_id), Some(&cleanup_id))?
        } else {
            (HashSet::new(), HashSet::new())
        };
        deleted.extend(store.pending_attachment_cleanup(Some(&cleanup_id))?);
        store.delete_with_attachments(&cleanup_id, &deleted)?;
        crate::file_changes::delete_session_file_backups(&cleanup_app, &cleanup_id)?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| format!("删除对话任务失败: {error}"))?;
    schedule_storage_maintenance(app.clone());
    cleanup_result?;
    if let Some(snapshot) = state.reset_idle_thread(&id) {
        let _ = app.emit("agent:thread_snapshot", snapshot);
    }
    Ok(())
}

#[tauri::command]
pub async fn export_history_sessions(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = app.path().app_cache_dir().map_err(|error| error.to_string())?.join("agent-attachments");
        let mut export: Value = serde_json::from_str(&open_store(&app)?.export_json()?).map_err(|error| error.to_string())?;
        crate::attachment_cache::inline_export_images(&root, &mut export)?;
        serde_json::to_string_pretty(&export).map_err(|error| error.to_string())
    })
        .await
        .map_err(|error| format!("历史导出任务失败: {error}"))?
}

#[tauri::command]
pub fn write_history_export(path: String, content: String) -> Result<(), String> {
    let path = PathBuf::from(path.trim());
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("json"))
    {
        return Err("历史导出文件必须使用 .json 扩展名".to_string());
    }
    std::fs::write(&path, content).map_err(|error| format!("无法写入历史导出文件: {error}"))
}
