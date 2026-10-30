use std::{
    collections::HashMap,
    fs,
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
}

impl TaskControl {
    fn new() -> Self {
        Self {
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    fn request_cancellation(&self) -> bool {
        self.cancellation.swap(true, Ordering::Relaxed)
    }
}

#[derive(Default, Clone)]
pub struct TaskManager {
    running: Arc<Mutex<HashMap<String, TaskControl>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
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
    /// 任务提交时刻所属的 turn（后端权威下发）；旧记录可能没有该字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
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
        .join("tasks");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建任务目录：{error}"))?;
    let connection = Connection::open(root.join("tasks.sqlite3"))
        .map_err(|error| format!("无法打开任务记录：{error}"))?;
    // 进度与审计写入可能并发；让 SQLite 稍等活跃事务而不是立刻报错。
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("无法配置任务数据库：{error}"))?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, kind TEXT NOT NULL, target TEXT NOT NULL, status TEXT NOT NULL, progress INTEGER NOT NULL, message TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, artifact_path TEXT, error TEXT); CREATE TABLE IF NOT EXISTS task_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, created_at INTEGER NOT NULL, event TEXT NOT NULL, detail TEXT NOT NULL)")
        .map_err(|error| format!("无法初始化任务记录：{error}"))?;
    let _ = connection.execute_batch("ALTER TABLE tasks ADD COLUMN turn_id TEXT");
    Ok(connection)
}

static TASK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn make_id(kind: &str) -> String {
    format!(
        "{}-{}-{}",
        kind,
        now_millis(),
        TASK_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

/// 任务提交时刻的所属 turn。同一时刻每会话最多一个活跃 turn，任务必然由它提交。
fn owning_turn_id(app: &AppHandle, session_id: &str) -> Option<String> {
    let state = app.try_state::<agent_core::AgentRuntimeState>()?;
    let requests = state.requests.lock().ok()?;
    requests.get(session_id).map(|control| control.turn_id.clone())
}

pub fn create_task(
    app: &AppHandle,
    state: &TaskManager,
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
        .map_err(|_| "任务状态不可用。".to_string())?
        .insert(id.clone(), control);
    let turn_id = owning_turn_id(app, target);
    task_store(app)?.execute("INSERT INTO tasks (id,kind,target,status,progress,message,started_at,turn_id) VALUES (?1,?2,?3,'running',0,?4,?5,?6)", params![id, kind, target, message, now_millis(), turn_id]).map_err(|error| format!("无法创建任务：{error}"))?;
    record_task_audit(app, &id, "created", message)?;
    publish(app, &id)?;
    Ok((id, cancellation))
}

pub fn update_task(app: &AppHandle, id: &str, progress: u8, message: &str) -> Result<(), String> {
    task_store(app)?
        .execute(
            "UPDATE tasks SET progress=?2,message=?3 WHERE id=?1",
            params![id, progress.min(100), message],
        )
        .map_err(|error| format!("无法更新任务：{error}"))?;
    record_task_audit(app, id, "progress", message)?;
    publish(app, id)
}

pub fn finish_task(
    app: &AppHandle,
    state: &TaskManager,
    id: &str,
    artifact_path: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let cancelled = state
        .running
        .lock()
        .map_err(|_| "任务状态不可用。".to_string())?
        .remove(id)
        .is_some_and(|control| control.cancellation.load(Ordering::Relaxed));
    let (artifact_path, error) = if cancelled { (None, None) } else { (artifact_path, error) };
    let status = if cancelled {
        "cancelled"
    } else if error.is_some() {
        "error"
    } else {
        "done"
    };
    let message = error.unwrap_or(if cancelled { "任务已取消" } else { "任务完成" });
    task_store(app)?.execute("UPDATE tasks SET status=?2,progress=?3,message=?4,finished_at=?5,artifact_path=?6,error=?7 WHERE id=?1", params![id, status, if status == "done" { 100 } else { 0 }, message, now_millis(), artifact_path, error]).map_err(|error| format!("无法完成任务：{error}"))?;
    record_task_audit(app, id, status, message)?;
    publish(app, id)
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
        .map_err(|error| format!("无法记录任务审计：{error}"))?;
    Ok(())
}

fn read_task_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        target: row.get(2)?,
        status: row.get(3)?,
        progress: row.get(4)?,
        message: row.get(5)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        artifact_path: row.get(8)?,
        error: row.get(9)?,
        turn_id: row.get(10)?,
    })
}

