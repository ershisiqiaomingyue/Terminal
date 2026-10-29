//! Optional model-provider plugins. Installed separately from Skills/MCP and
//! the desktop bundle; provider credentials/endpoints live only in the host.
mod installer;
mod process;
mod spec;

use agent_core::{AgentProviderSettings, AgentQuotaView};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Manager};

static OPERATION: Mutex<()> = Mutex::new(());
static PROGRESS: Mutex<String> = Mutex::new(String::new());
static INSTALLING_PLUGIN: Mutex<String> = Mutex::new(String::new());
/// One state slot per spec row, so a new plugin needs no new static. An id with
/// no spec row gets its own fallback slot rather than slot 0, which would
/// otherwise report, stop or overwrite another plugin's state.
fn latest(id: &str) -> &'static Mutex<Option<String>> {
    static SLOTS: OnceLock<Vec<Mutex<Option<String>>>> = OnceLock::new();
    static FALLBACK: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    match spec::ids().position(|candidate| candidate == id) {
        Some(index) => &SLOTS.get_or_init(|| spec::ids().map(|_| Mutex::default()).collect())[index],
        None => FALLBACK.get_or_init(Mutex::default),
    }
}
fn live(id: &str) -> &'static Mutex<process::Snapshot> {
    static SLOTS: OnceLock<Vec<Mutex<process::Snapshot>>> = OnceLock::new();
    static FALLBACK: OnceLock<Mutex<process::Snapshot>> = OnceLock::new();
    match spec::ids().position(|candidate| candidate == id) {
        Some(index) => &SLOTS.get_or_init(|| spec::ids().map(|_| Mutex::default()).collect())[index],
        None => FALLBACK.get_or_init(Mutex::default),
    }
}
fn root(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("model-plugins").join(id))
        .map_err(|e| e.to_string())
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginConfig {
    #[serde(skip)]
    id: String,
    #[serde(default)]
    electron_file_cn: String,
    #[serde(default)]
    electron_file_ai: String,
    installed_version: Option<String>,
    directory: Option<String>,
    enabled: bool,
    #[serde(default)]
    auth_file_cn: String,
    #[serde(default)]
    auth_file_ai: String,
}
fn load_config(app: &AppHandle, id: &str) -> Result<PluginConfig, String> {
    let path = root(app, id)?.join("plugin.json");
    if !path.exists() {
        return Ok(PluginConfig {
            id: id.into(),
            ..Default::default()
        });
    }
    let mut config: PluginConfig =
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("插件配置无效：{e}"))?;
    config.id = id.into();
    Ok(config)
}
fn save_config(app: &AppHandle, config: &PluginConfig) -> Result<(), String> {
    let directory = root(app, &config.id)?;
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    // A temp file prevents a partially serialized config from being restored.
    let temporary = directory.join("plugin.json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, directory.join("plugin.json")).map_err(|e| e.to_string())
}
pub(super) fn install_directory(app: &AppHandle, config: &PluginConfig) -> Result<PathBuf, String> {
    let name = config.directory.as_deref().ok_or("请先安装插件")?;
    if name.is_empty() || !name.bytes().all(|c| c.is_ascii_digit()) {
        return Err("插件安装目录无效".into());
    }
    Ok(root(app, &config.id)?.join("versions").join(name))
}
fn validate_id(id: &str) -> Result<(), String> {
    spec::require(id).map(|_| ())
}

fn required_environment_tools(id: &str) -> &'static [&'static str] {
    spec::find(id)
        .map(|spec| spec.environment_tools)
        .unwrap_or(&[])
}

