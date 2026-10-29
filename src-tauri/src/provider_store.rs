use agent_core::{
    sanitized_model_settings, AgentModelSettings, AgentProviderSettings, ProviderConfig,
    ProviderKind,
};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

const PROVIDER_CONFIG_FILE: &str = "agent-providers.json";
const PROVIDER_CREDENTIAL_SERVICE: &str = "android-tool-provider";
const LEGACY_PROVIDER_CREDENTIAL_SERVICE: &str = "android-tool-agent-provider";
const PROVIDER_USAGE_TOKEN_SERVICE: &str = "android-tool-provider-usage-token";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredProviderSettings {
    providers: Vec<AgentProviderSettings>,
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(PROVIDER_CONFIG_FILE))
        .map_err(|error| format!("无法定位 Agent 配置目录: {error}"))
}

fn load_providers(app: &AppHandle) -> Result<Vec<AgentProviderSettings>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("无法读取 Agent 配置: {error}"))?;
    let stored = serde_json::from_str::<StoredProviderSettings>(&content)
        .map_err(|error| format!("Agent 配置格式无效: {error}"))?;
    Ok(stored.providers)
}

fn save_providers(app: &AppHandle, providers: &[AgentProviderSettings]) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Agent 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 Agent 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(&StoredProviderSettings {
        providers: providers.to_vec(),
    })
    .map_err(|error| format!("无法序列化 Agent 配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 Agent 配置: {error}"))
}

#[tauri::command]
pub fn list_agent_providers(app: AppHandle) -> Result<Vec<AgentProviderSettings>, String> {
    let mut providers = load_providers(&app)?;
    crate::model_capability_cache::ensure_capabilities_in_background(&app,
        &providers.iter().flat_map(|provider| std::iter::once(provider.model.clone())
            .chain(provider.models.iter().map(|model| model.id.clone())))
            .filter(|id| !id.is_empty()).collect::<Vec<_>>());
    agent_core::apply_cached_model_capabilities(
        &mut providers,
        &crate::model_capability_cache::read_cached_capabilities(&app),
    );
    for provider in &mut providers {
        provider.models = agent_core::normalized_model_settings(provider);
    }
    providers.extend(crate::model_plugins::providers());
    Ok(providers)
}

pub(crate) fn load_provider(app: &AppHandle, id: &str) -> Result<AgentProviderSettings, String> {
    if crate::model_plugins::is_plugin_provider(id) {
        return crate::model_plugins::providers().into_iter().find(|provider| provider.id == id)
            .ok_or_else(|| "插件模型不可用，请在设置的插件页检查登录状态".into());
    }
    load_providers(app)?
        .into_iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| "找不到这个供应商，请先在设置里保存。".to_string())
}

