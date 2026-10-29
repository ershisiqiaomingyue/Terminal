use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Output,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone)]
struct TaskControl {
    cancellation: Arc<AtomicBool>,
    // Device-side install tools cannot reliably be interrupted after their
    // transaction begins. Keep the task running and report its real result.
    cancellable: Arc<AtomicBool>,
}

impl TaskControl {
    fn new() -> Self {
        Self {
            cancellation: Arc::new(AtomicBool::new(false)),
            cancellable: Arc::new(AtomicBool::new(true)),
        }
    }

    // Callers hold the task map lock while changing these flags, making the
    // transition atomic with respect to cancellation requests.
    fn enter_non_cancellable(&self) -> bool {
        if self.cancellation.load(Ordering::Relaxed) {
            return false;
        }
        self.cancellable.store(false, Ordering::Relaxed);
        true
    }

    fn request_cancellation(&self) -> bool {
        if !self.cancellable.load(Ordering::Relaxed) {
            return false;
        }
        self.cancellation.store(true, Ordering::Relaxed);
        true
    }
}

#[derive(Default, Clone)]
pub struct AndroidTaskManager {
    running: Arc<Mutex<HashMap<String, TaskControl>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidTaskRecord {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub status: String,
    pub progress: u8,
    pub message: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub artifact_path: Option<String>,
    pub error: Option<String>,
    /// P5-E：提交时刻的所属 turn（后端权威下发）。前端停止作用域优先用它，
    /// 不再单靠本地事件流推断；旧任务无此字段时回退本地推断。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidTaskAuditRecord {
    pub id: i64,
    pub task_id: String,
    pub created_at: i64,
    pub event: String,
    pub detail: String,
}

/// Instant ADB actions are intentionally kept out of the long-running task
/// list, but still need a bounded audit trail for later diagnosis.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidOperationAuditRecord {
    pub id: i64,
    pub created_at: i64,
    pub action: String,
    pub target: String,
    pub status: String,
    pub detail: String,
}

/// A bounded, read-only inventory of a task artifact. Keeping this separate
/// from the task record lets the UI and Agent inspect large artifact folders
/// without reading their binary payloads into memory.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidTaskArtifactEntry {
    pub path: String,
    pub relative_path: String,
    pub is_directory: bool,
    pub size: u64,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn task_store(app: &AppHandle) -> Result<Connection, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("android-tasks");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建任务目录：{error}"))?;
    let connection = Connection::open(root.join("tasks.sqlite3"))
        .map_err(|error| format!("无法打开 Android 任务记录：{error}"))?;
    // Capture, progress and audit threads can write concurrently. Let SQLite
    // wait briefly for the active transaction instead of dropping a task
    // update with an immediate "database is locked" error.
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("无法配置 Android 任务数据库：{error}"))?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, kind TEXT NOT NULL, target TEXT NOT NULL, status TEXT NOT NULL, progress INTEGER NOT NULL, message TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, artifact_path TEXT, error TEXT); CREATE TABLE IF NOT EXISTS task_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, created_at INTEGER NOT NULL, event TEXT NOT NULL, detail TEXT NOT NULL); CREATE TABLE IF NOT EXISTS operation_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL, action TEXT NOT NULL, target TEXT NOT NULL, status TEXT NOT NULL, detail TEXT NOT NULL)").map_err(|error| format!("无法初始化 Android 任务记录：{error}"))?;
    // P5-D：任务归属到 turn——清扫按「所属 turn 已结束」判定（N7 表驱动），
    // 不再单赖内存 running map（panic 后 map 条目不清会让僵尸清不掉）。
    let _ = connection.execute_batch("ALTER TABLE tasks ADD COLUMN turn_id TEXT");
    Ok(connection)
}