/// One credential field the settings form renders, straight from the spec so
/// the UI never branches on a plugin id.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConfigField {
    key: String,
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPluginView {
    id: String,
    name: String,
    description: String,
    source: String,
    installed_version: Option<String>,
    available_version: String,
    enabled: bool,
    status: String,
    message: String,
    auth_file_cn: String,
    auth_file_ai: String,
    electron_file_cn: String,
    electron_file_ai: String,
    icon_brand: String,
    advanced_hint: String,
    config_fields: Vec<PluginConfigField>,
    supports_reasoning_probe: bool,
    accounts: Vec<process::AccountView>,
    required_environment_tools: Vec<String>,
}
fn view(app: &AppHandle, id: &str) -> Result<ModelPluginView, String> {
    let spec = spec::require(id)?;
    let config = load_config(app, id)?;
    let progress = if *INSTALLING_PLUGIN.lock().map_err(|e| e.to_string())? == id {
        PROGRESS.lock().map_err(|e| e.to_string())?.clone()
    } else {
        String::new()
    };
    let snapshot = live(id).lock().map_err(|e| e.to_string())?.clone();
    let running = process::is_running(id);
    let status = if !progress.is_empty() {
        "installing"
    } else if config.installed_version.is_none() {
        "not-installed"
    } else if !config.enabled {
        "disabled"
    } else if running {
        "running"
    } else {
        "error"
    };
    let message = if !progress.is_empty() {
        progress
    } else if status == "error" {
        "插件未运行，请刷新重试或重新安装".into()
    } else if status == "not-installed" {
        spec.install_hint.into()
    } else if status == "disabled" {
        "已停用，模型分组已移除".into()
    } else {
        "插件正在运行，模型按账号权限同步".into()
    };
    Ok(ModelPluginView {
        id: id.into(),
        name: spec.name.into(),
        description: spec.description.into(),
        source: spec.source.into(),
        installed_version: config.installed_version,
        available_version: latest(id)
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .unwrap_or_else(|| spec.base_version.into()),
        enabled: config.enabled,
        status: status.into(),
        message,
        auth_file_cn: config.auth_file_cn,
        auth_file_ai: config.auth_file_ai,
        electron_file_cn: config.electron_file_cn,
        electron_file_ai: config.electron_file_ai,
        icon_brand: spec.icon_brand.into(),
        advanced_hint: spec.advanced_hint.into(),
        config_fields: spec
            .config_fields
            .iter()
            .map(|field| PluginConfigField {
                key: field.key.into(),
                label: field.label.into(),
            })
            .collect(),
        supports_reasoning_probe: spec.supports_probe,
        accounts: if running {
            snapshot.accounts
        } else {
            Vec::new()
        },
        required_environment_tools: spec
            .environment_tools
            .iter()
            .map(|tool| (*tool).into())
            .collect(),
    })
}

#[tauri::command]
pub fn list_model_plugins(app: AppHandle) -> Result<Vec<ModelPluginView>, String> {
    spec::ids().map(|id| view(&app, id)).collect()
}

pub(crate) fn check_update(id: &str) -> Result<String, String> {
    validate_id(id)?;
    let version = installer::latest_version(id)?;
    *latest(id).lock().map_err(|e| e.to_string())? = Some(version.clone());
    Ok(version)
}

