#![allow(dead_code)]

use super::*;

pub(super) fn provider_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(PROVIDER_CONFIG_FILE))
        .map_err(|error| format!("无法定位 Agent 配置目录: {error}"))
}

pub(super) fn load_providers(app: &AppHandle) -> Result<Vec<AgentProviderSettings>, String> {
    let path = provider_config_path(app)?;
    if !path.exists() {
        return Ok(crate::model_plugins::providers());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("无法读取 Agent 配置: {error}"))?;
    let stored = serde_json::from_str::<StoredProviderSettings>(&content)
        .map_err(|error| format!("Agent 配置格式无效: {error}"))?;
    let mut providers = stored.providers;
    if agent_core::migrate_legacy_provider_protocols(&mut providers) {
        // Keep the user's configured endpoint and credential reference intact;
        // only persist the protocol and model capability migration.
        if let Err(error) = save_providers(app, &providers) {
            eprintln!("无法持久化旧 Provider 协议迁移: {error}");
        }
    }
    agent_core::apply_cached_model_capabilities(&mut providers, &read_cached_capabilities(app));
    providers.extend(crate::model_plugins::providers());
    Ok(providers)
}

#[tauri::command]
pub async fn refresh_agent_capabilities(app: AppHandle) -> Result<usize, String> {
    refresh_remote_capabilities(app).await
}

pub(super) fn save_providers(
    app: &AppHandle,
    providers: &[AgentProviderSettings],
) -> Result<(), String> {
    let path = provider_config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Agent 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 Agent 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(&StoredProviderSettings {
        providers: providers
            .iter()
            .filter(|provider| !crate::model_plugins::is_plugin_provider(&provider.id))
            .cloned()
            .collect(),
    })
    .map_err(|error| format!("无法序列化 Agent 配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 Agent 配置: {error}"))
}

pub(super) fn provider_credential(provider_id: &str) -> Result<String, String> {
    if crate::model_plugins::is_plugin_provider(provider_id) {
        return crate::model_plugins::provider_token(provider_id);
    }
    let mut last_error = "未找到 Provider API Key".to_string();
    for service in [PROVIDER_CREDENTIAL_SERVICE, "android-tool-agent-provider"] {
        let entry = match Entry::new(service, provider_id) {
            Ok(entry) => entry,
            Err(error) => return Err(format!("无法访问系统凭据库: {error}")),
        };
        match entry.get_password() {
            Ok(value) if !value.trim().is_empty() => return Ok(value),
            Ok(_) => {}
            Err(KeyringError::NoEntry) => {}
            Err(error) => last_error = format!("未找到 Provider API Key: {error}"),
        }
    }
    Err(last_error)
}

pub(super) fn provider_usage_access_token(provider_id: &str) -> Option<String> {
    Entry::new(PROVIDER_USAGE_TOKEN_SERVICE, provider_id)
        .ok()?
        .get_password()
        .ok()
}

pub fn list_agent_models(app: AppHandle) -> Result<Vec<AgentModelSummary>, String> {
    Ok(agent_core::expand_agent_models(&load_providers(&app)?))
}

pub fn list_agent_providers(app: AppHandle) -> Result<Vec<AgentProviderSummary>, String> {
    Ok(agent_core::expand_agent_providers(&load_providers(&app)?))
}