static TASK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Time makes task IDs readable in audit reports; the sequence prevents two
/// UI actions in one millisecond from sharing a SQLite primary key.
fn make_id(kind: &str) -> String {
    format!(
        "{}-{}-{}",
        kind,
        now_millis(),
        TASK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

/// P5-D：任务提交时刻的所属 turn。同一时刻每会话最多一个活跃 turn
///（AgentRequestGuard 保证），任务必然由它提交——无需穿透传参。
fn owning_turn_id(app: &AppHandle, session_id: &str) -> Option<String> {
    let state = app.try_state::<agent_core::AgentRuntimeState>()?;
    let requests = state.requests.lock().ok()?;
    requests.get(session_id).map(|control| control.turn_id.clone())
}

pub fn create_task(
    app: &AppHandle,
    state: &AndroidTaskManager,
    kind: &str,
    target: &str,
    message: &str,
) -> Result<(String, Arc<AtomicBool>), String> {
    let id = make_id(kind);
    let control = TaskControl::new();
    let cancellation = control.cancellation.clone();
    state
        .running
        .lock()
        .map_err(|_| "Android 任务状态不可用。".to_string())?
        .insert(id.clone(), control);
    let turn_id = owning_turn_id(app, target);
    task_store(app)?.execute("INSERT INTO tasks (id,kind,target,status,progress,message,started_at,turn_id) VALUES (?1,?2,?3,'running',0,?4,?5,?6)", params![id, kind, target, message, now_millis(), turn_id]).map_err(|error| format!("无法创建 Android 任务：{error}"))?;
    record_task_audit(app, &id, "created", message)?;
    publish(app, &id, "running", 0, message, None, None)?;
    Ok((id, cancellation))
}

pub fn update_task(app: &AppHandle, id: &str, progress: u8, message: &str) -> Result<(), String> {
    task_store(app)?
        .execute(
            "UPDATE tasks SET progress=?2,message=?3 WHERE id=?1",
            params![id, progress.min(100), message],
        )
        .map_err(|error| format!("无法更新 Android 任务：{error}"))?;
    record_task_audit(app, id, "progress", message)?;
    publish(app, id, "running", progress, message, None, None)
}

fn persisted_task_result<'a>(
    cancelled: bool,
    artifact_path: Option<&'a str>,
    error: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if cancelled {
        (None, None)
    } else {
        (artifact_path, error)
    }
}

pub fn finish_task(
    app: &AppHandle,
    state: &AndroidTaskManager,
    id: &str,
    artifact_path: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let cancelled = state
        .running
        .lock()
        .map_err(|_| "Android 任务状态不可用。".to_string())?
        .remove(id)
        .is_some_and(|control| control.cancellation.load(Ordering::Relaxed));
    // A cancelled capture can leave a partial file or an empty destination.
    // Its cancellation audit remains available, but it is not a usable result.
    let (artifact_path, error) = persisted_task_result(cancelled, artifact_path, error);
    let status = if cancelled {
        "cancelled"
    } else if error.is_some() {
        "error"
    } else {
        "done"
    };
    let message = error.unwrap_or(if cancelled {
        "任务已取消"
    } else {
        "任务完成"
    });
    task_store(app)?.execute("UPDATE tasks SET status=?2,progress=?3,message=?4,finished_at=?5,artifact_path=?6,error=?7 WHERE id=?1", params![id, status, if status == "done" { 100 } else { 0 }, message, now_millis(), artifact_path, error]).map_err(|error| format!("无法完成 Android 任务：{error}"))?;
    record_task_audit(app, id, status, message)?;
    publish(
        app,
        id,
        status,
        if status == "done" { 100 } else { 0 },
        message,
        artifact_path,
        error,
    )
}

pub fn record_task_audit(
    app: &AppHandle,
    task_id: &str,
    event: &str,
    detail: &str,
) -> Result<(), String> {
    let detail = if detail.len() > 16 * 1024 {
        let boundary = detail
            .char_indices()
            .take_while(|(index, _)| *index <= 16 * 1024)
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0);
        format!("{}\n[审计内容已截断]", &detail[..boundary])
    } else {
        detail.to_string()
    };
    task_store(app)?
        .execute(
            "INSERT INTO task_audit (task_id,created_at,event,detail) VALUES (?1,?2,?3,?4)",
            params![task_id, now_millis(), event, detail],
        )
        .map_err(|error| format!("无法记录 Android 任务审计：{error}"))?;
    Ok(())
}

/// Makes a task non-cancellable immediately before an external tool starts a
/// device-side transaction. The switch and cancellation check share the task
/// lock, so a cancellation request has one unambiguous outcome.
pub fn enter_non_cancellable_phase(
    app: &AppHandle,
    state: &AndroidTaskManager,
    id: &str,
    message: &str,
) -> Result<(), String> {
    let cancelled = {
        let running = state
            .running
            .lock()
            .map_err(|_| "Android 任务状态不可用。".to_string())?;
        let control = running
            .get(id)
            .ok_or_else(|| "该任务已结束，无法进入安装阶段。".to_string())?;
        !control.enter_non_cancellable()
    };
    if cancelled {
        return Err("任务在设备安装开始前已取消。".to_string());
    }
    record_task_audit(app, id, "non-cancellable", message)?;
    Ok(())
}

