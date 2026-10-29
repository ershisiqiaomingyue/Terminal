//! Origin-independent project configuration shared by dev and packaged WebViews.
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use tauri::{AppHandle, Manager};

const KEYS: &[&str] = &[
    "android-tool.chat.workspaces.v1",
    "android-tool.project-automation.v1",
    "android-tool.project-automation.active-project.v1",
];

fn connection(app: &AppHandle) -> Result<Connection, String> {
    let directory = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let connection = Connection::open(directory.join("project-preferences.sqlite"))
        .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

#[tauri::command]
pub async fn get_project_preferences(app: AppHandle) -> Result<HashMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(move || read_values(&connection(&app)?))
        .await
        .map_err(|error| format!("项目配置读取任务失败：{error}"))?
}

fn read_values(connection: &Connection) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for key in KEYS {
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(value) = value {
            values.insert((*key).to_string(), value);
        }
    }
    Ok(values)
}

#[tauri::command]
pub async fn save_project_preference(
    app: AppHandle,
    key: String,
    value: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_value(&connection(&app)?, &key, &value))
        .await
        .map_err(|error| format!("项目配置保存任务失败：{error}"))?
}

fn write_value(connection: &Connection, key: &str, value: &str) -> Result<(), String> {
    if !KEYS.contains(&key) || value.len() > 1024 * 1024 {
        return Err("项目配置键或内容大小无效".into());
    }
    connection.execute(
        "INSERT INTO preferences (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_independent_values_preserve_explicit_removal_and_reject_unknown_keys() {
        let directory = std::env::temp_dir().join(format!(
            "android-project-preferences-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("preferences.sqlite");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                )
                .unwrap();
            assert!(read_values(&connection).unwrap().is_empty());
            write_value(&connection, KEYS[0], r#"[{"path":"D:/App","name":"App"}]"#).unwrap();
            assert!(write_value(&connection, "unknown-key", "value").is_err());
            write_value(&connection, KEYS[0], "[]").unwrap();
            write_value(&connection, KEYS[2], "project-one").unwrap();
        }
        let connection = Connection::open(&path).unwrap();
        let restored = read_values(&connection).unwrap();
        assert_eq!(restored[KEYS[0]], "[]");
        assert_eq!(restored[KEYS[2]], "project-one");
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