/// Read a saved provider key only after the settings UI explicitly asks for
/// it. Provider listings deliberately never contain credential material.
pub fn get_agent_provider_api_key(id: String) -> Result<Option<String>, String> {
    let provider_id = id.trim();
    if provider_id.is_empty() {
        return Err("Provider ID 不能为空".to_string());
    }
    let entry = Entry::new(PROVIDER_CREDENTIAL_SERVICE, provider_id)
        .map_err(|error| format!("无法访问系统凭据库: {error}"))?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取 Provider API Key: {error}")),
    }
}

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
    usage_script: Option<String>,
    usage_user_id: Option<String>,
    usage_access_token: Option<String>,
    models: Option<Vec<AgentModelSettings>>,
    prompt_cache_retention: Option<String>,
) -> Result<AgentProviderSettings, String> {
    let provider_id = id.trim().to_string();
    let mut model_settings = agent_core::sanitized_model_settings(models.unwrap_or_default());
    let selected_model = model.trim();
    if model_settings.is_empty() && !selected_model.is_empty() {
        model_settings.push(AgentModelSettings {
            reasoning_efforts: None,
            icon_brand: None,
            billing: None,
            id: selected_model.to_string(),
            name: selected_model.to_string(),
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
    if model_settings.is_empty() {
        return Err("至少发现一个模型后再保存 Provider".to_string());
    }
    let selected_model = selected_model.to_string();
    if !selected_model.is_empty()
        && !model_settings
            .iter()
            .any(|model| model.id == selected_model)
    {
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
    let selected_context_window = model_settings
        .iter()
        .find(|model| model.id == selected_model)
        .and_then(|model| model.context_window)
        .or_else(|| context_window.filter(|value| *value >= 1024))
        .or_else(|| known_context_window(kind, &selected_model));
    let prompt_cache_retention = prompt_cache_retention.filter(|value| value != "default");
    if prompt_cache_retention
        .as_deref()
        .is_some_and(|value| value != "extended")
    {
        return Err("缓存时长仅支持默认或延长".into());
    }
    let settings = AgentProviderSettings {
        prompt_cache_retention,
        id: provider_id.clone(),
        name: name.trim().to_string(),
        kind,
        brand: brand.filter(|value| !value.trim().is_empty()),
        base_url: base_url.trim().trim_end_matches('/').to_string(),
        model: selected_model,
        context_window: selected_context_window,
        input_price_per_million: input_price_per_million
            .filter(|value| value.is_finite() && *value >= 0.0),
        output_price_per_million: output_price_per_million
            .filter(|value| value.is_finite() && *value >= 0.0),
        currency: currency.filter(|value| !value.trim().is_empty()),
        usage_script: usage_script
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string()),
        usage_user_id: usage_user_id
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string()),
        quota_urls: None,
        models: model_settings,
    };
    let config = ProviderConfig::new(
        &settings.id,
        &settings.name,
        settings.kind,
        &settings.base_url,
        format!("keyring:{}", settings.id),
    )
    .map_err(|error| error.to_string())?;
    config.validate().map_err(|error| error.to_string())?;
    if let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) {
        Entry::new(PROVIDER_CREDENTIAL_SERVICE, &settings.id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?
            .set_password(&api_key)
            .map_err(|error| format!("无法保存 API Key 到系统凭据库: {error}"))?;
    }
    if let Some(access_token) = usage_access_token.filter(|value| !value.trim().is_empty()) {
        Entry::new(PROVIDER_USAGE_TOKEN_SERVICE, &settings.id)
            .map_err(|error| format!("无法访问系统凭据库: {error}"))?
            .set_password(&access_token)
            .map_err(|error| format!("无法保存额度访问令牌到系统凭据库: {error}"))?;
    }

    let mut providers = load_providers(&app)?;
    providers.retain(|provider| provider.id != settings.id);
    providers.push(settings.clone());
    save_providers(&app, &providers)?;
    Ok(settings)
}

pub(super) fn delete_provider_credential(
    service: &str,
    provider_id: &str,
    label: &str,
) -> Result<(), String> {
    let entry =
        Entry::new(service, provider_id).map_err(|error| format!("无法访问系统凭据库：{error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除{label}：{error}")),
    }
}

pub fn delete_agent_provider(app: AppHandle, id: String) -> Result<(), String> {
    let provider_id = id.trim();
    if provider_id.is_empty() {
        return Err("Provider ID 不能为空".to_string());
    }
    let mut providers = load_providers(&app)?;
    if !providers.iter().any(|provider| provider.id == provider_id) {
        return Err("未找到要删除的 Provider".to_string());
    }
    providers.retain(|provider| provider.id != provider_id);
    save_providers(&app, &providers)?;
    delete_provider_credential(PROVIDER_CREDENTIAL_SERVICE, provider_id, "Provider API Key")?;
    delete_provider_credential(PROVIDER_USAGE_TOKEN_SERVICE, provider_id, "额度访问令牌")?;
    Ok(())
}

pub(super) async fn probe_agent_provider(
    provider: &AgentProviderSettings,
    api_key: Option<String>,
) -> Result<ConnectionStatus, String> {
    let api_key = api_key
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_default();
    let protocol = provider_protocol(provider);
    if api_key.is_empty() && protocol != ProviderKind::OpenAiCompatible {
        return Ok(ConnectionStatus::NotConfigured);
    }
    let endpoint = provider_health_endpoint(&provider)?;
    let builder = crate::runtime_env::configured_http_client_builder()?;
    let client = (if crate::model_plugins::is_plugin_provider(&provider.id) {
        builder.no_proxy()
    } else {
        builder
    })
    .timeout(std::time::Duration::from_secs(10))
    .build()
    .map_err(|error| format!("无法创建 Provider 请求: {error}"))?;
    let response = match protocol {
        ProviderKind::Anthropic => {
            client
                .get(endpoint)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
        }
        ProviderKind::OpenAi
        | ProviderKind::OpenAiResponses
        | ProviderKind::OpenAiCompatible
        | ProviderKind::DeepSeek
        | ProviderKind::Kimi
        | ProviderKind::Glm
        | ProviderKind::MiMo => {
            let request = client.get(endpoint);
            if api_key.is_empty() {
                request.send().await
            } else {
                request.bearer_auth(&api_key).send().await
            }
        }
    };
    if protocol == ProviderKind::Anthropic
        && matches!(
            response.as_ref().map(|value| value.status().as_u16()),
            Ok(404 | 405)
        )
    {
        let message_request = client
            .head(provider_message_endpoint(provider))
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01");
        let message_probe = message_request.send().await;
        return Ok(match message_probe {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 405 => {
                ConnectionStatus::connected("Anthropic 消息接口可访问，Provider 未提供模型列表")
            }
            Ok(response) => ConnectionStatus::error(format!(
                "[{}] Anthropic 消息接口返回 HTTP {}",
                provider_error_code(response.status().as_u16()),
                response.status().as_u16()
            )),
            Err(error) => {
                ConnectionStatus::error(format!("[network] Anthropic 消息接口网络失败: {error}"))
            }
        });
    }
    Ok(match response {
        Ok(response) if response.status().is_success() => {
            let selected_model = provider.model.trim();
            if selected_model.is_empty() {
                ConnectionStatus::connected("模型列表接口可访问")
            } else {
                match response.json::<Value>().await {
                    Ok(body) => match provider_model_is_listed(&body, selected_model) {
                        Some(true) => ConnectionStatus::connected(format!(
                            "模型列表接口可访问，已验证模型 {selected_model}"
                        )),
                        Some(false) => ConnectionStatus::error(format!(
                            "[model_not_found] Provider 返回的模型列表中不存在 {selected_model}"
                        )),
                        None => ConnectionStatus::connected(format!(
                            "模型列表接口可访问，但 Provider 未返回可校验的模型清单；尚未验证 {selected_model}"
                        )),
                    },
                    Err(_) => ConnectionStatus::connected(format!(
                        "模型列表接口可访问，但响应格式无法校验模型 {selected_model}"
                    )),
                }
            }
        }
        Ok(response) => ConnectionStatus::error(format!(
            "[{}] Provider 返回 HTTP {}",
            provider_error_code(response.status().as_u16()),
            response.status().as_u16()
        )),
        Err(error) => ConnectionStatus::error(format!("[network] Provider 网络失败: {error}")),
    })
}

pub(super) fn provider_model_is_listed(body: &Value, model_id: &str) -> Option<bool> {
    let models = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())?;
    Some(models.iter().any(|model| {
        model
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.eq_ignore_ascii_case(model_id))
    }))
}