/// Stores a bounded, reproducible record for a typed external command. The
/// task UI and Agent can then distinguish a missing device capability from a
/// successful command that returned no useful records.
pub fn record_command_audit(
    app: &AppHandle,
    task_id: &str,
    label: &str,
    program: &str,
    args: &[&str],
    output: &Output,
) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = format!(
        "{label}\n命令: {program} {}\n退出码: {}\n标准输出:\n{}\n标准错误:\n{}",
        args.join(" "),
        output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "信号终止".to_string()),
        stdout.trim(),
        stderr.trim(),
    );
    record_task_audit(app, task_id, "command", &detail)
}

pub fn record_operation_audit(
    app: &AppHandle,
    action: &str,
    target: &str,
    status: &str,
    detail: &str,
) -> Result<(), String> {
    let detail = if detail.len() > 4 * 1024 {
        format!(
            "{}\n[操作详情已截断]",
            &detail[..detail.floor_char_boundary(4 * 1024)]
        )
    } else {
        detail.to_string()
    };
    task_store(app)?.execute("INSERT INTO operation_audit (created_at,action,target,status,detail) VALUES (?1,?2,?3,?4,?5)", params![now_millis(), action, target, status, detail]).map_err(|error| format!("无法记录 Android 操作审计：{error}"))?;
    Ok(())
}

fn publish(
    app: &AppHandle,
    id: &str,
    _status: &str,
    _progress: u8,
    _message: &str,
    _artifact_path: Option<&str>,
    _error: Option<&str>,
) -> Result<(), String> {
    let record = task_store(app)?.query_row(
        "SELECT id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error,turn_id FROM tasks WHERE id=?1",
        params![id],
        |row| Ok(AndroidTaskRecord {
            id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?, status: row.get(3)?, progress: row.get(4)?, message: row.get(5)?, started_at: row.get(6)?, finished_at: row.get(7)?, artifact_path: row.get(8)?, error: row.get(9)?, turn_id: row.get(10)?,
        }),
    ).map_err(|database_error| format!("无法读取 Android 任务进度：{database_error}"))?;
    // A concurrent completion/cancellation can supersede the caller between
    // its write and this read. Publish the latest durable state rather than
    // asserting that an older progress notification is still current.
    app.emit("android:task", record)
        .map_err(|event_error| format!("无法发送 Android 任务进度：{event_error}"))
}

#[tauri::command(async)]
pub fn list_android_tasks(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
) -> Result<Vec<AndroidTaskRecord>, String> {
    let connection = task_store(&app)?;
    let active = state
        .running
        .lock()
        .map_err(|_| "Android 任务状态不可用。".to_string())?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let stale = {
        // N7 表驱动清扫：除了「内存 running map 没有」（应用退出/异常中断），
        // 还判定「所属 turn 已结束或切换」（tasks.turn_id 与该会话当前活跃
        // turn 不再一致）——panic 后内存 map 条目不清的僵尸由此兜住。
        let live_turns = app
            .try_state::<agent_core::AgentRuntimeState>()
            .and_then(|state| {
                state
                    .requests
                    .lock()
                    .ok()
                    .map(|requests| {
                        requests
                            .iter()
                            .map(|(session, control)| (session.clone(), control.turn_id.clone()))
                            .collect::<std::collections::HashMap<_, _>>()
                    })
            })
            .unwrap_or_default();
        let mut stale_statement = connection
            .prepare("SELECT id, target, turn_id FROM tasks WHERE status='running'")
            .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
        let records = stale_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|error| format!("无法读取 Android 任务：{error}"))?
            .filter_map(Result::ok)
            .filter(|(id, target, turn_id)| {
                if !active.contains(id) {
                    return true;
                }
                // 有 turn 归属且归属已失效（活跃表里该会话的 turn 与任务归属不同，
                // 或会话已无活跃 turn）→ 该任务不可能再有合法进展。
                match turn_id.as_deref() {
                    None => false,
                    Some(turn) => live_turns.get(target).map(String::as_str) != Some(turn),
                }
            })
            .map(|(id, _, _)| id)
            .collect::<Vec<_>>();
        records
    };
    for id in stale {
        connection.execute("UPDATE tasks SET status='error',progress=0,message='应用退出或异常中断。',finished_at=?2,error='应用退出或异常中断。' WHERE id=?1", params![id, now_millis()]).map_err(|error| format!("无法恢复 Android 任务状态：{error}"))?;
        record_task_audit(
            &app,
            &id,
            "interrupted",
            "应用退出或异常中断，任务状态已恢复。",
        )?;
    }
    let mut statement = connection.prepare("SELECT id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error,turn_id FROM tasks ORDER BY started_at DESC LIMIT 200").map_err(|error| format!("无法读取 Android 任务：{error}"))?;
    let tasks = statement
        .query_map([], |row| {
            Ok(AndroidTaskRecord {
                id: row.get(0)?,
                kind: row.get(1)?,
                target: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                message: row.get(5)?,
                started_at: row.get(6)?,
                finished_at: row.get(7)?,
                artifact_path: row.get(8)?,
                error: row.get(9)?, turn_id: row.get(10)?,
            })
        })
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Android 任务：{error}"))?;
    Ok(tasks)
}

