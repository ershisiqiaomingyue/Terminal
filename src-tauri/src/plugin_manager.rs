//! One management API for hosted packages and existing model-provider plugins.
use serde_json::Value;
use tauri::AppHandle;
mod updates;

#[tauri::command]
pub async fn check_plugin_updates(app: AppHandle) -> Result<Vec<updates::PluginUpdate>, String> {
    updates::check(app).await
}

/// Routed by the model-plugin spec table, so a new plugin id needs no edit here.
fn model_provider(id: &str) -> bool {
    crate::model_plugins::is_model_plugin(id)
}
fn value<T: serde::Serialize>(result: Result<T, String>) -> Result<Value, String> {
    serde_json::to_value(result?).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn list_plugins(app: AppHandle) -> Result<Vec<Value>, String> {
    let mut entries = Vec::new();
    for model in crate::model_plugins::list_model_plugins(app.clone())? {
        let mut plugin = serde_json::to_value(model).map_err(|e| e.to_string())?;
        plugin["protocol"] = Value::from("model-provider");
        entries.push(plugin);
    }
    for hosted in crate::plugin_host::list_host_plugins(app)? {
        let mut plugin = serde_json::to_value(hosted).map_err(|e| e.to_string())?;
        plugin["protocol"] = Value::from("dsh");
        entries.push(plugin);
    }
    Ok(entries)
}
#[tauri::command]
pub async fn install_plugin(
    app: AppHandle,
    id: Option<String>,
    source: Option<String>,
    required_environment_tools: Option<Vec<String>>,
    request_id: Option<String>,
) -> Result<Value, String> {
    if let Some(id) = id.filter(|id| model_provider(id)) {
        return value(crate::model_plugins::install_model_plugin(app, id).await);
    }
    value(
        crate::plugin_host::install_host_plugin(
            app,
            source.ok_or("请选择插件来源")?,
            required_environment_tools.unwrap_or_else(|| vec!["node".into()]),
            request_id,
        )
        .await,
    )
}
#[tauri::command]
pub async fn set_plugin_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Value, String> {
    if model_provider(&id) {
        value(crate::model_plugins::set_model_plugin_enabled(app, id, enabled).await)
    } else {
        value(crate::plugin_host::set_host_plugin_enabled(app, id, enabled).await)
    }
}
#[tauri::command]
pub async fn refresh_plugin(app: AppHandle, id: String) -> Result<Value, String> {
    if model_provider(&id) {
        value(crate::model_plugins::refresh_model_plugin(app, id).await)
    } else {
        value(crate::plugin_host::set_host_plugin_enabled(app, id, true).await)
    }
}
#[tauri::command]
pub async fn uninstall_plugin(app: AppHandle, id: String) -> Result<Value, String> {
    if model_provider(&id) {
        value(crate::model_plugins::uninstall_model_plugin(app, id).await)
    } else {
        value(crate::plugin_host::uninstall_host_plugin(app, id).await)
    }
}