#[tauri::command]
pub async fn check_agent_provider(
    request: AgentProviderCheckRequest,
) -> Result<AgentConnectionView, String> {
    let provider = AgentProviderSettings {
        prompt_cache_retention: None,
        id: request.id.trim().to_string(),
        name: request.name.trim().to_string(),
        kind: request.kind,
        brand: request.brand.clone(),
        base_url: request.base_url.trim().trim_end_matches('/').to_string(),
        model: request.model.trim().to_string(),
        context_window: None,
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        currency: request.currency.clone(),
        usage_script: request.usage_script.clone(),
        usage_user_id: request.usage_user_id.clone(),
        quota_urls: None,
        models: Vec::new(),
    };
    let api_key = request
        .api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| provider_credential(&provider.id).ok());
    let status = probe_agent_provider(&provider, api_key).await?;
    Ok(AgentConnectionView {
        id: provider.id,
        name: provider.name,
        status,
    })
}

#[tauri::command]
pub async fn get_provider_quota(
    app: AppHandle,
    request: AgentProviderCheckRequest,
) -> Result<AgentQuotaView, String> {
    if crate::model_plugins::is_plugin_provider(&request.id) {
        return crate::model_plugins::provider_quota(request.id).await;
    }
    let checked_at = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default()
    );
    let base_url = request.base_url.trim().trim_end_matches('/').to_string();
    let api_key = request
        .api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| provider_credential(request.id.trim()).ok());
    let api_key_missing = api_key.is_none();
    let saved = crate::provider_store::load_provider(&app, request.id.trim()).ok();
    let custom_urls = match &request.quota_urls {
        Some(urls) => urls.clone().normalized(&request.base_url)?,
        None => saved
            .as_ref()
            .and_then(|provider| provider.quota_urls.clone()),
    };
    if let Some(urls) = custom_urls.as_ref() {
        let mut quota_request = request.clone();
        if let Some(provider) = saved.as_ref().filter(|_| request.quota_urls.is_none()) {
            quota_request.base_url = provider.base_url.clone();
        }
        let view = query_custom_quota(
            &quota_request,
            urls,
            api_key.as_deref().unwrap_or_default(),
            request
                .access_token
                .clone()
                .filter(|token| !token.trim().is_empty())
                .or_else(|| provider_usage_access_token(request.id.trim()))
                .as_deref(),
            &checked_at,
        )
        .await?;
        // Older saved relay presets use the account endpoint with a model key.
        // Only fall back for that same-origin preset, not arbitrary custom APIs.
        let account_preset = urls.balance_url.as_deref().is_some_and(|endpoint| {
            agent_core::ProviderQuotaUrls::resolve(&quota_request.base_url, endpoint)
                .ok()
                .zip(Url::parse(&quota_request.base_url).ok())
                .is_some_and(|(url, base)| {
                    url.origin() == base.origin()
                        && url.path().trim_end_matches('/') == "/api/user/self"
                })
        });
        if view.status != "available" && account_preset {
            if let Some(fallback) = query_relay_quota(
                &quota_request,
                &quota_request.base_url,
                api_key.as_deref().unwrap_or_default(),
                &checked_at,
            )
            .await?
            {
                if fallback.status == "available" {
                    return Ok(fallback);
                }
            }
        }
        return Ok(view);
    }
    if api_key.is_none() && request.kind != ProviderKind::OpenAiCompatible {
        return Ok(AgentQuotaView {
            status: "not_configured".to_string(),
            source: None,
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: "未配置 API Key".to_string(),
            checked_at,
            ..Default::default()
        });
    }

    let api_key = api_key.unwrap_or_default();
    let usage_access_token = request
        .access_token
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| provider_usage_access_token(request.id.trim()))
        .unwrap_or_else(|| api_key.clone());

    if let Some(script) = request
        .usage_script
        .as_deref()
        .map(str::trim)
        .filter(|script| !script.is_empty())
    {
        return query_usage_script(
            &request,
            &base_url,
            &api_key,
            &usage_access_token,
            request.usage_user_id.as_deref(),
            &checked_at,
            script,
        )
        .await;
    }

    if request.brand.as_deref() == Some("deepseek") && !is_official_deepseek_base_url(&base_url) {
        if let Some(quota) = query_relay_quota(&request, &base_url, &api_key, &checked_at).await? {
            return Ok(quota);
        }
    }

    if request.brand.as_deref() != Some("deepseek") {
        if let Some(quota) = query_relay_quota(&request, &base_url, &api_key, &checked_at).await? {
            return Ok(quota);
        }
        return Ok(AgentQuotaView {
            status: "unsupported".to_string(),
            source: None,
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: if request.kind == ProviderKind::OpenAiCompatible && api_key_missing {
                "本地 OpenAI Compatible Provider 不提供余额接口；缓存指标仍以模型 usage 上报为准"
                    .to_string()
            } else {
                "该 Provider 没有统一的余额接口，未猜测剩余额度".to_string()
            },
            checked_at,
            ..Default::default()
        });
    }

    if !is_official_deepseek_base_url(&base_url) {
        return Ok(AgentQuotaView {
            status: "unsupported".to_string(),
            source: None,
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: "当前 DeepSeek 使用的是自定义线路，未调用未知的余额接口；额度请以该线路的 usage 返回为准".to_string(),
            checked_at,
            ..Default::default()
        });
    }

    let root = base_url
        .strip_suffix("/v1")
        .or_else(|| base_url.strip_suffix("/anthropic"))
        .unwrap_or(&base_url);
    let endpoint = format!("{root}/user/balance");
    let client = crate::runtime_env::configured_http_client_builder()?
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| format!("无法创建额度请求: {error}"))?;
    let response = client
        .get(endpoint.clone())
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| format!("额度接口网络失败: {error}"))?;
    if !response.status().is_success() {
        return Ok(AgentQuotaView {
            status: "error".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: format!("额度接口返回 HTTP {}", response.status().as_u16()),
            checked_at,
            ..Default::default()
        });
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("额度接口返回格式无效: {error}"))?;
    let balance = body
        .get("balance_infos")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .unwrap_or(&body);
    let amount = ["total_balance", "topped_up_balance", "balance"]
        .iter()
        .find_map(|key| balance.get(*key).and_then(value_as_f64));
    let currency = balance
        .get("currency")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(AgentQuotaView {
        status: "available".to_string(),
        source: Some(endpoint),
        remaining_amount: amount,
        remaining_tokens: None,
        currency: currency.or(request.currency.clone()),
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        message: "已读取 Provider 公开余额接口；价格来自本地配置".to_string(),
        checked_at,
        ..Default::default()
    })
}