#[tauri::command]
pub fn list_android_task_audit(
    app: AppHandle,
    task_id: String,
) -> Result<Vec<AndroidTaskAuditRecord>, String> {
    let connection = task_store(&app)?;
    let mut statement = connection.prepare("SELECT id,task_id,created_at,event,detail FROM task_audit WHERE task_id=?1 ORDER BY id ASC LIMIT 500").map_err(|error| format!("无法读取 Android 任务审计：{error}"))?;
    let records = statement
        .query_map(params![task_id.trim()], |row| {
            Ok(AndroidTaskAuditRecord {
                id: row.get(0)?,
                task_id: row.get(1)?,
                created_at: row.get(2)?,
                event: row.get(3)?,
                detail: row.get(4)?,
            })
        })
        .map_err(|error| format!("无法读取 Android 任务审计：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Android 任务审计：{error}"))?;
    Ok(records)
}

#[tauri::command]
pub fn list_android_operation_audit(
    app: AppHandle,
) -> Result<Vec<AndroidOperationAuditRecord>, String> {
    let connection = task_store(&app)?;
    let mut statement = connection.prepare("SELECT id,created_at,action,target,status,detail FROM operation_audit ORDER BY id DESC LIMIT 200").map_err(|error| format!("无法读取 Android 操作审计：{error}"))?;
    let records = statement
        .query_map([], |row| {
            Ok(AndroidOperationAuditRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                action: row.get(2)?,
                target: row.get(3)?,
                status: row.get(4)?,
                detail: row.get(5)?,
            })
        })
        .map_err(|error| format!("无法读取 Android 操作审计：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取 Android 操作审计：{error}"))?;
    Ok(records)
}