pub(crate) fn read_provider_api_key(id: &str) -> Result<String, String> {
    if crate::model_plugins::is_plugin_provider(id) { return crate::model_plugins::provider_token(id); }
    let provider_id = id.trim();
    if provider_id.is_empty() {
        return Err("Provider ID 不能为空".to_string());
    }
    for service in [PROVIDER_CREDENTIAL_SERVICE, LEGACY_PROVIDER_CREDENTIAL_SERVICE] {
        let entry = Entry::new(service, provider_id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?;
        match entry.get_password() {
            Ok(value) if !value.trim().is_empty() => return Ok(value),
            Ok(_) | Err(KeyringError::NoEntry) => {},
            Err(error) => return Err(format!("无法读取 Provider API Key: {error}")),
        }
    }
    Ok(String::new())
}

#[tauri::command]
pub fn get_agent_provider_api_key(id: String) -> Result<Option<String>, String> {
    if crate::model_plugins::is_plugin_provider(&id) { return Ok(None); }
    read_provider_api_key(&id).map(|value| (!value.trim().is_empty()).then_some(value))
}

#[tauri::command]
pub fn save_agent_provider(
    app: AppHandle,
    id: String,
    name: String,
    kind: ProviderKind,
    brand: Option<String>,
    base_url: String,
    model: String,
    context_window: Option<u32>,
    api_key: Option<String>,
    input_price_per_million: Option<f64>,
    output_price_per_million: Option<f64>,
    currency: Option<String>,
    quota_urls: Option<agent_core::ProviderQuotaUrls>,
    usage_access_token: Option<String>,
    models: Option<Vec<AgentModelSettings>>,
    prompt_cache_retention: Option<String>,
) -> Result<AgentProviderSettings, String> {
    if crate::model_plugins::is_plugin_provider(&id) { return Err("插件模型由插件管理，请在设置的插件页修改".into()); }
    let provider_id = id.trim().to_string();
    let provider_name = name.trim().to_string();
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    let selected_model = model.trim().to_string();
    if provider_id.is_empty() || provider_name.is_empty() || selected_model.is_empty() {
        return Err("Provider 名称、标识和模型不能为空".to_string());
    }
    ProviderConfig::new(
        &provider_id,
        &provider_name,
        kind,
        &base_url,
        format!("keyring:{provider_id}"),
    )
    .map_err(|error| error.to_string())?;
    let mut model_settings = sanitized_model_settings(models.unwrap_or_default());
    if !model_settings.iter().any(|item| item.id == selected_model) {
        model_settings.push(AgentModelSettings {
            reasoning_efforts: None,
            icon_brand: None,
            billing: None,
            id: selected_model.clone(),
            name: selected_model.clone(),
            protocol: Some(kind),
            protocols: vec![kind],
            context_window,
            max_output_tokens: None,
            supports_tools: None,
            supports_vision: None,
            supports_usage: None,
            supports_reasoning: None,
        });
    }
    let previous = load_providers(&app)?
        .into_iter()
        .find(|provider| provider.id == provider_id);
    let quota_urls = quota_urls
        .map(|urls| urls.normalized(&base_url))
        .transpose()?
        .flatten();
    let prompt_cache_retention = prompt_cache_retention.or_else(||previous.as_ref().and_then(|p|p.prompt_cache_retention.clone()));
    if prompt_cache_retention.as_deref().is_some_and(|value| !matches!(value, "default" | "extended")) {
        return Err("缓存时长仅支持默认或延长".into());
    }
    let settings = AgentProviderSettings {
        prompt_cache_retention: prompt_cache_retention.filter(|value|value != "default"),
        id: provider_id.clone(),
        name: provider_name,
        kind,
        brand: brand.filter(|value| !value.trim().is_empty()),
        base_url,
        model: selected_model,
        context_window: context_window.filter(|value| *value >= 1024),
        input_price_per_million: input_price_per_million
            .filter(|value| value.is_finite() && *value >= 0.0),
        output_price_per_million: output_price_per_million
            .filter(|value| value.is_finite() && *value >= 0.0),
        currency: currency.filter(|value| !value.trim().is_empty()),
        usage_script: previous
            .as_ref()
            .and_then(|provider| provider.usage_script.clone()),
        usage_user_id: previous
            .as_ref()
            .and_then(|provider| provider.usage_user_id.clone()),
        quota_urls,
        models: model_settings,
    };
    if let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) {
        Entry::new(PROVIDER_CREDENTIAL_SERVICE, &settings.id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?
            .set_password(&api_key)
            .map_err(|error| format!("无法保存 API Key: {error}"))?;
    }
    if let Some(token) = usage_access_token.filter(|value| !value.trim().is_empty()) {
        Entry::new(PROVIDER_USAGE_TOKEN_SERVICE, &settings.id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?
            .set_password(token.trim())
            .map_err(|error| format!("无法保存查询令牌: {error}"))?;
    }
    let mut providers = load_providers(&app)?;
    providers.retain(|provider| provider.id != settings.id);
    providers.push(settings.clone());
    save_providers(&app, &providers)?;
    Ok(settings)
}

#[tauri::command]
pub fn delete_agent_provider(app: AppHandle, id: String) -> Result<(), String> {
    if crate::model_plugins::is_plugin_provider(&id) { return Err("请在设置的插件页停用或卸载这个插件".into()); }
    let provider_id = id.trim();
    if provider_id.is_empty() {
        return Err("Provider ID 不能为空".to_string());
    }
    let mut providers = load_providers(&app)?;
    let before = providers.len();
    providers.retain(|provider| provider.id != provider_id);
    if providers.len() == before {
        return Err("未找到要删除的 Provider".to_string());
    }
    save_providers(&app, &providers)?;
    for service in [PROVIDER_CREDENTIAL_SERVICE, LEGACY_PROVIDER_CREDENTIAL_SERVICE] {
        let entry = Entry::new(service, provider_id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => return Err(format!("无法删除 Provider API Key: {error}")),
        }
    }
    if let Ok(entry) = Entry::new(PROVIDER_USAGE_TOKEN_SERVICE, provider_id) {
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => return Err(format!("无法删除查询令牌: {error}")),
        }
    }
    Ok(())
}
