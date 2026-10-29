//! Package-driven plugin host. No package-specific renderer or scheduler lives here.
mod archive;
mod installation;
mod process;
pub(crate) mod providers;
mod progress;
mod removal;
pub(crate) use removal::remove_tree as remove_plugin_files;
mod tools;
pub(crate) use tools::{prepare_turn, register_tools, PluginTurn};

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Emitter, Manager};

static OPERATIONS: Mutex<()> = Mutex::new(());
static FAILURES: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
fn failures() -> &'static Mutex<std::collections::HashMap<String, String>> {
    FAILURES.get_or_init(Mutex::default)
}
fn root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("plugins"))
}
fn validate_id(id: &str) -> Result<(), String> {
    if id.len() == 16 && id.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("插件标识无效".into())
    }
}
fn directory(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    Ok(root(app)?.join(id))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub source: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub required_environment_tools: Vec<String>,
    #[serde(default)]
    pub(crate) install_directory: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub frame_url: Option<String>,
    #[serde(default)]
    pub bridge_key: Option<String>,
    #[serde(default)]
    pub settings_inline: bool,
}
fn load(app: &AppHandle, id: &str) -> Result<Plugin, String> {
    serde_json::from_slice(
        &fs::read(directory(app, id)?.join("plugin.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn save(app: &AppHandle, plugin: &Plugin) -> Result<(), String> {
    let path = directory(app, &plugin.id)?;
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    let mut stored = plugin.clone();
    stored.frame_url = None;
    stored.bridge_key = None;
    stored.status.clear();
    stored.message.clear();
    fs::write(
        path.join("plugin.json.tmp"),
        serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(path.join("plugin.json.tmp"), path.join("plugin.json")).map_err(|e| e.to_string())
}
fn installed(app: &AppHandle) -> Result<Vec<Plugin>, String> {
    let path = root(app)?;
    if !path.is_dir() {
        return Ok(vec![]);
    }
    let mut result = vec![];
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if validate_id(&id).is_ok() && entry.path().join("plugin.json").is_file() {
            result.push(load(app, &id)?);
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
fn view(app: &AppHandle, id: &str) -> Result<Plugin, String> {
    let mut plugin = load(app, id)?;
    if let Some(runtime) = process::connection(id) {
        plugin.status = "running".into();
        plugin.frame_url = runtime.frame_url;
        plugin.bridge_key = runtime.bridge_key;
    } else {
        plugin.status = if plugin.enabled { "error" } else { "disabled" }.into();
        plugin.message = failures()
            .lock()
            .map_err(|e| e.to_string())?
            .get(id)
            .cloned()
            .unwrap_or_default();
    }
    Ok(plugin)
}
fn changed(app: &AppHandle) {
    let _ = app.emit("plugins-changed", ());
    let _ = app.emit("model-plugins-changed", ());
}
#[tauri::command]
pub async fn report_plugin_error(
    app: AppHandle,
    id: String,
    bridge_key: String,
    message: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATIONS.lock().map_err(|e| e.to_string())?;
        validate_id(&id)?;
        if process::connection(&id)
            .and_then(|plugin| plugin.bridge_key)
            .as_deref()
            != Some(&bridge_key)
        {
            return Ok(());
        }
        process::stop(&id);
        failures()
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id, message.chars().take(2000).collect());
        changed(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn list_host_plugins(app: AppHandle) -> Result<Vec<Plugin>, String> {
    installed(&app)?
        .iter()
        .map(|plugin| view(&app, &plugin.id))
        .collect()
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlugin {
    pub(crate) name: String,
    pub(crate) version: String,
    description: String,
    source: String,
    required_environment_tools: Vec<String>,
}
#[tauri::command]
pub async fn inspect_host_plugin(source: String) -> Result<LocalPlugin, String> {
    let source = source.trim().to_string();
    let pkg: serde_json::Value = if std::path::Path::new(&source).is_dir() {
        serde_json::from_slice(
            &fs::read(std::path::Path::new(&source).join("package.json"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?
    } else if std::path::Path::new(&source).is_file() {
        let path = PathBuf::from(&source);
        tauri::async_runtime::spawn_blocking(move || {
            let package = archive::Package::open(&path)?;
            serde_json::from_slice(
                &fs::read(package.root.join("package.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??
    } else {
        if !regex::Regex::new(r"^(@[a-z0-9._-]+/)?[a-z0-9._-]+(@[a-zA-Z0-9._+-]+)?$")
            .unwrap()
            .is_match(&source)
        {
            return Err("插件包名无效".into());
        }
        let at = source.rfind('@').filter(|i| *i > 0);
        let name = at.map(|i| &source[..i]).unwrap_or(&source);
        let version = at.map(|i| &source[i + 1..]).unwrap_or("latest");
        let encoded: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect();
        agent_core::http_client_builder()?
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?
            .get(format!("https://registry.npmjs.org/{encoded}/{version}"))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?
    };
    if pkg.get("dsh").is_none() && pkg.get("androidTool").is_none() {
        return Err("该包未声明插件入口（dsh / androidTool）".into());
    }
    Ok(LocalPlugin {
        name: pkg["name"].as_str().ok_or("缺少插件包名")?.into(),
        version: pkg["version"].as_str().unwrap_or_default().into(),
        description: pkg["description"].as_str().unwrap_or_default().into(),
        source,
        required_environment_tools: installation::declared_environments(&pkg)?,
    })
}
#[tauri::command]
pub async fn discover_local_plugins() -> Result<Vec<LocalPlugin>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let profile_root = std::env::var_os("DSH_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".dsh")))
            .ok_or("无法确定本机插件目录")?
            .join("profiles");
        let mut result = vec![];
        let Ok(profiles) = fs::read_dir(profile_root) else {
            return Ok(result);
        };
        for profile in profiles.flatten() {
            let Ok(packages) = fs::read_dir(profile.path().join("node_modules")) else {
                continue;
            };
            let mut paths = vec![];
            for entry in packages.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('@') {
                    if let Ok(scope) = fs::read_dir(entry.path()) {
                        paths.extend(scope.flatten().map(|e| e.path()));
                    }
                } else if !name.starts_with('.') {
                    paths.push(entry.path());
                }
            }
            for path in paths {
                let Ok(bytes) = fs::read(path.join("package.json")) else {
                    continue;
                };
                let Ok(pkg) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                if pkg.get("dsh").is_none() && pkg.get("androidTool").is_none() {
                    continue;
                }
                let name = pkg["name"].as_str().unwrap_or_default();
                if result.iter().any(|p: &LocalPlugin| p.name == name) {
                    continue;
                }
                result.push(LocalPlugin {
                    name: name.into(),
                    version: pkg["version"].as_str().unwrap_or_default().into(),
                    description: pkg["description"].as_str().unwrap_or_default().into(),
                    source: path.to_string_lossy().into_owned(),
                    required_environment_tools: installation::declared_environments(&pkg)?,
                });
            }
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn install_host_plugin(
    app: AppHandle,
    source: String,
    required_environment_tools: Vec<String>,
    request_id: Option<String>,
) -> Result<Plugin, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut progress =
            progress::InstallProgress::new(&app, request_id.as_deref().unwrap_or_default());
        progress.phase("waiting");
        let result = (|| {
            let _guard = OPERATIONS.lock().map_err(|e| e.to_string())?;
            let mut plugin =
                installation::install(&app, &source, &required_environment_tools, &mut progress)?;
            let previous = load(&app, &plugin.id).ok();
            if let Some(previous) = previous.as_ref() {
                plugin.enabled = previous.enabled;
            }
            save(&app, &plugin)?;
            process::stop(&plugin.id);
            progress.phase("starting");
            let start_result = if plugin.enabled {
                process::start(&app, &plugin)
            } else {
                Ok(())
            };
            match start_result {
                Ok(()) => {
                    failures()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .remove(&plugin.id);
                    if let Some(previous) = previous.as_ref() {
                        if previous.install_directory != plugin.install_directory
                            && previous
                                .install_directory
                                .bytes()
                                .all(|c| c.is_ascii_digit())
                            && !previous.install_directory.is_empty()
                        {
                            let _ = fs::remove_dir_all(
                                directory(&app, &plugin.id)?
                                    .join("versions")
                                    .join(&previous.install_directory),
                            );
                        }
                    }
                }
                Err(error) => {
                    if let Some(previous) = previous {
                        save(&app, &previous)?;
                        if previous.enabled {
                            let _ = process::start(&app, &previous);
                        }
                        changed(&app);
                        return Err(format!("新版本启动失败，已恢复原插件：{error}"));
                    }
                    failures()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .insert(plugin.id.clone(), error);
                }
            }
            changed(&app);
            view(&app, &plugin.id)
        })();
        progress.phase(
            if result
                .as_ref()
                .is_ok_and(|plugin: &Plugin| plugin.status != "error")
            {
                "completed"
            } else {
                "failed"
            },
        );
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn set_host_plugin_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Plugin, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATIONS.lock().map_err(|e| e.to_string())?;
        let mut plugin = load(&app, &id)?;
        process::stop(&id);
        if enabled {
            process::start(&app, &plugin)?;
            plugin = load(&app, &id)?;
        }
        let previous = plugin.clone();
        plugin.enabled = enabled;
        if let Err(error) = save(&app, &plugin) {
            process::stop(&id);
            if previous.enabled {
                let _ = process::start(&app, &previous);
            }
            return Err(error);
        }
        failures().lock().map_err(|e| e.to_string())?.remove(&id);
        changed(&app);
        view(&app, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn uninstall_host_plugin(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = OPERATIONS.lock().map_err(|e| e.to_string())?;
        let path = directory(&app, &id)?;
        if let Ok(mut plugin) = load(&app, &id) {
            plugin.enabled = false;
            save(&app, &plugin)?;
        }
        process::stop(&id);
        removal::remove_tree(&path).map_err(|e| format!("无法清理插件文件，请关闭占用文件的程序后重试：{e}"))?;
        failures().lock().map_err(|e| e.to_string())?.remove(&id);
        changed(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
pub fn initialize(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(_guard) = OPERATIONS.lock() else {
            return;
        };
        let Ok(plugins) = installed(&app) else {
            return;
        };
        for plugin in plugins.into_iter().filter(|p| p.enabled) {
            match process::start(&app, &plugin) {
                Err(error) => {
                    if let Ok(mut failures) = failures().lock() {
                        failures.insert(plugin.id, error);
                    }
                }
                Ok(()) => {
                    if let Ok(mut failures) = failures().lock() {
                        failures.remove(&plugin.id);
                    }
                }
            }
        }
        changed(&app);
    });
}
pub fn observe(payload: &serde_json::Value) {
    process::broadcast(payload);
}
pub fn shutdown() {
    process::shutdown();
}
