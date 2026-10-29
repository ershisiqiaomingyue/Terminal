use super::*;

static LOGCAT_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn logcat_capture_key(owner: &str, serial: &str) -> String {
    format!("{owner}\0{serial}")
}

pub(crate) fn transfer_window_logcat(
    app: &AppHandle,
    source: &str,
    destination: &str,
    state: &serde_json::Value,
) -> Result<(), String> {
    let Some(id) = state
        .pointer("/logcat/sessionId")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(());
    };
    let runtime = app.state::<AndroidToolState>();
    let mut captures = runtime.logcat.lock().map_err(|_| "Logcat 状态不可用")?;
    let key = captures
        .iter()
        .find(|(_, capture)| capture.session_id == id && capture.owner == source)
        .map(|(key, _)| key.clone());
    if let Some(key) = key {
        let mut capture = captures.remove(&key).ok_or("采集不存在")?;
        let serial = key.split_once('\0').map(|(_, serial)| serial).unwrap_or("");
        capture.owner = destination.into();
        captures.insert(logcat_capture_key(destination, serial), capture);
    }
    Ok(())
}

pub(crate) fn stop_window_logcat(app: &AppHandle, label: &str) {
    let runtime = app.state::<AndroidToolState>();
    if let Ok(mut captures) = runtime.logcat.lock() {
        let keys = captures
            .iter()
            .filter(|(_, capture)| capture.owner == label)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(capture) = captures.remove(&key) {
                capture.stop_requested.store(true, Ordering::Relaxed);
                if let Ok(mut child) = capture.child.lock() {
                    let _ = child.kill();
                }
            }
        }
    };
}

#[cfg(test)]
mod window_tests {
    #[test]
    fn captures_for_same_device_are_isolated_by_window() {
        assert_ne!(
            super::logcat_capture_key("main", "device"),
            super::logcat_capture_key("tool-1", "device")
        );
        assert_ne!(
            super::logcat_capture_key("tool-1", "device"),
            super::logcat_capture_key("tool-2", "device")
        );
    }
}

pub(super) fn logcat_history_store(app: &AppHandle) -> Result<Connection, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("logcat");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建 Logcat 数据目录：{error}"))?;
    let connection = Connection::open(root.join("history.sqlite3"))
        .map_err(|error| format!("无法打开 Logcat 历史：{error}"))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("无法配置 Logcat 历史数据库：{error}"))?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, serial TEXT NOT NULL, started_at INTEGER NOT NULL, ended_at INTEGER, file_path TEXT NOT NULL, line_count INTEGER NOT NULL DEFAULT 0, end_reason TEXT); CREATE TABLE IF NOT EXISTS bookmarks (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, created_at INTEGER NOT NULL, line TEXT NOT NULL, note TEXT NOT NULL DEFAULT '')").map_err(|error| format!("无法初始化 Logcat 历史：{error}"))?;
    let has_end_reason = connection
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|error| format!("无法检查 Logcat 历史结构：{error}"))?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("无法检查 Logcat 历史结构：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法检查 Logcat 历史结构：{error}"))?
        .iter()
        .any(|column| column == "end_reason");
    if !has_end_reason {
        connection
            .execute("ALTER TABLE sessions ADD COLUMN end_reason TEXT", [])
            .map_err(|error| format!("无法迁移 Logcat 历史：{error}"))?;
    }
    Ok(connection)
}

pub(super) fn logcat_session_file(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("logcat")
        .join("sessions");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建 Logcat 会话目录：{error}"))?;
    Ok(root.join(format!("{id}.log")))
}

/// Session records point to files created by `logcat_session_file`. Resolve
/// through that invariant before reading or deleting a historical log.
pub(super) fn managed_logcat_session_path(
    app: &AppHandle,
    id: &str,
    stored_path: &str,
) -> Result<PathBuf, String> {
    let expected_name = format!("{id}.log");
    let sessions = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("logcat")
        .join("sessions");
    fs::create_dir_all(&sessions).map_err(|error| format!("无法创建 Logcat 会话目录：{error}"))?;
    let sessions =
        fs::canonicalize(sessions).map_err(|error| format!("无法定位 Logcat 会话目录：{error}"))?;
    let path =
        fs::canonicalize(stored_path).map_err(|error| format!("无法读取 Logcat 会话：{error}"))?;
    if !path.starts_with(&sessions)
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
    {
        return Err("Logcat 会话文件路径无效。".to_string());
    }
    Ok(path)
}