fn collect_artifact_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<AndroidTaskArtifactEntry>,
) -> Result<(), String> {
    if entries.len() >= 300 {
        return Ok(());
    }
    let mut children = fs::read_dir(current)
        .map_err(|error| format!("无法读取任务产物目录：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取任务产物目录：{error}"))?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        if entries.len() >= 300 {
            break;
        }
        let path = child.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法读取任务产物信息：{error}"))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let relative_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let is_directory = metadata.is_dir();
        entries.push(AndroidTaskArtifactEntry {
            path: path.to_string_lossy().to_string(),
            relative_path,
            is_directory,
            size: if is_directory { 0 } else { metadata.len() },
        });
        if is_directory {
            collect_artifact_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

/// Lists a bounded artifact manifest for one completed task. A task artifact
/// is always resolved from the persisted task record; callers cannot pass an
/// arbitrary local path.
#[tauri::command]
pub fn list_android_task_artifacts(
    app: AppHandle,
    task_id: String,
) -> Result<Vec<AndroidTaskArtifactEntry>, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("任务 ID 不能为空。".to_string());
    }
    let connection = task_store(&app)?;
    let artifact: String = connection
        .query_row(
            "SELECT artifact_path FROM tasks WHERE id=?1 AND artifact_path IS NOT NULL",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(|_| "该任务没有可读取的产物。".to_string())?;
    let root = fs::canonicalize(&artifact).map_err(|error| format!("无法读取任务产物：{error}"))?;
    let metadata = fs::metadata(&root).map_err(|error| format!("无法读取任务产物：{error}"))?;
    if metadata.is_file() {
        return Ok(vec![AndroidTaskArtifactEntry {
            path: root.to_string_lossy().to_string(),
            relative_path: root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact")
                .to_string(),
            is_directory: false,
            size: metadata.len(),
        }]);
    }
    let mut entries = Vec::new();
    collect_artifact_entries(&root, &root, &mut entries)?;
    Ok(entries)
}

/// Exports task state, bounded audit events and the artifact manifest as a
/// portable UTF-8 report. The destination is user-chosen by the UI; inputs
/// are never copied or modified by this command.
#[tauri::command]
pub fn export_android_task_report(
    app: AppHandle,
    task_id: String,
    destination: String,
) -> Result<String, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("任务 ID 不能为空。".to_string());
    }
    let destination = PathBuf::from(destination.trim());
    let parent = destination
        .parent()
        .ok_or_else(|| "导出文件路径无效。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建导出目录：{error}"))?;
    let connection = task_store(&app)?;
    let task = connection.query_row(
        "SELECT id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error,turn_id FROM tasks WHERE id=?1",
        params![task_id],
        |row| Ok(AndroidTaskRecord { id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?, status: row.get(3)?, progress: row.get(4)?, message: row.get(5)?, started_at: row.get(6)?, finished_at: row.get(7)?, artifact_path: row.get(8)?, error: row.get(9)?, turn_id: row.get(10)? }),
    ).map_err(|_| "任务不存在。".to_string())?;
    let mut statement = connection.prepare("SELECT id,created_at,event,detail FROM task_audit WHERE task_id=?1 ORDER BY id ASC LIMIT 500").map_err(|error| format!("无法读取任务审计：{error}"))?;
    let audit = statement
        .query_map(params![task_id], |row| {
            Ok(AndroidTaskAuditRecord {
                id: row.get(0)?,
                task_id: task_id.to_string(),
                created_at: row.get(1)?,
                event: row.get(2)?,
                detail: row.get(3)?,
            })
        })
        .map_err(|error| format!("无法读取任务审计：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取任务审计：{error}"))?;
    drop(statement);
    let artifacts = list_android_task_artifacts(app, task_id.to_string()).unwrap_or_default();
    let mut report = format!(
        "Android Toolkit 任务报告\n\n任务 ID: {}\n类型: {}\n目标: {}\n状态: {}\n进度: {}%\n开始时间(ms): {}\n结束时间(ms): {}\n信息: {}\n错误: {}\n产物路径: {}\n\n产物清单（最多 300 项）:\n",
        task.id, task.kind, task.target, task.status, task.progress, task.started_at,
        task.finished_at.map(|value| value.to_string()).unwrap_or_else(|| "未结束".to_string()),
        task.message, task.error.unwrap_or_else(|| "无".to_string()), task.artifact_path.unwrap_or_else(|| "无".to_string(),),
    );
    if artifacts.is_empty() {
        report.push_str("（无可读取产物）\n");
    }
    for item in artifacts {
        report.push_str(&format!(
            "{}\t{}\t{}\n",
            if item.is_directory {
                "目录"
            } else {
                "文件"
            },
            item.size,
            item.relative_path
        ));
    }
    report.push_str("\n任务审计：\n");
    if audit.is_empty() {
        report.push_str("（无审计事件）\n");
    }
    for item in audit {
        report.push_str(&format!(
            "\n[{}] {}\n{}\n",
            item.created_at, item.event, item.detail
        ));
    }
    fs::write(&destination, report).map_err(|error| format!("无法写入任务报告：{error}"))?;
    Ok(destination.to_string_lossy().to_string())
}

#[tauri::command]
pub fn cancel_android_task(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    id: String,
) -> Result<(), String> {
    let control = state
        .running
        .lock()
        .map_err(|_| "Android 任务状态不可用。".to_string())?
        .get(&id)
        .cloned()
        .ok_or_else(|| "该任务已结束，无法取消。".to_string())?;
    if !control.request_cancellation() {
        return Err("任务已进入设备安装阶段，无法安全取消；将继续记录实际安装结果。".to_string());
    }
    update_task(&app, &id, 0, "正在取消任务...")
}

#[cfg(test)]
mod tests {
    use super::{make_id, persisted_task_result, TaskControl};

    #[test]
    fn cancellation_before_install_prevents_entering_the_device_phase() {
        let control = TaskControl::new();
        assert!(control.request_cancellation());
        assert!(!control.enter_non_cancellable());
    }

    #[test]
    fn device_phase_rejects_late_cancellation_without_marking_the_task_cancelled() {
        let control = TaskControl::new();
        assert!(control.enter_non_cancellable());
        assert!(!control.request_cancellation());
        assert!(!control
            .cancellation
            .load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn cancelled_tasks_do_not_expose_partial_artifacts_or_errors() {
        assert_eq!(
            persisted_task_result(true, Some("partial.pftrace"), Some("请求已取消")),
            (None, None),
        );
        assert_eq!(
            persisted_task_result(false, Some("complete.pftrace"), None),
            (Some("complete.pftrace"), None),
        );
        assert_eq!(
            persisted_task_result(false, None, Some("设备断开")),
            (None, Some("设备断开")),
        );
    }

    #[test]
    fn task_ids_remain_distinct_for_back_to_back_creates() {
        let first = make_id("perfetto");
        let second = make_id("perfetto");
        assert_ne!(first, second);
        assert!(first.starts_with("perfetto-"));
        assert!(second.starts_with("perfetto-"));
    }
}
