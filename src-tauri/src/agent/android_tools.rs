use super::*;

pub(super) fn summarize_trace_node(node: &TraceNode, depth: usize, remaining: &mut usize) -> Value {
    if *remaining == 0 {
        return json!({ "truncated": true });
    }
    *remaining -= 1;
    let children = if depth < 4 {
        node.children
            .iter()
            .map(|child| summarize_trace_node(child, depth + 1, remaining))
            .collect::<Vec<_>>()
    } else if node.children.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "truncated": true, "childCount": node.children.len() })]
    };
    json!({
        "id": node.id,
        "label": node.label,
        "kind": node.kind,
        "visible": node.visible,
        "children": children,
    })
}

pub(super) fn summarize_trace_document(
    document: &TraceDocument,
    view: &str,
    snapshot_index: usize,
    max_items: usize,
) -> Result<Value, String> {
    let available = [
        document.surface_flinger.as_ref().map(|_| "surface_flinger"),
        document.window_manager.as_ref().map(|_| "window_manager"),
        (!document.transactions.is_empty()).then_some("transactions"),
        (!document.transitions.is_empty()).then_some("transitions"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let mut result = json!({
        "sourceName": document.source_name,
        "format": "winscope",
        "available": available,
        "timelinePoints": document.timeline.len(),
        "surfaceFlingerSnapshots": document.surface_flinger.as_ref().map(|value| value.snapshots.len()).unwrap_or_default(),
        "windowManagerSnapshots": document.window_manager.as_ref().map(|value| value.snapshots.len()).unwrap_or_default(),
        "transactionCount": document.transactions.len(),
        "transitionCount": document.transitions.len(),
    });
    match view {
        "summary" => {}
        "overview" => {
            let mut views = serde_json::Map::new();
            for child_view in ["surface_flinger", "window_manager"] {
                let has_view = if child_view == "surface_flinger" {
                    document.surface_flinger.is_some()
                } else {
                    document.window_manager.is_some()
                };
                if !has_view {
                    continue;
                }
                let child = summarize_trace_document(
                    document,
                    child_view,
                    snapshot_index,
                    max_items.min(2),
                )?;
                views.insert(child_view.to_string(), child);
            }
            result["view"] = json!("overview");
            result["views"] = Value::Object(views);
        }
        "surface_flinger" | "window_manager" => {
            let hierarchy = if view == "surface_flinger" {
                document.surface_flinger.as_ref()
            } else {
                document.window_manager.as_ref()
            }
            .ok_or_else(|| format!("Trace 中没有 {view} 数据"))?;
            let snapshots = hierarchy
                .snapshots
                .iter()
                .skip(snapshot_index)
                .take(max_items)
                .map(|snapshot| {
                    let mut remaining = 160usize;
                    json!({
                        "timestamp": snapshot.timestamp,
                        "timestampNs": snapshot.timestamp_ns.to_string(),
                        "rectCount": snapshot.rects.len(),
                        "propertyGroupCount": snapshot.properties_by_id.values().map(Vec::len).sum::<usize>(),
                        "tree": summarize_trace_node(&snapshot.tree, 0, &mut remaining),
                    })
                })
                .collect::<Vec<_>>();
            result["view"] = json!(view);
            result["snapshotIndex"] = json!(snapshot_index);
            result["snapshots"] = json!(snapshots);
            result["truncated"] = json!(snapshot_index.saturating_add(snapshots.len()) < hierarchy.snapshots.len());
        }
        "transactions" => {
            let entries = document
                .transactions
                .iter()
                .skip(snapshot_index)
                .take(max_items)
                .map(|entry| json!({
                    "id": entry.id,
                    "time": entry.time,
                    "type": entry.entry_type,
                    "layerOrDisplayId": entry.layer_or_display_id,
                    "what": entry.what,
                    "pid": entry.pid,
                    "uid": entry.uid,
                }))
                .collect::<Vec<_>>();
            result["view"] = json!(view);
            result["items"] = json!(entries);
            result["truncated"] = json!(snapshot_index.saturating_add(entries.len()) < document.transactions.len());
        }
        "transitions" => {
            let entries = document
                .transitions
                .iter()
                .skip(snapshot_index)
                .take(max_items)
                .map(|entry| json!({
                    "id": entry.id,
                    "type": entry.entry_type,
                    "sendTime": entry.send_time,
                    "dispatchTime": entry.dispatch_time,
                    "duration": entry.duration,
                    "status": entry.status,
                }))
                .collect::<Vec<_>>();
            result["view"] = json!(view);
            result["items"] = json!(entries);
            result["truncated"] = json!(snapshot_index.saturating_add(entries.len()) < document.transitions.len());
        }
        _ => return Err("view 只支持 overview、summary、surface_flinger、window_manager、transactions 或 transitions".to_string()),
    }
    Ok(result)
}

pub(super) struct InspectWinScopeTraceTool {
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}

pub(super) struct QueryAndroidLogcatTool {
    pub(super) app: Option<AppHandle>,
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct CaptureAndroidLogcatTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidTaskTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidArchiveHistoryTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct StartAndroidArchiveTaskTool {
    pub(super) app: AppHandle,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryPerfettoTraceTool {
    pub(super) app: Option<AppHandle>,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidDiagnosticsTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidBugreportTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAospBuildTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidSimpleperfTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidNativeSymbolizerTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidMethodProfileTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidHeapDumpTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidBenchmarkTool {
    pub(super) cancellation: Arc<AtomicBool>,
}
pub(super) struct QueryAndroidDeviceTool {
    pub(super) cancellation: Arc<AtomicBool>,
}

pub(super) fn read_bounded_logcat_evidence<R: BufRead>(
    mut reader: R,
    terms: &[String],
    limit: usize,
    cancellation: &AtomicBool,
) -> Result<(Vec<String>, bool), String> {
    const SCAN_BYTES: usize = 32 * 1024 * 1024;
    const LINE_BYTES: usize = 64 * 1024;
    const EVIDENCE_BYTES: usize = 512 * 1024;
    let started = std::time::Instant::now();
    let mut scanned = 0;
    let mut evidence_bytes = 0;
    let mut evidence = Vec::with_capacity(limit);
    let mut line_bytes = Vec::new();
    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        if scanned >= SCAN_BYTES || started.elapsed() >= std::time::Duration::from_secs(3) {
            return Ok((evidence, true));
        }
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("无法读取 Logcat 会话文件：{error}"))?;
        let eof = buffer.is_empty();
        if eof && line_bytes.is_empty() {
            break;
        }
        if !eof {
            let next_line = buffer.iter().position(|byte| *byte == b'\n');
            let length = next_line
                .map_or(buffer.len(), |index| index + 1)
                .min(SCAN_BYTES - scanned);
            if line_bytes.len() + length > LINE_BYTES {
                return Ok((evidence, true));
            }
            line_bytes.extend_from_slice(&buffer[..length]);
            reader.consume(length);
            scanned += length;
            if line_bytes.last() != Some(&b'\n') {
                continue;
            }
        }
        let line = std::str::from_utf8(&line_bytes)
            .map_err(|error| format!("Logcat 文本编码无效：{error}"))?;
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let lower = (!terms.is_empty()).then(|| line.to_ascii_lowercase());
        if terms
            .iter()
            .all(|term| lower.as_ref().is_some_and(|lower| lower.contains(term)))
        {
            if evidence.len() == limit {
                return Ok((evidence, true));
            }
            if evidence_bytes + line.len() > EVIDENCE_BYTES {
                return Ok((evidence, true));
            }
            evidence_bytes += line.len();
            evidence.push(line.to_string());
        }
        line_bytes.clear();
        if eof {
            break;
        }
    }
    Ok((evidence, false))
}

#[async_trait]
impl ToolHandler for CaptureAndroidLogcatTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("capture_android_logcat", "Read and filter the current Android device Logcat buffer in the background, then return found, matchedCount and bounded evidence directly to this conversation. Never opens any page, planner or new chat, clears logs, or starts a persistent capture. Use for current/live log checks. Batch alternative keywords into one terms array with matchMode=any (default); all requires all terms on the same line. No match is a valid result: report the inspected scope instead of repeatedly querying saved sessions. Only capture again after a new reproduction or an explicit scope change. If serial is omitted, exactly one online device must exist.", json!({
            "type":"object", "additionalProperties":false, "properties": {
                "serial":{"type":"string"}, "terms":{"type":"array", "items":{"type":"string", "maxLength":256}, "maxItems":32},
                "matchMode":{"type":"string", "enum":["any","all"]},
                "limit":{"type":"integer", "minimum":1, "maximum":500},
                "maxLines":{"type":"integer", "minimum":1, "maximum":10000, "description":"Number of most recent device-buffer lines to inspect; default 2000."},
                "timeoutMs":{"type":"integer", "minimum":1000, "maximum":15000}
            }
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        crate::agent_logcat::capture_logcat(input, self.cancellation.clone()).await
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidLogcatTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_logcat", "Read saved historical Android Logcat evidence only when the user explicitly requests a saved session. This does NOT read fresh device logs. For current log checks use capture_android_logcat instead; do not replay this historical query with one keyword per call. Omit sessionId to list saved sessions, then pass a returned sessionId to read bounded raw evidence lines matching all requested keywords. Returns results in this conversation without opening any page, planner or new chat.", json!({
            "type":"object", "properties": { "sessionId": {"type":"string", "description":"Logcat 会话 ID；省略时列出最近会话"}, "terms": {"type":"array", "items":{"type":"string"}, "description":"全部匹配的关键词"}, "limit": {"type":"integer", "minimum":1, "maximum":500} }
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        let cancellation = self.cancellation.clone();
        let data_root = match &self.app {
            Some(app) => app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法获取应用数据目录：{error}"))?,
            None => android_agent_data_root()?,
        };
        tokio::task::spawn_blocking(move || query_saved_logcat(input, &data_root, &cancellation))
            .await
            .map_err(|error| format!("读取 Logcat 工作线程失败：{error}"))?
    }
}

pub(super) fn query_saved_logcat(
    input: Value,
    data_root: &Path,
    cancellation: &AtomicBool,
) -> Result<Value, String> {
    if cancellation.load(Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    let root = data_root.join("logcat");
    let connection = Connection::open_with_flags(
        root.join("history.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("无法读取 Logcat 历史：{error}"))?;
    let session_id = input
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if session_id.is_empty() {
        let mut statement = connection.prepare("SELECT id,serial,started_at,ended_at,line_count FROM sessions ORDER BY started_at DESC LIMIT 20").map_err(|error| format!("无法读取 Logcat 会话：{error}"))?;
        let sessions = statement.query_map([], |row| Ok(json!({"id":row.get::<_,String>(0)?,"serial":row.get::<_,String>(1)?,"startedAt":row.get::<_,i64>(2)?,"endedAt":row.get::<_,Option<i64>>(3)?,"lineCount":row.get::<_,i64>(4)?}))).map_err(|error| format!("无法读取 Logcat 会话：{error}"))?.collect::<Result<Vec<_>,_>>().map_err(|error| format!("无法读取 Logcat 会话：{error}"))?;
        return Ok(json!({"sessions":sessions}));
    }
    let path: String = connection
        .query_row(
            "SELECT file_path FROM sessions WHERE id=?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法找到 Logcat 会话：{error}"))?;
    let sessions_root = fs::canonicalize(root.join("sessions"))
        .map_err(|error| format!("无法定位 Logcat 会话目录：{error}"))?;
    let path =
        fs::canonicalize(path).map_err(|error| format!("无法读取 Logcat 会话文件：{error}"))?;
    let expected_name = format!("{session_id}.log");
    if !path.starts_with(&sessions_root)
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
    {
        return Err("Logcat 会话文件路径无效。".to_string());
    }
    let terms = input
        .get("terms")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|term| term.to_ascii_lowercase())
                .filter(|term| !term.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    let file =
        fs::File::open(path).map_err(|error| format!("无法读取 Logcat 会话文件：{error}"))?;
    let (evidence, truncated) =
        read_bounded_logcat_evidence(BufReader::new(file), &terms, limit, cancellation)?;
    Ok(
        json!({"sessionId":session_id,"terms":terms,"evidence":evidence,"truncated":truncated,"partial":truncated,"nextStep":if truncated {Some("结果达到匹配数量或扫描预算，不代表全部日志；请缩小日志范围后继续核对。")} else {None}}),
    )
}

#[async_trait]
impl ToolHandler for QueryAndroidTaskTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_tasks", "Read bounded Android task and instant-operation audit records, including task target, current state, errors, and local artifact path. Use this to inspect completed capture/build jobs or recent ADB mutations before analysing their artifacts.", json!({
            "type":"object", "properties": { "taskId": {"type":"string", "description":"任务 ID；省略时列出最近任务"}, "limit": {"type":"integer", "minimum":1, "maximum":100}, "includeOperations": {"type":"boolean", "description":"是否附带最近即时 ADB 操作审计"} }
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_root = android_agent_data_root()?.join("android-tasks");
        let connection = Connection::open(task_root.join("tasks.sqlite3"))
            .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .clamp(1, 100) as i64;
        let sql = if task_id.is_empty() {
            "SELECT id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error FROM tasks ORDER BY started_at DESC LIMIT ?1"
        } else {
            "SELECT id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error FROM tasks WHERE id=?1 LIMIT 1"
        };
        let parameter = if task_id.is_empty() {
            rusqlite::types::Value::Integer(limit)
        } else {
            rusqlite::types::Value::Text(task_id.to_string())
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(|error| format!("无法查询 Android 任务：{error}"))?;
        let records = statement.query_map(rusqlite::params![parameter], |row| Ok(json!({
            "id":row.get::<_,String>(0)?, "kind":row.get::<_,String>(1)?, "target":row.get::<_,String>(2)?, "status":row.get::<_,String>(3)?, "progress":row.get::<_,u8>(4)?, "message":row.get::<_,String>(5)?, "startedAt":row.get::<_,i64>(6)?, "finishedAt":row.get::<_,Option<i64>>(7)?, "artifactPath":row.get::<_,Option<String>>(8)?, "error":row.get::<_,Option<String>>(9)?
        }))).map_err(|error| format!("无法查询 Android 任务：{error}"))?.collect::<Result<Vec<_>,_>>().map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let audit = if task_id.is_empty() {
            Vec::new()
        } else {
            let mut audit_statement = connection.prepare("SELECT id,created_at,event,detail FROM task_audit WHERE task_id=?1 ORDER BY id ASC LIMIT 500").map_err(|error| format!("无法读取 Android 任务审计：{error}"))?;
            let records = audit_statement.query_map([task_id], |row| Ok(json!({"id":row.get::<_,i64>(0)?,"createdAt":row.get::<_,i64>(1)?,"event":row.get::<_,String>(2)?,"detail":row.get::<_,String>(3)?}))).map_err(|error| format!("无法读取 Android 任务审计：{error}"))?.collect::<Result<Vec<_>,_>>().map_err(|error| format!("无法读取 Android 任务审计：{error}"))?;
            records
        };
        let artifacts = if task_id.is_empty() {
            Vec::new()
        } else if let Some(path) = records
            .first()
            .and_then(|record| record.get("artifactPath"))
            .and_then(Value::as_str)
        {
            let artifact_root =
                fs::canonicalize(path).map_err(|error| format!("无法读取任务产物：{error}"))?;
            if artifact_root.is_file() {
                vec![
                    json!({"path": artifact_root.file_name().and_then(|name| name.to_str()).unwrap_or("artifact"), "size": fs::metadata(&artifact_root).map(|meta| meta.len()).unwrap_or(0), "directory": false}),
                ]
            } else {
                let mut entries = Vec::new();
                let mut queue = vec![artifact_root.clone()];
                while let Some(directory) = queue.pop() {
                    let children = fs::read_dir(&directory)
                        .map_err(|error| format!("无法读取任务产物：{error}"))?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| format!("无法读取任务产物：{error}"))?;
                    for child in children {
                        if entries.len() >= 100 {
                            break;
                        }
                        let path = child.path();
                        let metadata = fs::symlink_metadata(&path)
                            .map_err(|error| format!("无法读取任务产物：{error}"))?;
                        if metadata.file_type().is_symlink() {
                            continue;
                        }
                        let relative = path
                            .strip_prefix(&artifact_root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/");
                        entries.push(json!({"path": relative, "size": if metadata.is_file() { metadata.len() } else { 0 }, "directory": metadata.is_dir()}));
                        if metadata.is_dir() {
                            queue.push(path);
                        }
                    }
                    if entries.len() >= 100 {
                        break;
                    }
                }
                entries
            }
        } else {
            Vec::new()
        };
        let operations = if input
            .get("includeOperations")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let mut statement = connection.prepare("SELECT id,created_at,action,target,status,detail FROM operation_audit ORDER BY id DESC LIMIT 100").map_err(|error| format!("无法读取 Android 操作审计：{error}"))?;
            let records = statement.query_map([], |row| Ok(json!({"id":row.get::<_,i64>(0)?,"createdAt":row.get::<_,i64>(1)?,"action":row.get::<_,String>(2)?,"target":row.get::<_,String>(3)?,"status":row.get::<_,String>(4)?,"detail":row.get::<_,String>(5)?}))).map_err(|error| format!("无法读取 Android 操作审计：{error}"))?.collect::<Result<Vec<_>,_>>().map_err(|error| format!("无法读取 Android 操作审计：{error}"))?;
            records
        } else {
            Vec::new()
        };
        Ok(
            json!({"tasks":records,"audit":audit,"artifacts":artifacts,"operations":operations,"truncated":task_id.is_empty() && records.len()==limit as usize,"artifactsTruncated":!task_id.is_empty() && artifacts.len() >= 100}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidArchiveHistoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_archive_history", "Read bounded APK archive/workspace history. Returns original package metadata, edit workspace state, signed build artifact, verification and installation result without opening arbitrary files.", json!({
            "type":"object", "properties": { "historyId": {"type":"string", "description":"APK 历史 ID；省略时列出最近记录"}, "limit": {"type":"integer", "minimum":1, "maximum":100} }
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let connection =
            Connection::open(android_agent_data_root()?.join("archive-history.sqlite3"))
                .map_err(|error| format!("无法读取 APK 历史：{error}"))?;
        let history_id = input
            .get("historyId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .clamp(1, 100) as i64;
        let sql = if history_id.is_empty() {
            "SELECT id,archive_path,file_name,sha256,opened_at,updated_at,workspace_path,apktool_version,manifest_changed,output_path,verification,installed_serial,install_result FROM archive_history ORDER BY updated_at DESC LIMIT ?1"
        } else {
            "SELECT id,archive_path,file_name,sha256,opened_at,updated_at,workspace_path,apktool_version,manifest_changed,output_path,verification,installed_serial,install_result FROM archive_history WHERE id=?1 LIMIT 1"
        };
        let parameter = if history_id.is_empty() {
            rusqlite::types::Value::Integer(limit)
        } else {
            rusqlite::types::Value::Text(history_id.to_string())
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(|error| format!("无法查询 APK 历史：{error}"))?;
        let records = statement.query_map(rusqlite::params![parameter], |row| Ok(json!({
            "id":row.get::<_,String>(0)?, "archivePath":row.get::<_,String>(1)?, "fileName":row.get::<_,String>(2)?, "sha256":row.get::<_,String>(3)?, "openedAt":row.get::<_,i64>(4)?, "updatedAt":row.get::<_,i64>(5)?, "workspacePath":row.get::<_,Option<String>>(6)?, "apktoolVersion":row.get::<_,Option<String>>(7)?, "manifestChanged":row.get::<_,i32>(8)? != 0, "outputPath":row.get::<_,Option<String>>(9)?, "verification":row.get::<_,Option<String>>(10)?, "installedSerial":row.get::<_,Option<String>>(11)?, "installResult":row.get::<_,Option<String>>(12)?
        }))).map_err(|error| format!("无法查询 APK 历史：{error}"))?.collect::<Result<Vec<_>,_>>().map_err(|error| format!("无法读取 APK 历史：{error}"))?;
        Ok(
            json!({"history":records,"truncated":history_id.is_empty() && records.len()==limit as usize}),
        )
    }
}

#[async_trait]
impl ToolHandler for StartAndroidArchiveTaskTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("start_android_archive_task", "Submit a managed read-only APK, AAR, JAR, or ZIP inspection task. The path must be in the user-authorized workspace. Returns a task ID; use query_android_tasks after it completes to read the audited result.", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["inspect"], "description": "Managed archive action" },
                "path": { "type": "string", "description": "User-authorized APK, AAR, JAR, or ZIP path" }
            },
            "required": ["action", "path"]
        }))
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        if input.get("action").and_then(Value::as_str) != Some("inspect") {
            return Err("action 目前只支持 inspect".to_string());
        }
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            return Err("压缩包路径不能为空。".to_string());
        }
        let path = PathBuf::from(path);
        if !is_allowed_agent_path(&path, &self.roots, self.allow_external_paths) {
            return Err("压缩包路径不在当前已授权工作区。".to_string());
        }
        if !path.is_file() {
            return Err("压缩包路径不是可读取的文件。".to_string());
        }
        let manager = self.app.state::<AndroidTaskManager>().inner().clone();
        let task_id = crate::android_tools::start_android_archive_inspection_with_manager(
            self.app.clone(),
            manager,
            path.to_string_lossy().to_string(),
        )?;
        Ok(json!({ "taskId": task_id, "kind": "archive-inspect", "status": "submitted" }))
    }
}

pub(super) fn is_registered_perfetto_artifact(path: &Path) -> Result<bool, String> {
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("无法定位 Trace 文件：{error}"))?;
    let connection = Connection::open(
        android_agent_data_root()?
            .join("android-tasks")
            .join("tasks.sqlite3"),
    )
    .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
    let mut statement = connection.prepare("SELECT artifact_path FROM tasks WHERE kind='perfetto' AND status='done' AND artifact_path IS NOT NULL").map_err(|error| format!("无法查询 Perfetto 任务：{error}"))?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询 Perfetto 任务：{error}"))?;
    for stored in paths {
        let stored = stored.map_err(|error| format!("无法读取 Perfetto 任务：{error}"))?;
        if fs::canonicalize(stored).ok().as_deref() == Some(canonical.as_path()) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[async_trait]
impl ToolHandler for QueryPerfettoTraceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_perfetto", "Run one bounded read-only Perfetto SQL query or a built-in diagnostic summary on a user-selected Trace or a completed Perfetto task artifact. Use the returned rows as performance-analysis evidence.", json!({
            "type":"object", "properties": { "tracePath": {"type":"string", "description":"用户选择的 Trace 路径，或 query_android_tasks 返回的已完成 Perfetto 产物路径"}, "mode": {"type":"string", "enum":["sql","diagnose"], "description":"省略时为 sql；diagnose 返回调度、CPU、Runnable、启动与帧卡顿线索；当前 Trace 缺少相应表时会标记不可用"}, "query": {"type":"string", "description":"mode=sql 时的单条 SELECT 或 WITH SQL"} }, "required":["tracePath"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let app = self
            .app
            .as_ref()
            .ok_or_else(|| "Android 桌面任务上下文不可用。".to_string())?;
        let trace_path = input
            .get("tracePath")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let mode = input.get("mode").and_then(Value::as_str).unwrap_or("sql");
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if trace_path.is_empty() || (mode == "sql" && query.is_empty()) {
            return Err("Trace 路径和 SQL 查询不能为空。".to_string());
        }
        let trace = PathBuf::from(trace_path);
        let selected = is_allowed_agent_path(&trace, &self.roots, self.allow_external_paths);
        if !selected && !is_registered_perfetto_artifact(&trace)? {
            return Err("只能查询用户明确选择的 Trace 或已完成 Perfetto 任务产物。".to_string());
        }
        if mode == "diagnose" {
            let diagnosis =
                crate::android_tools::diagnose_perfetto_trace(app.clone(), trace_path.to_string())?;
            return Ok(
                json!({"tracePath":trace_path,"mode":"diagnose","sections":diagnosis.sections}),
            );
        }
        if mode != "sql" {
            return Err("mode 只支持 sql 或 diagnose。".to_string());
        }
        let result = crate::android_tools::run_perfetto_sql(
            app.clone(),
            crate::android_tools::PerfettoSqlRequest {
                trace_path: trace_path.to_string(),
                query: query.to_string(),
            },
        )?;
        let mut output = result;
        const MAX_AGENT_PERFETTO_OUTPUT: usize = 64 * 1024;
        let truncated = agent_core::truncate_output_bytes(&mut output, MAX_AGENT_PERFETTO_OUTPUT);
        if truncated {
            output.push_str("\n[SQL 输出已截断]");
        }
        Ok(json!({"tracePath":trace_path,"query":query,"output":output,"truncated":truncated}))
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidDiagnosticsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_diagnostics", "List files in a completed Android crash-diagnostics or device-health task, or return bounded text evidence from one artifact file. Use query_android_tasks first to obtain a task ID.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "relativePath": {"type":"string", "description":"诊断产物目录内相对文件路径；省略时列出文件"}, "maxLines": {"type":"integer", "minimum":1,"maximum":500} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供诊断任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let (kind, artifact): (String, String) = connection.query_row("SELECT kind,artifact_path FROM tasks WHERE id=?1 AND kind IN ('crash-diagnostics','device-health') AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| "未找到已完成的崩溃诊断或设备健康快照任务，或该任务没有产物。".to_string())?;
        let root = fs::canonicalize(&artifact)
            .map_err(|error| format!("无法访问诊断产物目录：{error}"))?;
        if !root.is_dir() {
            return Err("诊断产物不是目录。".to_string());
        }
        let relative = input
            .get("relativePath")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if relative.is_empty() {
            let mut files = Vec::new();
            for entry in
                fs::read_dir(&root).map_err(|error| format!("无法读取诊断目录：{error}"))?
            {
                let entry = entry.map_err(|error| format!("无法读取诊断目录：{error}"))?;
                let path = entry.path();
                if path.is_file() {
                    files.push(json!({"path":entry.file_name().to_string_lossy(),"size":entry.metadata().map(|meta|meta.len()).unwrap_or(0)}));
                }
                if path.is_dir() {
                    for child in fs::read_dir(path).into_iter().flatten().flatten().take(20) {
                        if child.path().is_file() {
                            files.push(json!({"path":format!("{}/{}",entry.file_name().to_string_lossy(),child.file_name().to_string_lossy()),"size":child.metadata().map(|meta|meta.len()).unwrap_or(0)}));
                        }
                    }
                }
                if files.len() >= 100 {
                    break;
                }
            }
            return Ok(
                json!({"taskId":task_id,"kind":kind,"files":files,"truncated":files.len()>=100}),
            );
        }
        let relative = Path::new(relative);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err("relativePath 必须是诊断包内的相对路径。".to_string());
        }
        let path = root.join(relative);
        let canonical =
            fs::canonicalize(&path).map_err(|error| format!("无法读取诊断文件：{error}"))?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err("只能读取当前诊断包内的文件。".to_string());
        }
        if fs::metadata(&canonical)
            .map_err(|error| format!("无法读取诊断文件：{error}"))?
            .len()
            > 32 * 1024 * 1024
        {
            return Err("诊断文件超过 32 MB，请通过更小的诊断产物分析。".to_string());
        }
        let max_lines = input
            .get("maxLines")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 500) as usize;
        let content = fs::read_to_string(&canonical)
            .map_err(|error| format!("诊断文件不是 UTF-8 文本：{error}"))?;
        let lines = content
            .lines()
            .take(max_lines)
            .map(str::to_string)
            .collect::<Vec<_>>();
        Ok(
            json!({"taskId":task_id,"kind":kind,"relativePath":relative,"lines":lines,"truncated":content.lines().nth(max_lines).is_some()}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidBugreportTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_bugreport", "Return bounded text evidence from a completed Android bugreport task ZIP. Use query_android_tasks first to obtain a bugreport task ID; never reads arbitrary ZIP files.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "terms": {"type":"array","items":{"type":"string"},"description":"全部匹配的关键词；省略时返回可分析文本条目"}, "limit": {"type":"integer","minimum":1,"maximum":300} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 bugreport 任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='bugreport' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Bugreport 任务或该任务没有 ZIP 产物。".to_string())?;
        let file = fs::File::open(&artifact)
            .map_err(|error| format!("无法打开 Bugreport ZIP：{error}"))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| format!("无法读取 Bugreport ZIP：{error}"))?;
        let terms = input
            .get("terms")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|term| term.to_ascii_lowercase())
                    .filter(|term| !term.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(1, 300) as usize;
        let mut entries = Vec::new();
        let mut evidence = Vec::new();
        for index in 0..archive.len().min(3000) {
            if self.cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            let entry = archive
                .by_index(index)
                .map_err(|error| format!("无法读取 Bugreport 条目：{error}"))?;
            if entry.is_dir() || entry.size() > 16 * 1024 * 1024 {
                continue;
            }
            let name = entry.name().to_string();
            let lower_name = name.to_ascii_lowercase();
            if !(lower_name.ends_with(".txt")
                || lower_name.ends_with(".log")
                || lower_name.contains("dumpstate"))
            {
                continue;
            }
            if terms.is_empty() {
                entries.push(name);
                if entries.len() >= limit {
                    break;
                }
                continue;
            }
            let mut text = String::new();
            use std::io::Read;
            entry
                .take(16 * 1024 * 1024)
                .read_to_string(&mut text)
                .map_err(|error| format!("无法读取 Bugreport 文本条目：{error}"))?;
            for line in text.lines() {
                if terms
                    .iter()
                    .all(|term| line.to_ascii_lowercase().contains(term))
                {
                    evidence.push(json!({"entry":name,"line":line}));
                    if evidence.len() >= limit {
                        break;
                    }
                }
            }
            if evidence.len() >= limit {
                break;
            }
        }
        Ok(
            json!({"taskId":task_id,"terms":terms,"entries":entries,"evidence":evidence,"truncated":(terms.is_empty() && entries.len()>=limit) || (!terms.is_empty() && evidence.len()>=limit)}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAospBuildTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_aosp_build", "Read bounded errors and warnings from an AOSP Linux build task log. Use query_android_tasks first to obtain an aosp-build task ID; never accepts arbitrary log paths.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 AOSP 构建任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let (status, artifact): (String, Option<String>) = connection
            .query_row(
                "SELECT status,artifact_path FROM tasks WHERE id=?1 AND kind='aosp-build'",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| "未找到 AOSP 构建任务。".to_string())?;
        let artifact = artifact.map(PathBuf::from).unwrap_or(
            android_agent_data_root()?
                .join("android-tasks")
                .join("aosp")
                .join(format!("{task_id}.log")),
        );
        let artifact = fs::canonicalize(artifact)
            .map_err(|error| format!("无法读取 AOSP 构建日志：{error}"))?;
        let root = fs::canonicalize(
            android_agent_data_root()?
                .join("android-tasks")
                .join("aosp"),
        )
        .map_err(|error| format!("无法定位 AOSP 任务目录：{error}"))?;
        let expected_name = format!("{task_id}.log");
        if !artifact.starts_with(&root)
            || artifact.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
        {
            return Err("AOSP 构建日志路径无效。".to_string());
        }
        let analysis =
            crate::android_tools::analyze_aosp_build_log(artifact.to_string_lossy().to_string())?;
        Ok(
            json!({"taskId":task_id,"status":status,"path":analysis.path,"errors":analysis.errors,"warnings":analysis.warnings,"truncated":analysis.truncated}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidSimpleperfTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_simpleperf", "Read bounded CPU hotspot CSV evidence from a completed Android Simpleperf task. Use query_android_tasks first to obtain a simpleperf task ID; never accepts arbitrary artifact paths.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "limit": {"type":"integer", "minimum":1, "maximum":500} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 Simpleperf 任务 ID。".to_string());
        }
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .clamp(1, 500) as usize;
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='simpleperf' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Simpleperf 任务或该任务没有产物。".to_string())?;
        let root = fs::canonicalize(artifact)
            .map_err(|error| format!("无法定位 Simpleperf 产物：{error}"))?;
        let report = root.join("report.csv");
        let canonical = fs::canonicalize(&report)
            .map_err(|error| format!("无法读取 Simpleperf CSV：{error}"))?;
        if !canonical.starts_with(&root)
            || fs::metadata(&canonical)
                .map_err(|error| format!("无法读取 Simpleperf CSV：{error}"))?
                .len()
                > 8 * 1024 * 1024
        {
            return Err("Simpleperf CSV 无效或超过 8 MB。".to_string());
        }
        let content = fs::read_to_string(canonical)
            .map_err(|error| format!("无法读取 Simpleperf CSV：{error}"))?;
        let rows = content
            .lines()
            .filter(|line| {
                !line.starts_with("Cmdline:")
                    && !line.starts_with("Arch:")
                    && !line.starts_with("Event:")
                    && !line.starts_with("Samples:")
                    && !line.starts_with("Event count:")
            })
            .take(limit + 1)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let truncated = rows.len() > limit;
        let rows = rows.into_iter().take(limit).collect::<Vec<_>>();
        Ok(json!({"taskId":task_id,"csvRows":rows,"truncated":truncated}))
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidNativeSymbolizerTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_native_symbolizer", "Read bounded Native stack symbolization evidence from a completed Android Native symbolizer task. Use query_android_tasks first to obtain a task ID; it never opens arbitrary tombstones or symbol directories.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "maxLines": {"type":"integer", "minimum":1, "maximum":2000} }, "required":["taskId"]
        })).read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 Native 符号化任务 ID。".to_string());
        }
        let limit = input
            .get("maxLines")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(1, 2000) as usize;
        let root = fs::canonicalize(
            android_agent_data_root()?
                .join("android-tasks")
                .join("native-symbolizer"),
        )
        .map_err(|error| format!("无法定位 Native 符号化任务目录：{error}"))?;
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let artifact: String = connection.query_row(
            "SELECT artifact_path FROM tasks WHERE id=?1 AND kind='native-symbolizer' AND status='done' AND artifact_path IS NOT NULL",
            [task_id], |row| row.get(0),
        ).map_err(|_| "未找到已完成的 Native 符号化任务或该任务没有产物。".to_string())?;
        let artifact = fs::canonicalize(artifact)
            .map_err(|error| format!("无法读取 Native 符号化产物：{error}"))?;
        if !artifact.starts_with(&root)
            || artifact.file_name().and_then(|name| name.to_str()) != Some(task_id)
        {
            return Err("Native 符号化任务产物路径无效。".to_string());
        }
        let output_path = fs::canonicalize(artifact.join("symbolized.txt"))
            .map_err(|error| format!("无法读取 Native 符号化输出：{error}"))?;
        if !output_path.starts_with(&artifact)
            || fs::metadata(&output_path)
                .map_err(|error| format!("无法读取 Native 符号化输出：{error}"))?
                .len()
                > 2 * 1024 * 1024
        {
            return Err("Native 符号化输出无效或超过 2 MB。".to_string());
        }
        let lines = fs::read_to_string(output_path)
            .map_err(|error| format!("无法读取 Native 符号化输出：{error}"))?
            .lines()
            .take(limit + 1)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let truncated = lines.len() > limit;
        let lines = lines.into_iter().take(limit).collect::<Vec<_>>();
        Ok(json!({"taskId":task_id,"lines":lines,"truncated":truncated}))
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidMethodProfileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_method_profile", "Read bounded metadata and the ART Trace key-section index for a completed method profiling task. Returns selected package, capture settings, managed Trace path, thread/method/class counts and overflow status. It never infers timings or hotspots from binary Trace data.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 ART 方法采样任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='method-profile' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 ART 方法采样任务或该任务没有产物。".to_string())?;
        let root = fs::canonicalize(artifact)
            .map_err(|error| format!("无法定位 ART 方法采样产物：{error}"))?;
        let trace = fs::canonicalize(root.join("method.trace"))
            .map_err(|error| format!("无法读取 ART 方法 Trace：{error}"))?;
        let metadata = fs::canonicalize(root.join("metadata.txt"))
            .map_err(|error| format!("无法读取 ART 方法采样元数据：{error}"))?;
        if !trace.starts_with(&root) || !metadata.starts_with(&root) {
            return Err("ART 方法采样产物路径无效。".to_string());
        }
        let metadata = fs::read_to_string(metadata)
            .map_err(|error| format!("无法读取 ART 方法采样元数据：{error}"))?;
        let summary = crate::android_tools::summarize_method_trace(&trace)?;
        Ok(
            json!({"taskId":task_id,"tracePath":trace,"traceSize":fs::metadata(&trace).map_err(|error| format!("无法读取 ART 方法 Trace：{error}"))?.len(),"metadata":metadata,"summary":summary}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidHeapDumpTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_heap_dump", "Read bounded metadata and a structural summary for a completed Java heap dump. When analysisTaskId is supplied, return the managed Shark object-graph result produced from that dump. It never reads arbitrary HPROF data or invents leak conclusions.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "analysisTaskId": {"type":"string", "description":"可选：已完成的 heap-shark 任务 ID"} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 Java 堆转储任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='heap-dump' AND status='done' AND artifact_path IS NOT NULL", [task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Java 堆转储任务或该任务没有产物。".to_string())?;
        let root = fs::canonicalize(artifact)
            .map_err(|error| format!("无法定位 Java 堆转储产物：{error}"))?;
        let android = fs::canonicalize(root.join("android.hprof"))
            .map_err(|error| format!("无法读取 Android HPROF：{error}"))?;
        let standard = fs::canonicalize(root.join("standard.hprof"))
            .map_err(|error| format!("无法读取标准 HPROF：{error}"))?;
        let metadata = fs::canonicalize(root.join("metadata.txt"))
            .map_err(|error| format!("无法读取堆转储元数据：{error}"))?;
        if !android.starts_with(&root)
            || !standard.starts_with(&root)
            || !metadata.starts_with(&root)
        {
            return Err("Java 堆转储产物路径无效。".to_string());
        }
        let metadata = fs::read_to_string(metadata)
            .map_err(|error| format!("无法读取堆转储元数据：{error}"))?;
        let summary = crate::android_tools::summarize_hprof_container(&standard)?;
        let shark = if let Some(analysis_task_id) = input
            .get("analysisTaskId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind='heap-shark' AND status='done' AND artifact_path IS NOT NULL", [analysis_task_id], |row| row.get(0)).map_err(|_| "未找到已完成的 Shark 对象图分析任务。".to_string())?;
            let report = fs::canonicalize(artifact)
                .map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?;
            if report.file_name().and_then(|name| name.to_str()) != Some("shark-analysis.json")
                || fs::metadata(&report)
                    .map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?
                    .len()
                    > 256 * 1024
            {
                return Err("Shark 分析产物无效。".to_string());
            }
            let source = report
                .parent()
                .ok_or_else(|| "Shark 分析产物路径无效。".to_string())?
                .join("shark-source-task.txt");
            if fs::read_to_string(source)
                .map_err(|error| format!("无法读取 Shark 来源任务：{error}"))?
                .trim()
                != task_id
            {
                return Err("Shark 分析结果不属于当前 Java 堆转储任务。".to_string());
            }
            Some(
                serde_json::from_slice::<Value>(
                    &fs::read(report)
                        .map_err(|error| format!("无法读取 Shark 分析产物：{error}"))?,
                )
                .map_err(|error| format!("无法解析 Shark 分析产物：{error}"))?,
            )
        } else {
            None
        };
        Ok(
            json!({"taskId":task_id,"androidHprofPath":android,"androidHprofSize":fs::metadata(&android).map_err(|error| format!("无法读取 Android HPROF：{error}"))?.len(),"standardHprofPath":standard,"standardHprofSize":fs::metadata(&standard).map_err(|error| format!("无法读取标准 HPROF：{error}"))?.len(),"metadata":metadata,"containerSummary":summary,"sharkAnalysis":shark}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidBenchmarkTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_benchmark", "Read the bounded AndroidX BenchmarkData summary from a completed benchmark-import or benchmark-run task. Optionally compare it with another completed benchmark task using matching median metrics. It returns only stable metric summaries, never raw per-run samples or arbitrary JSON files. Use query_android_tasks first to obtain task IDs.", json!({
            "type":"object", "properties": { "taskId": {"type":"string"}, "baselineTaskId": {"type":"string"}, "regressionThresholdPercent": {"type":"number", "minimum":0, "maximum":100} }, "required":["taskId"]
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let task_id = input
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if task_id.is_empty() {
            return Err("请提供 Benchmark 导入任务 ID。".to_string());
        }
        let connection = Connection::open(
            android_agent_data_root()?
                .join("android-tasks")
                .join("tasks.sqlite3"),
        )
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let managed_root = fs::canonicalize(
            android_agent_data_root()?
                .join("android-tasks")
                .join("benchmark"),
        )
        .map_err(|error| format!("无法定位 Benchmark 任务目录：{error}"))?;
        let read_summary = |id: &str| -> Result<Value, String> {
            let artifact: String = connection.query_row("SELECT artifact_path FROM tasks WHERE id=?1 AND kind IN ('benchmark-import','benchmark-run') AND status='done' AND artifact_path IS NOT NULL", [id], |row| row.get(0)).map_err(|_| "未找到已完成的 Benchmark 任务或该任务没有产物。".to_string())?;
            let report = fs::canonicalize(artifact)
                .map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?;
            let metadata = fs::metadata(&report)
                .map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?;
            if !report.starts_with(&managed_root)
                || report.file_name().and_then(|name| name.to_str())
                    != Some("benchmark-summary.json")
                || metadata.len() > 256 * 1024
            {
                return Err("Benchmark 摘要无效或超过 256 KB。".to_string());
            }
            serde_json::from_slice::<Value>(
                &fs::read(report).map_err(|error| format!("无法读取 Benchmark 摘要：{error}"))?,
            )
            .map_err(|error| format!("无法解析 Benchmark 摘要：{error}"))
        };
        let summary = read_summary(task_id)?;
        let baseline_id = input
            .get("baselineTaskId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if baseline_id.is_empty() {
            return Ok(json!({"taskId":task_id,"summary":summary}));
        }
        if baseline_id == task_id {
            return Err("基线任务不能与当前任务相同。".to_string());
        }
        let baseline = read_summary(baseline_id)?;
        let threshold = input
            .get("regressionThresholdPercent")
            .and_then(Value::as_f64)
            .unwrap_or(5.0)
            .clamp(0.0, 100.0);
        let metrics = |document: &Value| -> Vec<(String, f64)> {
            document
                .get("benchmarks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|case| {
                    let class_name = case.get("className").and_then(Value::as_str).unwrap_or("");
                    let name = case.get("name").and_then(Value::as_str).unwrap_or("");
                    case.get("metrics")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(move |metric| {
                            let metric_name = metric.get("name").and_then(Value::as_str)?;
                            let value = ["median", "p50", "p95", "p90"]
                                .iter()
                                .find_map(|field| metric.get(*field).and_then(Value::as_f64))?;
                            Some((format!("{class_name}\u{0}{name}\u{0}{metric_name}"), value))
                        })
                })
                .collect()
        };
        let baseline_metrics = metrics(&baseline)
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let mut comparisons = metrics(&summary).into_iter().filter_map(|(key, current)| {
            let previous = *baseline_metrics.get(&key)?;
            if previous == 0.0 { return None; }
            let mut pieces = key.split('\u{0}'); let class_name = pieces.next().unwrap_or(""); let benchmark = pieces.next().unwrap_or(""); let metric = pieces.next().unwrap_or("");
            let change_percent = (current - previous) / previous * 100.0;
            Some(json!({"className":class_name,"benchmark":benchmark,"metric":metric,"baseline":previous,"current":current,"changePercent":change_percent,"regression":change_percent > threshold}))
        }).take(201).collect::<Vec<_>>();
        let truncated = comparisons.len() > 200;
        comparisons.truncate(200);
        Ok(
            json!({"taskId":task_id,"baselineTaskId":baseline_id,"thresholdPercent":threshold,"summary":summary,"comparisons":comparisons,"truncated":truncated}),
        )
    }
}

