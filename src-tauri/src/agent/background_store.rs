use super::*;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BackgroundJob {
    pub id: String,
    pub session_id: String,
    pub turn_id: String,
    #[serde(default)]
    pub title: String,
    pub command: String,
    pub environment: String,
    pub cwd: Option<String>,
    pub status: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub exit_code: Option<i64>,
    pub log_tail: String,
    pub log_path: String,
    pub error: Option<String>,
    pub notification_status: String,
}

pub(super) fn store(app: &AppHandle) -> Result<Connection, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("background-shells");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let connection =
        Connection::open(directory.join("tasks.sqlite3")).map_err(|e| e.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, status TEXT NOT NULL, record TEXT NOT NULL, request TEXT NOT NULL, notified INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS jobs_session ON jobs(session_id);").map_err(|e| e.to_string())?;
    Ok(connection)
}

pub(super) fn insert(
    app: &AppHandle,
    job: &BackgroundJob,
    request: &AgentChatRequest,
) -> Result<(), String> {
    store(app)?
        .execute(
            "INSERT INTO jobs(id,session_id,status,record,request) VALUES(?1,?2,?3,?4,?5)",
            params![
                job.id,
                job.session_id,
                job.status,
                serde_json::to_string(job).map_err(|e| e.to_string())?,
                serde_json::to_string(request).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn save(app: &AppHandle, job: &BackgroundJob) -> Result<(), String> {
    store(app)?
        .execute(
            "UPDATE jobs SET status=?2,record=?3 WHERE id=?1",
            params![
                job.id,
                job.status,
                serde_json::to_string(job).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn list(app: &AppHandle, session: &str) -> Result<Vec<BackgroundJob>, String> {
    let connection = store(app)?;
    let mut statement = connection
        .prepare("SELECT record FROM jobs WHERE session_id=?1 ORDER BY rowid DESC LIMIT 50")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![session], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    })
    .collect()
}

pub(super) fn pending(app: &AppHandle) -> Result<Vec<(BackgroundJob, AgentChatRequest)>, String> {
    let connection = store(app)?;
    let mut statement = connection
        .prepare("SELECT record,request FROM jobs WHERE notified=0")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let (record, request) = row.map_err(|e| e.to_string())?;
        Ok((
            serde_json::from_str(&record).map_err(|e| e.to_string())?,
            serde_json::from_str(&request).map_err(|e| e.to_string())?,
        ))
    })
    .collect()
}

pub(super) fn notified(
    app: &AppHandle,
    mut job: BackgroundJob,
    status: &str,
) -> Result<(), String> {
    job.notification_status = status.into();
    store(app)?
        .execute(
            "UPDATE jobs SET notified=1,record=?2 WHERE id=?1",
            params![
                job.id,
                serde_json::to_string(&job).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_record_preserves_nonzero_exit_and_notification_state() {
        let job = BackgroundJob {
            id: "task".into(),
            session_id: "chat".into(),
            turn_id: "turn".into(),
            title: "编译客户端".into(),
            command: "build".into(),
            environment: "ssh".into(),
            cwd: None,
            status: "error".into(),
            started_at: 1,
            finished_at: Some(2),
            exit_code: Some(42),
            log_tail: "编译失败".into(),
            log_path: "build.log".into(),
            error: None,
            notification_status: "pending".into(),
        };
        let decoded: BackgroundJob =
            serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
        assert_eq!(decoded.exit_code, Some(42));
        assert_eq!(decoded.notification_status, "pending");
        assert_eq!(decoded.title, "编译客户端");
        let mut legacy = serde_json::to_value(&job).unwrap();
        legacy.as_object_mut().unwrap().remove("title");
        let decoded: BackgroundJob = serde_json::from_value(legacy).unwrap();
        assert!(
            decoded.title.is_empty(),
            "existing task databases remain readable"
        );
    }
}