const TASK_COLUMNS: &str =
    "id,kind,target,status,progress,message,started_at,finished_at,artifact_path,error,turn_id";

fn publish(app: &AppHandle, id: &str) -> Result<(), String> {
    let record = task_store(app)?
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id=?1"),
            params![id],
            read_task_record,
        )
        .map_err(|database_error| format!("无法读取任务进度：{database_error}"))?;
    // 并发完成可能覆盖调用方刚写入的状态；发布最新持久化状态而不是假设旧通知仍有效。
    app.emit("task:progress", record)
        .map_err(|event_error| format!("无法发送任务进度：{event_error}"))
}

#[tauri::command(async)]
pub fn list_tasks(
    app: AppHandle,
    state: State<'_, TaskManager>,
) -> Result<Vec<TaskRecord>, String> {
    let connection = task_store(&app)?;
    let active = state
        .running
        .lock()
        .map_err(|_| "任务状态不可用。".to_string())?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let stale = {
        // 表驱动清扫：内存 running map 没有的（应用退出/异常中断），或所属
        // turn 已结束/切换的 running 任务，统一标记为中断，避免僵尸任务。
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
                            .collect::<HashMap<_, _>>()
                    })
            })
            .unwrap_or_default();
        let mut stale_statement = connection
            .prepare("SELECT id, target, turn_id FROM tasks WHERE status='running'")
            .map_err(|error| format!("无法读取任务：{error}"))?;
        let records = stale_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|error| format!("无法读取任务：{error}"))?
            .filter_map(Result::ok)
            .filter(|(id, target, turn_id)| {
                if !active.contains(id) {
                    return true;
                }
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
        connection.execute("UPDATE tasks SET status='error',progress=0,message='应用退出或异常中断。',finished_at=?2,error='应用退出或异常中断。' WHERE id=?1", params![id, now_millis()]).map_err(|error| format!("无法恢复任务状态：{error}"))?;
        record_task_audit(
            &app,
            &id,
            "interrupted",
            "应用退出或异常中断，任务状态已恢复。",
        )?;
    }
    let mut statement = connection
        .prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY started_at DESC LIMIT 200"
        ))
        .map_err(|error| format!("无法读取任务：{error}"))?;
    let tasks = statement
        .query_map([], read_task_record)
        .map_err(|error| format!("无法读取任务：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取任务：{error}"))?;
    Ok(tasks)
}

#[tauri::command]
pub fn cancel_task(
    app: AppHandle,
    state: State<'_, TaskManager>,
    id: String,
) -> Result<(), String> {
    let control = state
        .running
        .lock()
        .map_err(|_| "任务状态不可用。".to_string())?
        .get(&id)
        .cloned()
        .ok_or_else(|| "该任务已结束，无法取消。".to_string())?;
    if !control.request_cancellation() {
        return Err("该任务已结束，无法取消。".to_string());
    }
    update_task(&app, &id, 0, "正在取消任务...")
}

#[cfg(test)]
mod tests {
    use super::{make_id, TaskControl};
    use std::sync::atomic::Ordering;

    #[test]
    fn task_ids_remain_distinct_for_back_to_back_creates() {
        let first = make_id("agent");
        let second = make_id("agent");
        assert_ne!(first, second);
        assert!(first.starts_with("agent-"));
        assert!(second.starts_with("agent-"));
    }

    #[test]
    fn cancellation_is_reported_until_removal() {
        let control = TaskControl::new();
        assert!(!control.cancellation.load(Ordering::Relaxed));
        assert!(control.request_cancellation());
        assert!(control.cancellation.load(Ordering::Relaxed));
    }
}