#[async_trait]
impl ToolHandler for QueryAndroidDeviceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("query_android_device", "List connected Android devices, return one device overview, query performance-capture support, or run one bounded read-only diagnostic report through ADB. Never accepts arbitrary shell commands.", json!({
            "type":"object", "properties": { "serial": {"type":"string", "description":"设备序列号；省略时列出已连接设备"}, "report": {"type":"string", "enum":["battery","battery-stats","thermal","memory","cpu","activities","network","graphics","surfaceflinger","performance-support"], "description":"performance-support 返回 Perfetto、Simpleperf、Atrace、Root adbd、gfxinfo 与 SurfaceFlinger 合成诊断的真实可用性；其他值返回对应只读诊断"} }
        })).read_only()
    }
    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let serial = input
            .get("serial")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if serial.is_empty() {
            let devices = crate::android_tools::list_adb_devices()?;
            return Ok(json!({"devices":devices}));
        }
        let report = input
            .get("report")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !report.is_empty() {
            if report == "performance-support" {
                let matrix = crate::android_tools::probe_android_performance_support_matrix(
                    serial.to_string(),
                )
                .await?;
                return Ok(json!({"serial":serial,"report":report,"support":matrix}));
            }
            let output =
                crate::android_tools::run_adb_diagnostic(serial.to_string(), report.to_string())
                    .await?;
            let summary = crate::android_tools::summarize_adb_diagnostic(report, &output);
            const MAX_AGENT_ADB_DIAGNOSTIC_BYTES: usize = 96 * 1024;
            let mut output = output;
            let truncated =
                agent_core::truncate_output_bytes(&mut output, MAX_AGENT_ADB_DIAGNOSTIC_BYTES);
            if truncated {
                output.push_str("\n[诊断输出已截断]");
            }
            return Ok(
                json!({"serial":serial,"report":report,"summary":summary,"output":output,"truncated":truncated}),
            );
        }
        let (info, status) = futures_util::future::join(
            async { crate::android_tools::get_adb_device_info(serial.to_string()) },
            async { crate::android_tools::get_adb_device_status(serial.to_string()) },
        )
        .await;
        Ok(json!({"serial":serial,"info":info?,"status":status?}))
    }
}

