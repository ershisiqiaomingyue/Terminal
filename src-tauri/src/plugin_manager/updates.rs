use futures_util::{stream, StreamExt};
use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdate {
    id: String,
    version: Option<String>,
    source: Option<String>,
    error: Option<String>,
}

pub(super) async fn check(app: AppHandle) -> Result<Vec<PluginUpdate>, String> {
    let plugins = super::list_plugins(app)?;
    let client = agent_core::http_client_builder()?
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(stream::iter(plugins)
        .map(|plugin| {
            let client = client.clone();
            async move {
                let id = plugin["id"].as_str().unwrap_or_default().to_string();
                let result: Result<(String, Option<String>), String> = async {
                    if plugin["protocol"] == "model-provider" {
                        let model_id = id.clone();
                        let version = tauri::async_runtime::spawn_blocking(move || {
                            crate::model_plugins::check_update(&model_id)
                        })
                        .await
                        .map_err(|e| e.to_string())??;
                        return Ok((version, None));
                    }
                    let source = plugin["source"].as_str().ok_or("插件来源缺失")?;
                    // npm sources have a strict package spec. Local copies only check their
                    // original directory/archive; never silently switch their distribution.
                    if regex::Regex::new(r"^(@[a-z0-9._-]+/)?[a-z0-9._-]+(@[a-zA-Z0-9._+-]+)?$")
                        .unwrap()
                        .is_match(source)
                    {
                        let name = plugin["name"].as_str().ok_or("插件包名缺失")?;
                        let encoded: String =
                            url::form_urlencoded::byte_serialize(name.as_bytes()).collect();
                        let document: serde_json::Value = client
                            .get(format!("https://registry.npmjs.org/{encoded}/latest"))
                            .send()
                            .await
                            .map_err(|e| e.to_string())?
                            .error_for_status()
                            .map_err(|e| e.to_string())?
                            .json()
                            .await
                            .map_err(|e| e.to_string())?;
                        let version = document["version"]
                            .as_str()
                            .ok_or("插件版本信息缺失")?
                            .to_string();
                        return Ok((version.clone(), Some(format!("{name}@{version}"))));
                    }
                    let inspected = crate::plugin_host::inspect_host_plugin(source.into()).await?;
                    if inspected.name != plugin["name"] {
                        return Err("来源中的插件包名已变化，请重新选择来源".into());
                    }
                    Ok((inspected.version, Some(source.into())))
                }
                .await;
                match result {
                    Ok((version, source)) => PluginUpdate {
                        id,
                        version: Some(version),
                        source,
                        error: None,
                    },
                    Err(error) => PluginUpdate {
                        id,
                        version: None,
                        source: None,
                        error: Some(error),
                    },
                }
            }
        })
        .buffer_unordered(4)
        .collect()
        .await)
}