pub(super) fn is_known_official_provider_base_url(base_url: &str) -> bool {
    let Some(host) = Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    matches!(
        host.as_str(),
        "api.openai.com"
            | "api.x.ai"
            | "api.anthropic.com"
            | "api.deepseek.com"
            | "api.moonshot.cn"
            | "api.kimi.com"
            | "open.bigmodel.cn"
            | "api.xiaomimimo.com"
            | "generativelanguage.googleapis.com"
            | "ai.google.dev"
            | "token-plan.cn-beijing.maas.aliyuncs.com"
            | "dashscope.aliyuncs.com"
            | "api.groq.com"
            | "api.mistral.ai"
            | "api.cohere.com"
            | "api.perplexity.ai"
            | "api.together.xyz"
            | "api.fireworks.ai"
            | "api.deepinfra.com"
    )
}

pub(super) fn value_as_f64(value: &Value) -> Option<f64> {
    let value = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))?;
    value.is_finite().then_some(value)
}

pub(super) fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as u64)
        })
}

pub(super) fn is_official_deepseek_base_url(base_url: &str) -> bool {
    Url::parse(base_url)
        .ok()
        .filter(|url| url.scheme().eq_ignore_ascii_case("https"))
        .and_then(|url| {
            url.host_str()
                .map(|host| host.eq_ignore_ascii_case("api.deepseek.com"))
        })
        .unwrap_or(false)
}