pub(super) fn android_agent_data_root() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|root| root.join(crate::app_identity::identifier()))
        .ok_or_else(|| "无法定位 Android Toolkit 应用数据目录".to_string())
}

pub(super) fn selected_tool_path(input: &Value, roots: &[String]) -> Result<PathBuf, String> {
    input
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .map(|path| agent_core::resolve_selected_path(&path, roots))
        .or_else(|| selected_working_directory(roots))
        .ok_or_else(|| "请先选择工作目录或文件".to_string())
}

#[async_trait]
impl ToolHandler for InspectWinScopeTraceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "inspect_winscope_trace",
            "Inspect a selected WinScope .pb or .zip trace. The default overview returns bounded SurfaceFlinger and WindowManager snapshots together so one tool call is normally enough; request a specific view only when more detail is needed. Never return the complete trace document.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "用户选择的 WinScope .pb 或 .zip 文件路径" },
                    "view": { "type": "string", "enum": ["overview", "summary", "surface_flinger", "window_manager", "transactions", "transitions"] },
                    "snapshotIndex": { "type": "integer", "minimum": 0 },
                    "maxItems": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["path"]
            }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let path = selected_tool_path(&input, &self.roots)?;
        if !is_allowed_agent_path(&path, &self.roots, self.allow_external_paths) {
            return Err("只能分析用户明确选择的 WinScope 文件".to_string());
        }
        let metadata =
            fs::metadata(&path).map_err(|error| format!("无法读取 WinScope 文件信息：{error}"))?;
        if !metadata.is_file() {
            return Err("WinScope 分析只支持文件".to_string());
        }
        if metadata.len() > MAX_AGENT_TRACE_BYTES {
            return Err(format!(
                "WinScope 文件超过 {} MB 限制",
                MAX_AGENT_TRACE_BYTES / 1024 / 1024
            ));
        }
        let data = fs::read(&path).map_err(|error| format!("无法读取 WinScope 文件：{error}"))?;
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        let source_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("winscope.trace");
        let document = if source_name.to_ascii_lowercase().ends_with(".zip") {
            parse_trace_archive(&data, source_name)
        } else {
            parse_trace_bytes(&data, source_name)
        }
        .map_err(|error| format!("WinScope 文件解析失败：{error}"))?;
        let view = input
            .get("view")
            .and_then(Value::as_str)
            .unwrap_or("overview");
        let snapshot_index = input
            .get("snapshotIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(usize::MAX as u64) as usize;
        let max_items = input
            .get("maxItems")
            .and_then(Value::as_u64)
            .unwrap_or(8)
            .clamp(1, 32) as usize;
        summarize_trace_document(&document, view, snapshot_index, max_items)
    }
}

#[async_trait]
impl ToolHandler for RebuildTimelineTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "rebuild_log_timeline",
            "Rebuild a bounded event timeline from selected local logs.",
            json!({
                "type": "object",
                "properties": { "max_events": { "type": "integer", "maximum": 512 } }
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let max_events = input
            .get("max_events")
            .and_then(Value::as_u64)
            .unwrap_or(64) as usize;
        serde_json::to_value(rebuild_timeline_with_cancellation(
            &self.paths,
            max_events,
            &self.cancellation,
        )?)
        .map_err(|error| error.to_string())
    }
}