#[tauri::command]
pub async fn install_model_plugin(app: AppHandle, id: String) -> Result<ModelPluginView, String> {
    validate_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        let previous = load_config(&app, &id)?;
        *INSTALLING_PLUGIN.lock().map_err(|e| e.to_string())? = id.clone();
        let result = (|| {
            let (directory, version) = installer::install(&app, &id)?;
            let mut next = previous.clone();
            next.directory = Some(directory);
            next.installed_version = Some(version);
            next.enabled = previous.installed_version.is_none() || previous.enabled;
            process::stop(&app, &id);
            let start_result = if next.enabled {
                process::start(&app, &next)
            } else {
                Ok(())
            };
            if let Err(error) = start_result.and_then(|_| save_config(&app, &next)) {
                process::stop(&app, &id);
                let _ = fs::remove_dir_all(install_directory(&app, &next)?);
                if previous.enabled {
                    let _ = process::start(&app, &previous);
                }
                return Err(error);
            }
            // Remove only the prior managed release, retaining credentials/settings.
            if previous.directory != next.directory {
                if let Ok(old) = install_directory(&app, &previous) {
                    let _ = fs::remove_dir_all(old);
                }
            }
            Ok(())
        })();
        if let Ok(mut progress) = PROGRESS.lock() {
            progress.clear();
        }
        if let Ok(mut installing) = INSTALLING_PLUGIN.lock() {
            installing.clear();
        }
        result?;
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_model_plugin_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<ModelPluginView, String> {
    validate_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        let previous = load_config(&app, &id)?;
        if previous.installed_version.is_none() {
            return Err("请先安装插件".into());
        }
        let mut next = previous.clone();
        next.enabled = enabled;
        if enabled {
            process::start(&app, &next)?;
        } else {
            process::stop(&app, &id);
        }
        if let Err(error) = save_config(&app, &next) {
            process::stop(&app, &id);
            if previous.enabled {
                let _ = process::start(&app, &previous);
            }
            return Err(error);
        }
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn configure_model_plugin(
    app: AppHandle,
    id: String,
    auth_file_cn: String,
    auth_file_ai: String,
    electron_file_cn: Option<String>,
    electron_file_ai: Option<String>,
) -> Result<ModelPluginView, String> {
    validate_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        let previous = load_config(&app, &id)?;
        let mut next = previous.clone();
        next.auth_file_cn = auth_file_cn.trim().into();
        next.auth_file_ai = auth_file_ai.trim().into();
        if let Some(path) = electron_file_cn {
            next.electron_file_cn = path.trim().into();
        }
        if let Some(path) = electron_file_ai {
            next.electron_file_ai = path.trim().into();
        }
        if next.enabled {
            process::stop(&app, &id);
            if let Err(error) = process::start(&app, &next) {
                let _ = process::start(&app, &previous);
                return Err(error);
            }
        }
        if let Err(error) = save_config(&app, &next) {
            process::stop(&app, &id);
            if previous.enabled {
                let _ = process::start(&app, &previous);
            }
            return Err(error);
        }
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn refresh_model_plugin(app: AppHandle, id: String) -> Result<ModelPluginView, String> {
    validate_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        let config = load_config(&app, &id)?;
        if config.enabled {
            process::start(&app, &config)?;
            process::request(&id, serde_json::json!({"method":"refresh"}))?;
        }
        // Update checks are user-triggered, not startup or per-message traffic.
        if let Ok(version) = installer::latest_version(&id) {
            if let Ok(mut latest) = latest(&id).lock() {
                *latest = Some(version);
            }
        }
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn probe_model_plugin(
    app: AppHandle, provider_id: String, model_id: String,
) -> Result<ModelPluginView, String> {
    // Probe is a per-plugin capability: only a bridge that dispatches the
    // method can answer it, so the spec table decides instead of the id.
    let spec = spec::from_provider(&provider_id)
        .filter(|spec| spec.supports_probe)
        .ok_or("该插件不支持推理档位检测")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        if !process::is_running(spec.id) { return Err(format!("请先启用 {} 插件", spec.name)); }
        process::request(spec.id, serde_json::json!({"method":"probe", "providerId":provider_id, "modelId":model_id}))?;
        view(&app, spec.id)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn uninstall_model_plugin(app: AppHandle, id: String) -> Result<ModelPluginView, String> {
    validate_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATION.lock().map_err(|e| e.to_string())?;
        let mut config = load_config(&app, &id)?;
        config.enabled = false;
        save_config(&app, &config)?;
        process::stop(&app, &id);
        let directory = root(&app, &id)?;
        crate::plugin_host::remove_plugin_files(&directory)
            .map_err(|e| format!("无法清理插件文件，请关闭占用文件的程序后重试：{e}"))?;
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) fn initialize(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(_guard) = OPERATION.lock() else {
            return;
        };
        for id in spec::ids() {
            if let Ok(config) = load_config(&app, id) {
                if config.enabled {
                    let _ = process::start(&app, &config);
                }
            }
        }
    });
}
pub(crate) fn shutdown(app: &AppHandle) {
    for id in spec::ids() {
        process::stop(app, id);
    }
}
/// Whether an id names a row of the model-plugin spec table.
pub(crate) fn is_model_plugin(id: &str) -> bool {
    spec::find(id).is_some()
}
pub(crate) fn is_plugin_provider(id: &str) -> bool {
    spec::from_provider(id).is_some() || crate::plugin_host::providers::is_provider(id)
}
pub(crate) fn providers() -> Vec<AgentProviderSettings> {
    spec::ids()
        .filter(|id| process::is_running(id))
        .flat_map(|id| {
            live(id)
                .lock()
                .map(|snapshot| {
                    snapshot
                        .providers
                        .iter()
                        .map(|p| p.settings.clone())
                        .collect::<Vec<AgentProviderSettings>>()
                })
                .unwrap_or_default()
        })
        .chain(crate::plugin_host::providers::providers())
        .collect()
}
fn plugin_id(provider_id: &str) -> Result<&'static str, String> {
    spec::from_provider(provider_id)
        .map(|spec| spec.id)
        .ok_or_else(|| "未知模型插件".into())
}
pub(crate) fn provider_token(id: &str) -> Result<String, String> {
    if crate::plugin_host::providers::is_provider(id) { return crate::plugin_host::providers::token(id); }
    let plugin = plugin_id(id)?;
    if !process::is_running(plugin) {
        return Err("模型插件未运行，请在设置的插件页启用或刷新".into());
    }
    live(plugin)
        .lock()
        .map_err(|e| e.to_string())?
        .providers
        .iter()
        .find(|p| p.settings.id == id)
        .map(|p| p.token.clone())
        .ok_or_else(|| "模型不可用，请检查登录状态并刷新插件".into())
}
pub(crate) async fn provider_quota(id: String) -> Result<AgentQuotaView, String> {
    if crate::plugin_host::providers::is_provider(&id) { return crate::plugin_host::providers::quota(id).await; }
    tauri::async_runtime::spawn_blocking(move || {
        serde_json::from_value(process::request(
            plugin_id(&id)?,
            serde_json::json!({"method":"quota","providerId":id}),
        )?)
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
/// Package name for a plugin id. The adapter table is the only source of
/// truth, so an unknown id resolves to no package rather than a fallback.
#[cfg(test)]
fn package(id: &str) -> Option<&'static str> {
    spec::find(id).map(|spec| spec.package)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An id outside the table must be rejected everywhere, never routed to a
    /// plugin that happens to be first in the list.
    #[test]
    fn unknown_ids_are_not_plugins() {
        assert!(is_model_plugin("workbuddy"));
        assert!(is_model_plugin("opencode"));
        assert!(!is_model_plugin("plugin:workbuddy:"));
        assert!(!is_model_plugin("nope"));
        assert!(required_environment_tools("nope").is_empty());
        assert_eq!(package("nope"), None);
        assert_eq!(package("workbuddy"), Some("dsh-workbuddy-connect"));
    }

    /// Provider ids carry their owning plugin in the prefix, so routing must
    /// come from the table rather than from a fallback branch.
    #[test]
    fn provider_ids_route_back_to_their_plugin() {
        let cases = [
            ("plugin:workbuddy:cn", "workbuddy"),
            ("plugin:opencode:go", "opencode"),
        ];
        for (provider, expected) in cases {
            assert!(is_plugin_provider(provider), "{provider} is not recognised");
            // plugin_id yields the spec row id (the key of latest()/live()/the
            // process slot), not the provider-id prefix it was parsed from.
            assert_eq!(plugin_id(provider).unwrap(), expected);
        }
        assert!(!is_plugin_provider("plugin:unknown:cn"));
        assert!(plugin_id("plugin:unknown:cn").is_err());
    }

    /// Plugin state is keyed by spec row. An id with no row gets a slot that no
    /// plugin uses, so it can never read or stop a real plugin's process.
    #[test]
    fn unknown_ids_never_alias_a_plugin() {
        let unknown_latest = latest("no-such-plugin");
        let unknown_live = live("no-such-plugin");
        for id in spec::ids() {
            assert!(!std::ptr::eq(unknown_latest, latest(id)), "latest() aliases {id}");
            assert!(!std::ptr::eq(unknown_live, live(id)), "live() aliases {id}");
        }
        // Repeating the lookup is stable, so the fallback is not rebuilt.
        assert!(std::ptr::eq(unknown_latest, latest("no-such-plugin")));
        assert!(std::ptr::eq(unknown_live, live("no-such-plugin")));
    }
}