pub(super) fn provider_error_code(status: u16) -> &'static str {
    match status {
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        408 => "timeout",
        429 => "rate_limited",
        500..=599 => "server_failure",
        _ => "http_error",
    }
}

pub(super) fn model_capability(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
        .or_else(|| {
            value
                .get("capabilities")
                .and_then(Value::as_object)
                .and_then(|capabilities| {
                    keys.iter()
                        .find_map(|key| capabilities.get(*key).and_then(Value::as_bool))
                })
        })
}

pub(super) fn inferred_model_protocols(
    provider: &AgentProviderSettings,
    _model_id: &str,
    active_protocol: ProviderKind,
) -> Vec<ProviderKind> {
    let brand = agent_core::infer_provider_brand(provider);
    let mut protocols = match brand.as_str() {
        "openai" | "grok" => vec![ProviderKind::OpenAi, ProviderKind::OpenAiResponses],
        "anthropic" => vec![ProviderKind::Anthropic],
        "deepseek" | "kimi" | "glm" | "mimo" => {
            vec![ProviderKind::Anthropic, ProviderKind::OpenAi]
        }
        "qwen" => vec![
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
        ],
        "gemini" => vec![ProviderKind::OpenAiCompatible],
        "custom" => vec![
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenAiResponses,
            ProviderKind::OpenAiCompatible,
        ],
        _ => vec![active_protocol],
    };
    if !protocols.contains(&active_protocol) {
        protocols.insert(0, active_protocol);
    }
    protocols
}

