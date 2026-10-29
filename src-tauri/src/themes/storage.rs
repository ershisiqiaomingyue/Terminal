use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, fs, path::Path, sync::Mutex};

static LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePreferences {
    pub mode: String,
    pub active_id: Option<String>,
    pub themes: Vec<Value>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(flatten)]
    preferences: ThemePreferences,
    #[serde(default)]
    migrated_origins: HashSet<String>,
}

fn validate(preferences: &ThemePreferences) -> Result<(), String> {
    if !matches!(preferences.mode.as_str(), "light" | "dark") || preferences.themes.len() > 300 {
        return Err("主题配置无效或超过数量限制".into());
    }
    let mut ids = HashSet::new();
    for theme in &preferences.themes {
        let id = theme["id"].as_str().filter(|id| !id.is_empty()).ok_or("主题缺少标识")?;
        if !ids.insert(id) || !theme["colors"].is_object() { return Err("主题配置无效".into()); }
    }
    if preferences.active_id.as_deref().is_some_and(|id| !ids.contains(id)) { return Err("当前主题不在主题列表中".into()); }
    Ok(())
}

fn read(path: &Path) -> Result<Option<Stored>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if bytes.len() > MAX_BYTES { return Err("主题配置文件过大".into()); }
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|e| format!("主题配置读取失败：{e}"))?;
    validate(&stored.preferences)?;
    Ok(Some(stored))
}

fn write(path: &Path, stored: &Stored) -> Result<(), String> {
    validate(&stored.preferences)?;
    let bytes = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES { return Err("主题配置文件过大".into()); }
    crate::user_storage::atomic_write(path, &bytes)
}

fn initialize(path: &Path, origin: &str, legacy: ThemePreferences) -> Result<ThemePreferences, String> {
    if origin.len() > 4096 { return Err("主题来源无效".into()); }
    validate(&legacy)?;
    let mut stored = read(path)?.unwrap_or_else(|| Stored { preferences: legacy.clone(), ..Stored::default() });
    if stored.migrated_origins.insert(origin.to_owned()) {
        let existing: HashSet<String> = stored.preferences.themes.iter().filter_map(|theme| theme["id"].as_str().map(str::to_owned)).collect();
        stored.preferences.themes.extend(legacy.themes.into_iter().filter(|theme| theme["id"].as_str().is_some_and(|id| !existing.contains(id))));
        write(path, &stored)?;
    }
    Ok(stored.preferences)
}

fn path() -> Result<std::path::PathBuf, String> {
    crate::user_storage::directory("themes").map(|root| root.join("themes.json"))
}

#[tauri::command]
pub async fn initialize_theme_preferences(origin: String, legacy: ThemePreferences) -> Result<ThemePreferences, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = LOCK.lock().map_err(|e| e.to_string())?;
        initialize(&path()?, &origin, legacy)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_theme_preferences() -> Result<Option<ThemePreferences>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = LOCK.lock().map_err(|e| e.to_string())?;
        read(&path()?).map(|stored| stored.map(|stored| stored.preferences))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_theme_preferences(preferences: ThemePreferences) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = LOCK.lock().map_err(|e| e.to_string())?;
        let path = path()?;
        let mut stored = read(&path)?.unwrap_or_default();
        stored.preferences = preferences;
        write(&path, &stored)
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_merge_once_and_do_not_resurrect_deleted_themes() {
        let root = std::env::temp_dir().join(format!("android-theme-store-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        let path = root.join("themes.json");
        let theme = |id| serde_json::json!({"id":id,"colors":{}});
        let initial = ThemePreferences { mode:"light".into(), active_id:Some("github".into()), themes:vec![theme("github")] };
        assert_eq!(initialize(&path, "http://127.0.0.1:1422", initial.clone()).unwrap().themes.len(), 1);
        let merged = initialize(&path, "http://tauri.localhost", ThemePreferences { mode:"dark".into(), active_id:None, themes:vec![theme("dracula")] }).unwrap();
        assert_eq!(merged.themes.len(), 2);
        assert_eq!(merged.mode, "light");
        let mut stored = read(&path).unwrap().unwrap();
        stored.preferences.active_id = None;
        stored.preferences.themes.clear();
        write(&path, &stored).unwrap();
        assert!(initialize(&path, "http://127.0.0.1:1422", initial).unwrap().themes.is_empty());
        assert!(validate(&ThemePreferences { mode:"light".into(), active_id:Some("missing".into()), themes:vec![] }).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
