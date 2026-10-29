//! P6-B：Agent 设置页的数据面。
//!
//! 设置文件是 `~/.android-tool/agent-settings.json`——与 `hooks.json` 同目录，
//! 用户可以直接编辑。这里只暴露一个最小的读写面：
//!
//! - `get_agent_settings` 读出当前值（含默认值），供设置页渲染；
//! - `update_agent_settings` 校验、写盘，并**立即**更新进程内覆盖值，
//!   不必重启（旧实现在启动时把值读进 `OnceLock`，改配置要重启才生效）。
//!
//! 写盘前会合并已有内容：文件里可能有设置页不管的键（用户手写的），一概保留。
use serde_json::{json, Value};
use tauri::AppHandle;

/// 缺省权限等待秒数（P6-A/P6-B）。
pub(crate) const DEFAULT_PERMISSION_TIMEOUT_SECS: u64 = 300;
/// P6-D：缺省提问（ask_user）等待秒数，比权限更久——权限是「要不要放行」，
/// 提问是等人真正读题作答。
pub(crate) const DEFAULT_QUESTION_TIMEOUT_SECS: u64 = 600;
/// 钳位上限：一小时，再长就等于「不超时」，不如直接用 0。
pub(crate) const MAX_PERMISSION_TIMEOUT_SECS: u64 = 3600;

const SETTINGS_FILE: &str = "agent-settings.json";

fn settings_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::user_storage::root()?.join(SETTINGS_FILE))
}

fn read_settings() -> Value {
    let Ok(path) = settings_path() else {
        return json!({});
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return json!({});
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
}

/// 把任意输入钳到合法区间：0（永不超时）或 1–3600。
pub(crate) fn clamp_permission_timeout(value: Option<u64>) -> u64 {
    match value {
        None => DEFAULT_PERMISSION_TIMEOUT_SECS,
        Some(0) => 0,
        Some(seconds) => seconds.min(MAX_PERMISSION_TIMEOUT_SECS),
    }
}

/// 读出当前生效的权限等待秒数（0 = 永不超时）。
pub(crate) fn current_permission_timeout_secs() -> u64 {
    clamp_permission_timeout(
        read_settings()
            .get("permissionTimeoutSec")
            .and_then(Value::as_u64),
    )
}

/// P6-D：读出当前生效的提问等待秒数（0 = 永不超时）。
pub(crate) fn current_question_timeout_secs() -> u64 {
    let settings = read_settings();
    let Some(seconds) = settings.get("questionTimeoutSec").and_then(Value::as_u64) else {
        return DEFAULT_QUESTION_TIMEOUT_SECS;
    };
    if seconds == 0 {
        0
    } else {
        seconds.min(MAX_PERMISSION_TIMEOUT_SECS)
    }
}

#[tauri::command]
pub(crate) fn get_agent_settings() -> Result<Value, String> {
    Ok(json!({
        "permissionTimeoutSec": current_permission_timeout_secs(),
        "questionTimeoutSec": current_question_timeout_secs(),
        "defaultPermissionTimeoutSec": DEFAULT_PERMISSION_TIMEOUT_SECS,
        "defaultQuestionTimeoutSec": DEFAULT_QUESTION_TIMEOUT_SECS,
        "maxPermissionTimeoutSec": MAX_PERMISSION_TIMEOUT_SECS,
        "settingsPath": settings_path()?.to_string_lossy(),
    }))
}

/// 保存设置。两个字段都缺省表示不改。
#[tauri::command]
pub(crate) fn update_agent_settings(
    _app: AppHandle,
    permission_timeout_sec: Option<u64>,
    question_timeout_sec: Option<u64>,
) -> Result<Value, String> {
    let mut settings = read_settings();
    if let Some(seconds) = permission_timeout_sec {
        let clamped = clamp_permission_timeout(Some(seconds));
        settings["permissionTimeoutSec"] = json!(clamped);
        // 立即生效：设置页保存后不必重启。
        super::strategy::set_permission_timeout_override(clamped);
    }
    if let Some(seconds) = question_timeout_sec {
        let clamped = clamp_permission_timeout(Some(seconds));
        settings["questionTimeoutSec"] = json!(clamped);
        super::strategy::set_question_timeout_override(clamped);
    }
    let path = settings_path()?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    crate::user_storage::atomic_write(&path, &[bytes, b"\n".to_vec()].concat())?;
    Ok(json!({
        "permissionTimeoutSec": current_permission_timeout_secs(),
        "questionTimeoutSec": current_question_timeout_secs(),
        "settingsPath": path.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_timeout_is_clamped_to_the_documented_range() {
        assert_eq!(clamp_permission_timeout(None), 300);
        assert_eq!(clamp_permission_timeout(Some(0)), 0);
        assert_eq!(clamp_permission_timeout(Some(1)), 1);
        assert_eq!(clamp_permission_timeout(Some(600)), 600);
        assert_eq!(clamp_permission_timeout(Some(86_400)), 3600);
    }

    #[test]
    fn question_timeout_has_its_own_default_and_zero_means_never() {
        // P6-D：提问与权限是两档——提问缺省更久（600s），且 0 同样表示不超时。
        assert_eq!(DEFAULT_QUESTION_TIMEOUT_SECS, 600);
        assert!(DEFAULT_QUESTION_TIMEOUT_SECS > DEFAULT_PERMISSION_TIMEOUT_SECS);
        assert_eq!(clamp_permission_timeout(Some(0)), 0);
    }
}