#[tauri::command]
pub async fn discover_agent_models(
    app: AppHandle,
    request: AgentModelDiscoveryRequest,
) -> Result<Vec<AgentModelOption>, AgentCommandError> {
    let provider = AgentProviderSettings {
        prompt_cache_retention: None,
        id: request.id.trim().to_string(),
        name: request.name.trim().to_string(),
        kind: request.kind,
        brand: request.brand.clone(),
        base_url: request.base_url.trim().trim_end_matches('/').to_string(),
        model: String::new(),
        context_window: None,
        input_price_per_million: None,
        output_price_per_million: None,
        currency: None,
        usage_script: None,
        usage_user_id: None,
        quota_urls: None,
        models: Vec::new(),
    };
    let api_key = request
        .api_key
        .filter(|value| !value.trim().is_empty())
        .or_else(|| provider_credential(&provider.id).ok())
        .unwrap_or_default();
    let protocol = provider_protocol(&provider);
    if api_key.is_empty() && protocol != ProviderKind::OpenAiCompatible {
        return Err(AgentCommandError::new(
            "configuration",
            "未配置 API Key，无法发现模型",
            false,
        ));
    }
    let endpoint = provider_health_endpoint(&provider)
        .map_err(|error| AgentCommandError::new("configuration", error, false))?;
    let builder = crate::runtime_env::configured_http_client_builder()
        .map_err(|error| AgentCommandError::new("configuration", error, false))?;
    let client = (if crate::model_plugins::is_plugin_provider(&request.id) {
        builder.no_proxy()
    } else {
        builder
    })
    .timeout(std::time::Duration::from_secs(15))
    .build()
    .map_err(|error| {
        AgentCommandError::new(
            "configuration",
            format!("无法创建 Provider 请求: {error}"),
            false,
        )
    })?;
    let response = match protocol {
        ProviderKind::Anthropic => {
            client
                .get(endpoint)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
        }
        ProviderKind::OpenAi
        | ProviderKind::OpenAiResponses
        | ProviderKind::OpenAiCompatible
        | ProviderKind::DeepSeek
        | ProviderKind::Kimi
        | ProviderKind::Glm
        | ProviderKind::MiMo => {
            let request = client.get(endpoint);
            if api_key.is_empty() {
                request.send().await
            } else {
                request.bearer_auth(api_key).send().await
            }
        }
    }
    .map_err(|error| {
        AgentCommandError::provider("network", format!("Provider 网络失败: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        if protocol == ProviderKind::Anthropic && matches!(status.as_u16(), 404 | 405) {
            return Ok(Vec::new());
        }
        let code = provider_error_code(status.as_u16());
        return Err(AgentCommandError::new(
            code,
            format!("Provider 返回 HTTP {}", status.as_u16()),
            matches!(code, "timeout" | "rate_limited" | "server_failure"),
        ));
    }
    let body = response.json::<Value>().await.map_err(|error| {
        AgentCommandError::new(
            "parse",
            format!("Provider 模型列表格式无效: {error}"),
            false,
        )
    })?;

    let values = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .ok_or_else(|| AgentCommandError::new("parse", "Provider 返回中没有模型列表", false))?;
    let cached_capabilities = read_cached_capabilities(&app);
    let provider_brand = agent_core::infer_provider_brand(&provider);
    let mut models = values
        .iter()
        .filter_map(|value| {
            let id = value.get("id").and_then(Value::as_str)?.trim();
            if id.is_empty() {
                return None;
            }
            let name = value
                .get("display_name")
                .and_then(Value::as_str)
                .or_else(|| value.get("name").and_then(Value::as_str))
                .unwrap_or(id)
                .trim();
            let wire_context_window = [
                "context_length",
                "context_window",
                "context_window_tokens",
                "max_context_length",
                "max_input_tokens",
            ]
            .iter()
            .find_map(|field| value.get(*field).and_then(Value::as_u64))
            .and_then(|value| u32::try_from(value).ok());
            // 单轮输出上限：OpenRouter 放在 top_provider.max_completion_tokens，
            // 其他网关偶有平铺的 max_output_tokens；都没有就留给用户在面板里填。
            let wire_output_tokens = [
                "max_completion_tokens",
                "max_output_tokens",
                "max_output_len",
            ]
            .iter()
            .find_map(|field| value.get(*field).and_then(Value::as_u64))
            .or_else(|| {
                value
                    .get("top_provider")
                    .and_then(|top| top.get("max_completion_tokens"))
                    .and_then(Value::as_u64)
            })
            .and_then(|value| u32::try_from(value).ok());
            let capability =
                agent_core::find_model_capability(&cached_capabilities, id, Some(&provider_brand));
            let capability_protocols = capability
                .map(|record| {
                    record
                        .protocols
                        .iter()
                        .filter_map(|value| protocol_from_registry_name(value))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let protocols = if capability_protocols.is_empty() {
                inferred_model_protocols(&provider, id, protocol)
            } else {
                capability_protocols
            };
            let selected_protocol = if protocols.contains(&protocol) {
                protocol
            } else {
                protocols.first().copied().unwrap_or(protocol)
            };
            Some(AgentModelOption {
                id: id.to_string(),
                name: if name.is_empty() {
                    id.to_string()
                } else {
                    name.to_string()
                },
                protocol: Some(selected_protocol),
                protocols,
                // 优先级：上游自报 > models.dev 能力目录 > 内置厂商表。
                // 网关/中转常常自报 max_context_length，那是唯一可信的真值；
                // 内置表只在内置别名精确命中时才给值，否则留给上层兜底。
                context_window: wire_context_window
                    .or_else(|| capability.and_then(|record| record.context_window))
                    .or_else(|| known_context_window(protocol, id)),
                max_output_tokens: wire_output_tokens,
                supports_tools: model_capability(
                    value,
                    &["supports_tools", "tool_calling", "tool_use"],
                ),
                supports_vision: model_capability(
                    value,
                    &["supports_vision", "vision", "multimodal"],
                )
                .or_else(|| capability.and_then(|record| record.supports_vision)),
                supports_usage: model_capability(value, &["supports_usage", "usage"]),
                supports_reasoning: model_capability(
                    value,
                    &[
                        "supports_reasoning",
                        "reasoning",
                        "thinking",
                        "reasoning_effort",
                    ],
                )
                .or_else(|| capability.and_then(|record| record.supports_reasoning)),
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(models)
}

pub(super) fn provider_health_endpoint(provider: &AgentProviderSettings) -> Result<String, String> {
    models_endpoint(provider.kind, &provider.base_url)
}

pub(super) fn provider_message_endpoint(provider: &AgentProviderSettings) -> String {
    messages_endpoint(provider.kind, &provider.base_url)
}

#[tauri::command]
pub fn get_agent_model_context_window(
    app: AppHandle,
    model_id: String,
    kind: ProviderKind,
    brand: Option<String>,
) -> Option<u32> {
    let normalized_model = model_id.trim();
    if normalized_model.is_empty() {
        return None;
    }
    let provider_brand = brand.as_deref().filter(|value| !value.trim().is_empty());
    let records = read_cached_capabilities(&app);
    if let Some(record) =
        agent_core::find_model_capability(&records, normalized_model, provider_brand)
    {
        if record.context_window.is_some() {
            return record.context_window;
        }
    }
    let lookup_kind = match provider_brand
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("deepseek") => ProviderKind::DeepSeek,
        Some("kimi") => ProviderKind::Kimi,
        Some("glm") => ProviderKind::Glm,
        Some("mimo") => ProviderKind::MiMo,
        Some("openai") => ProviderKind::OpenAi,
        Some("anthropic") => ProviderKind::Anthropic,
        _ => kind,
    };
    known_context_window(lookup_kind, normalized_model)
}

#[tauri::command]
pub fn get_local_artifacts(paths: Vec<String>) -> Result<Vec<LocalArtifact>, String> {
    local_artifacts(paths)
}