pub(super) const MAX_LOGCAT_HISTORY_SESSIONS: i64 = 100;

/// Keeps a bounded managed history without ever deleting an active capture.
/// Files are derived from the validated session id rather than a database path.
pub(super) fn prune_logcat_history(app: &AppHandle) -> Result<(), String> {
    let connection = logcat_history_store(app)?;
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .map_err(|error| format!("无法统计 Logcat 历史：{error}"))?;
    let excess = (total - MAX_LOGCAT_HISTORY_SESSIONS).max(0);
    if excess == 0 {
        return Ok(());
    }
    let mut statement = connection
        .prepare(
            "SELECT id FROM sessions WHERE ended_at IS NOT NULL ORDER BY ended_at ASC LIMIT ?1",
        )
        .map_err(|error| format!("无法清理 Logcat 历史：{error}"))?;
    let ids = statement
        .query_map(params![excess], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法清理 Logcat 历史：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法清理 Logcat 历史：{error}"))?;
    for id in ids {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("无法清理 Logcat 历史：{error}"))?;
        transaction
            .execute("DELETE FROM bookmarks WHERE session_id=?1", params![id])
            .map_err(|error| format!("无法清理 Logcat 书签：{error}"))?;
        transaction
            .execute("DELETE FROM sessions WHERE id=?1", params![id])
            .map_err(|error| format!("无法清理 Logcat 会话：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("无法清理 Logcat 历史：{error}"))?;
        let _ = fs::remove_file(logcat_session_file(app, &id)?);
    }
    Ok(())
}

pub(super) fn read_bounded_logcat_tail<R: BufRead>(reader: R, limit: usize) -> Vec<String> {
    let mut tail = VecDeque::with_capacity(limit);
    for line in reader.lines().filter_map(Result::ok) {
        if tail.len() == limit {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    tail.into_iter().collect()
}

#[tauri::command]
pub fn read_logcat_window_output(
    state: State<'_, AndroidToolState>,
    session_id: String,
    after_sequence: u64,
) -> Result<Option<Vec<crate::terminal_output::OutputChunk>>, String> {
    let captures = state.logcat.lock().map_err(|_| "Logcat 状态不可用")?;
    let Some(capture) = captures
        .values()
        .find(|capture| capture.session_id == session_id)
    else {
        return Ok(None);
    };
    let output = capture.output.lock().map_err(|_| "Logcat 输出不可用")?;
    Ok(Some(output.since(after_sequence)))
}

#[tauri::command]
pub fn start_logcat(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AndroidToolState>,
    tasks: State<'_, AndroidTaskManager>,
    serial: String,
) -> Result<String, String> {
    let serial = serial.trim().to_string();
    if serial.is_empty() {
        return Err("请选择设备。".to_string());
    }
    let owner = window.label().to_string();
    let capture_key = logcat_capture_key(&owner, &serial);
    if let Some(capture) = state
        .logcat
        .lock()
        .map_err(|_| "Logcat 状态不可用")?
        .get(&capture_key)
    {
        return Ok(capture.session_id.clone());
    }
    let mut digest = Sha256::new();
    digest.update(serial.as_bytes());
    let session_id = format!(
        "{}-{}-{}",
        &format!("{:x}", digest.finalize())[..12],
        now_millis(),
        LOGCAT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let session_path = logcat_session_file(&app, &session_id)?;
    let adb = find_adb()?;
    let device_state = adb_output(&["-s", &serial, "get-state"])
        .map_err(|error| format!("无法检查设备状态：{error}"))?;
    if device_state.trim() != "device" {
        return Err(format!(
            "设备 {serial} 当前不可用于 Logcat，请刷新设备连接后重试。"
        ));
    }
    logcat_history_store(&app)?
        .execute(
            "INSERT INTO sessions (id,serial,started_at,file_path) VALUES (?1,?2,?3,?4)",
            params![
                session_id,
                serial,
                now_millis(),
                session_path.to_string_lossy()
            ],
        )
        .map_err(|error| format!("无法创建 Logcat 会话：{error}"))?;
    let mut child = match background_command(&adb)
        .args(["-s", &serial, "logcat", "-v", "threadtime"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = logcat_history_store(&app).and_then(|connection| {
                connection
                    .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
                    .map_err(|error| error.to_string())
            });
            return Err(format!("无法启动 Logcat：{error}"));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = logcat_history_store(&app).and_then(|connection| {
                connection
                    .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
                    .map_err(|error| error.to_string())
            });
            return Err("无法读取 Logcat 输出。".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = logcat_history_store(&app).and_then(|connection| {
                connection
                    .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
                    .map_err(|error| error.to_string())
            });
            return Err("无法读取 Logcat 错误输出。".to_string());
        }
    };
    let manager = tasks.inner().clone();
    let (task_id, _) = match android_tasks::create_task(
        &app,
        &manager,
        "logcat",
        &serial,
        "Logcat 持续采集中；请显式停止以完成会话。",
    ) {
        Ok(task) => task,
        Err(error) => {
            let _ = child.kill();
            let _ = logcat_history_store(&app).and_then(|connection| {
                connection
                    .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
                    .map_err(|database_error| database_error.to_string())
            });
            return Err(error);
        }
    };
    let _ = android_tasks::record_task_audit(
        &app,
        &task_id,
        "logcat-session",
        &format!("session_id={session_id}\nfile={}", session_path.display()),
    );
    let _ = android_tasks::record_task_audit(
        &app,
        &task_id,
        "command-start",
        &format!("command={} -s {serial} logcat -v threadtime", adb.display()),
    );
    let child = Arc::new(Mutex::new(child));
    let stop_requested = Arc::new(AtomicBool::new(false));
    let mut sessions = state
        .logcat
        .lock()
        .map_err(|_| "Logcat 状态不可用。".to_string())?;
    if sessions.contains_key(&capture_key) {
        let _ = child.lock().map(|mut process| process.kill());
        let _ = logcat_history_store(&app).and_then(|connection| {
            connection
                .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
                .map_err(|error| error.to_string())
        });
        let _ = android_tasks::finish_task(
            &app,
            &manager,
            &task_id,
            None,
            Some("同一设备已由另一个窗口启动 Logcat 采集。"),
        );
        return Err("该设备的 Logcat 已在采集。".to_string());
    }
    let output = Arc::new(Mutex::new(crate::terminal_output::TerminalOutput::default()));
    sessions.insert(
        capture_key,
        LogcatCapture {
            owner,
            output: output.clone(),
            child: child.clone(),
            session_id: session_id.clone(),
            task_id: task_id.clone(),
            stop_requested: stop_requested.clone(),
        },
    );
    drop(sessions);

    // The persisted session is authoritative. The WebView stream is bounded so
    // an overloaded renderer never grows an unbounded in-process queue.
    let (line_sender, line_receiver) = mpsc::sync_channel::<String>(MAX_LOGCAT_UI_QUEUE_LINES);
    let (stderr_sender, stderr_receiver) = mpsc::sync_channel::<String>(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(8 * 1024).read_to_end(&mut bytes);
        let _ = stderr_sender.send(String::from_utf8_lossy(&bytes).trim().to_string());
    });
    let reader_app = app.clone();
    let writer_path = session_path;
    let task_artifact = writer_path.clone();
    let closing_session_id = session_id.clone();
    let closing_task_id = task_id;
    let line_count = Arc::new(AtomicUsize::new(0));
    let writer_line_count = line_count.clone();
    let dropped_event_lines = Arc::new(AtomicUsize::new(0));
    let writer_dropped_event_lines = dropped_event_lines.clone();
    let capture_failure = Arc::new(Mutex::new(None::<String>));
    let writer_failure = capture_failure.clone();
    let writer_child = child.clone();
    std::thread::spawn(move || {
        let stop_capture = |message: String| {
            if let Ok(mut failure) = writer_failure.lock() {
                *failure = Some(message.clone());
            }
            let _ = reader_app.emit("android:logcat-error", message);
            if let Ok(mut child) = writer_child.lock() {
                let _ = child.kill();
            }
        };
        let mut writer = match File::create(writer_path) {
            Ok(file) => file,
            Err(error) => {
                stop_capture(format!("无法保存 Logcat 会话文件：{error}"));
                return;
            }
        };
        let mut reader = BufReader::new(stdout);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            match reader.read_until(b'\n', &mut bytes) {
                Ok(0) => break,
                Ok(_) => {
                    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                        bytes.pop();
                    }
                    // Android services can emit arbitrary bytes. Preserve the stream and replace
                    // only invalid UTF-8 sequences instead of ending capture for the whole device.
                    let line = String::from_utf8_lossy(&bytes).into_owned();
                    if let Err(error) = writeln!(writer, "{line}") {
                        stop_capture(format!("无法写入 Logcat 会话文件：{error}"));
                        break;
                    }
                    writer_line_count.fetch_add(1, Ordering::Relaxed);
                    match line_sender.try_send(line) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(_)) => {
                            writer_dropped_event_lines.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
                Err(error) => {
                    stop_capture(format!("Logcat 读取失败：{error}"));
                    break;
                }
            }
        }
    });
    let captures = state.logcat.clone();
    let watcher_child = child.clone();
    std::thread::spawn(move || {
        let mut batch = Vec::with_capacity(128);
        loop {
            match line_receiver.recv_timeout(Duration::from_millis(16)) {
                Ok(line) => batch.push(line),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        let _ = app.emit(
                            "android:logcat",
                            LogcatEvent {
                                session_id: closing_session_id.clone(),
                                serial: serial.clone(),
                                sequence: output
                                    .lock()
                                    .map(|mut output| output.append(batch.join("\n")).sequence)
                                    .unwrap_or(0),
                                lines: std::mem::take(&mut batch),
                            },
                        );
                    }
                    break;
                }
            }
            while batch.len() < 128 {
                match line_receiver.try_recv() {
                    Ok(line) => batch.push(line),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
            if !batch.is_empty() {
                let _ = app.emit(
                    "android:logcat",
                    LogcatEvent {
                        sequence: output
                            .lock()
                            .map(|mut output| output.append(batch.join("\n")).sequence)
                            .unwrap_or(0),
                        session_id: closing_session_id.clone(),
                        serial: serial.clone(),
                        lines: std::mem::take(&mut batch),
                    },
                );
            }
        }
        let exit_code = watcher_child
            .lock()
            .ok()
            .and_then(|mut child| child.wait().ok())
            .and_then(|status| status.code());
        let stderr = stderr_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default();
        let manually_stopped = stop_requested.load(Ordering::Relaxed);
        let capture_failure = capture_failure
            .lock()
            .ok()
            .and_then(|mut failure| failure.take());
        let mut reason = if manually_stopped {
            "已停止采集。".to_string()
        } else if let Some(failure) = capture_failure {
            failure
        } else if let Some(code) = exit_code {
            format!("Logcat 进程意外结束（退出码 {code}）。设备可能已断开或 ADB 服务不可用。")
        } else {
            "Logcat 进程意外结束。设备可能已断开或 ADB 服务不可用。".to_string()
        };
        if !stderr.is_empty() {
            reason.push_str(&format!(" ADB 错误：{stderr}"));
        }
        let count = line_count.load(Ordering::Relaxed) as i64;
        if let Ok(connection) = logcat_history_store(&app) {
            let _ = connection.execute(
                "UPDATE sessions SET ended_at=?2,line_count=?3,end_reason=?4 WHERE id=?1",
                params![closing_session_id, now_millis(), count, reason.clone()],
            );
        }
        let _ = android_tasks::record_task_audit(
            &app,
            &closing_task_id,
            "process-exit",
            &format!(
                "exit_code={}\nline_count={count}\nstderr={stderr}",
                exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "signal".to_string())
            ),
        );
        let task_error = (!manually_stopped).then_some(reason.as_str());
        let _ = android_tasks::finish_task(
            &app,
            &manager,
            &closing_task_id,
            task_artifact.to_str(),
            task_error,
        );
        if let Ok(mut active) = captures.lock() {
            active.retain(|_, capture| capture.session_id != closing_session_id);
        }
        let _ = prune_logcat_history(&app);
        let _ = app.emit(
            "android:logcat-ended",
            LogcatEndedEvent {
                serial,
                session_id: closing_session_id,
                reason,
                recoverable: !manually_stopped,
                dropped_event_lines: dropped_event_lines.load(Ordering::Relaxed),
            },
        );
    });
    Ok(session_id)
}

#[tauri::command]
pub fn stop_logcat(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AndroidToolState>,
    serial: String,
) -> Result<(), String> {
    let child = state
        .logcat
        .lock()
        .map_err(|_| "Logcat 状态不可用。".to_string())?
        .remove(&logcat_capture_key(window.label(), serial.trim()));
    if let Some(capture) = child {
        capture.stop_requested.store(true, Ordering::Relaxed);
        let _ = android_tasks::record_task_audit(
            &app,
            &capture.task_id,
            "stop-requested",
            "用户请求停止 Logcat 采集，正在终止 ADB 子进程。",
        );
        capture
            .child
            .lock()
            .map_err(|_| "无法停止 Logcat。".to_string())?
            .kill()
            .map_err(|error| format!("无法停止 Logcat：{error}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn list_logcat_history(
    app: AppHandle,
    state: State<'_, AndroidToolState>,
) -> Result<Vec<LogcatHistoryRecord>, String> {
    let connection = logcat_history_store(&app)?;
    let active_ids = state
        .logcat
        .lock()
        .map_err(|_| "Logcat 状态不可用。".to_string())?
        .values()
        .map(|capture| capture.session_id.clone())
        .collect::<Vec<_>>();
    if active_ids.is_empty() {
        connection.execute("UPDATE sessions SET ended_at=?1,end_reason='应用退出或异常中断，原始日志已保留。' WHERE ended_at IS NULL", params![now_millis()]).map_err(|error| format!("无法恢复 Logcat 历史状态：{error}"))?;
    } else {
        let active = active_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("UPDATE sessions SET ended_at=?1,end_reason='应用退出或异常中断，原始日志已保留。' WHERE ended_at IS NULL AND id NOT IN ({active})");
        let mut values = vec![rusqlite::types::Value::Integer(now_millis())];
        values.extend(active_ids.into_iter().map(rusqlite::types::Value::Text));
        connection
            .execute(&sql, rusqlite::params_from_iter(values))
            .map_err(|error| format!("无法恢复 Logcat 历史状态：{error}"))?;
    }
    let mut statement = connection.prepare("SELECT id,serial,started_at,ended_at,line_count,end_reason FROM sessions ORDER BY started_at DESC LIMIT 100").map_err(|error| format!("无法读取 Logcat 历史：{error}"))?;
    let records = statement
        .query_map([], |row| {
            Ok(LogcatHistoryRecord {
                id: row.get(0)?,
                serial: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                line_count: row.get(4)?,
                end_reason: row.get(5)?,
            })
        })
        .map_err(|error| format!("无法读取 Logcat 历史：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Logcat 历史：{error}"))?;
    Ok(records)
}

#[tauri::command]
pub fn read_logcat_history(
    app: AppHandle,
    request: LogcatHistoryReadRequest,
) -> Result<Vec<String>, String> {
    let connection = logcat_history_store(&app)?;
    let path: String = connection
        .query_row(
            "SELECT file_path FROM sessions WHERE id=?1",
            params![request.id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法找到 Logcat 会话：{error}"))?;
    let limit = request.limit.clamp(1, 20_000);
    let path = managed_logcat_session_path(&app, &request.id, &path)?;
    let reader =
        BufReader::new(File::open(path).map_err(|error| format!("无法读取 Logcat 会话：{error}"))?);
    Ok(read_bounded_logcat_tail(reader, limit))
}

#[tauri::command]
pub fn delete_logcat_history(
    app: AppHandle,
    state: State<'_, AndroidToolState>,
    request: LogcatHistoryDeleteRequest,
) -> Result<(), String> {
    let id = request.id.trim();
    if id.is_empty() {
        return Err("请选择要删除的 Logcat 会话。".to_string());
    }
    let active = state
        .logcat
        .lock()
        .map_err(|_| "Logcat 状态不可用。".to_string())?
        .values()
        .any(|capture| capture.session_id == id);
    if active {
        return Err("正在采集的 Logcat 会话不能删除，请先停止采集。".to_string());
    }

    let mut connection = logcat_history_store(&app)?;
    let file_path = connection
        .query_row(
            "SELECT file_path FROM sessions WHERE id=?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 Logcat 会话：{error}"))?
        .ok_or_else(|| "Logcat 会话不存在或已经被删除。".to_string())?;
    let file = managed_logcat_session_path(&app, id, &file_path)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法删除 Logcat 会话：{error}"))?;
    transaction
        .execute("DELETE FROM bookmarks WHERE session_id=?1", params![id])
        .map_err(|error| format!("无法删除 Logcat 书签：{error}"))?;
    transaction
        .execute("DELETE FROM sessions WHERE id=?1", params![id])
        .map_err(|error| format!("无法删除 Logcat 会话：{error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("无法删除 Logcat 会话：{error}"))?;
    fs::remove_file(&file).map_err(|error| {
        format!(
            "已移除历史记录，但无法删除日志文件 {}：{error}",
            file.display()
        )
    })?;
    Ok(())
}

#[tauri::command]
pub fn export_logcat_content(request: LogcatExportRequest) -> Result<String, String> {
    if request.content.len() > 64 * 1024 * 1024 {
        return Err("导出内容超过 64 MB 限制。".to_string());
    }
    let destination = PathBuf::from(request.destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "导出路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建导出目录：{error}"))?;
    fs::write(&destination, request.content)
        .map_err(|error| format!("无法导出 Logcat：{error}"))?;
    Ok(destination.to_string_lossy().to_string())
}

/// Imports a text Logcat file into the managed history directory so it keeps
/// working after the original file moves or is deleted.
#[tauri::command]
pub fn import_logcat_history(
    app: AppHandle,
    request: LogcatImportRequest,
) -> Result<LogcatHistoryRecord, String> {
    let source = fs::canonicalize(request.path.trim())
        .map_err(|error| format!("无法读取导入日志：{error}"))?;
    if !source.is_file() {
        return Err("请选择一个 Logcat 文本文件。".to_string());
    }
    let metadata = fs::metadata(&source).map_err(|error| format!("无法读取导入日志：{error}"))?;
    if metadata.len() > 512 * 1024 * 1024 {
        return Err("导入日志超过 512 MB 限制。".to_string());
    }
    let mut digest = Sha256::new();
    digest.update(source.to_string_lossy().as_bytes());
    digest.update(now_millis().to_le_bytes());
    let id = format!("import-{}", &format!("{:x}", digest.finalize())[..16]);
    let destination = logcat_session_file(&app, &id)?;
    fs::copy(&source, &destination).map_err(|error| format!("无法保存导入日志：{error}"))?;
    let reader = BufReader::new(
        File::open(&destination).map_err(|error| format!("无法统计导入日志：{error}"))?,
    );
    let line_count = reader.lines().count() as i64;
    let serial = if request.serial.trim().is_empty() {
        format!(
            "导入：{}",
            source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("logcat")
        )
    } else {
        request.serial.trim().to_string()
    };
    let started_at = now_millis();
    logcat_history_store(&app)?.execute("INSERT INTO sessions (id,serial,started_at,ended_at,file_path,line_count,end_reason) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![id, serial, started_at, started_at, destination.to_string_lossy(), line_count, "导入的文本记录。"]).map_err(|error| format!("无法保存导入 Logcat 历史：{error}"))?;
    prune_logcat_history(&app)?;
    Ok(LogcatHistoryRecord {
        id,
        serial,
        started_at,
        ended_at: Some(started_at),
        line_count,
        end_reason: Some("导入的文本记录。".to_string()),
    })
}

#[tauri::command]
pub fn add_logcat_bookmark(
    app: AppHandle,
    request: LogcatBookmarkRequest,
) -> Result<LogcatBookmark, String> {
    let created_at = now_millis();
    let connection = logcat_history_store(&app)?;
    connection
        .execute(
            "INSERT INTO bookmarks (session_id,created_at,line,note) VALUES (?1,?2,?3,?4)",
            params![request.session_id, created_at, request.line, request.note],
        )
        .map_err(|error| format!("无法保存 Logcat 书签：{error}"))?;
    Ok(LogcatBookmark {
        id: connection.last_insert_rowid(),
        session_id: request.session_id,
        created_at,
        line: request.line,
        note: request.note,
    })
}

#[tauri::command]
pub fn list_logcat_bookmarks(
    app: AppHandle,
    session_id: String,
) -> Result<Vec<LogcatBookmark>, String> {
    let connection = logcat_history_store(&app)?;
    let mut statement = connection.prepare("SELECT id,session_id,created_at,line,note FROM bookmarks WHERE session_id=?1 ORDER BY created_at DESC").map_err(|error| format!("无法读取 Logcat 书签：{error}"))?;
    let bookmarks = statement
        .query_map(params![session_id], |row| {
            Ok(LogcatBookmark {
                id: row.get(0)?,
                session_id: row.get(1)?,
                created_at: row.get(2)?,
                line: row.get(3)?,
                note: row.get(4)?,
            })
        })
        .map_err(|error| format!("无法读取 Logcat 书签：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Logcat 书签：{error}"))?;
    Ok(bookmarks)
}

#[tauri::command]
pub fn query_logcat_evidence(
    app: AppHandle,
    request: LogcatEvidenceRequest,
) -> Result<Vec<String>, String> {
    let terms = request
        .terms
        .into_iter()
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let lines = read_logcat_history(
        app,
        LogcatHistoryReadRequest {
            id: request.session_id,
            limit: 20_000,
        },
    )?;
    Ok(lines
        .into_iter()
        .filter(|line| terms.iter().all(|term| line.to_lowercase().contains(term)))
        .take(request.limit.clamp(1, 500))
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AndroidPackageKind {
    Apk,
    Aar,
    Jar,
    Aab,
    Apks,
    Zip,
    So,
}
