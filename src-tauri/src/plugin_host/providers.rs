use super::process;
use agent_core::{AgentProviderSettings, AgentQuotaView};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub(super) struct HostedProvider {
    #[serde(flatten)]
    pub settings: AgentProviderSettings,
    pub token: String,
}

const PREFIX: &str = "plugin:host:";
pub(crate) fn is_provider(id: &str) -> bool {
    id.starts_with(PREFIX)
}

pub(super) fn decode(
    plugin: &str,
    value: serde_json::Value,
) -> Result<Vec<HostedProvider>, String> {
    let mut providers: Vec<HostedProvider> =
        serde_json::from_value(value).map_err(|e| e.to_string())?;
    if providers.len() > 32 {
        return Err("插件供应商数量过多".into());
    }
    let mut ids = std::collections::HashSet::new();
    for provider in &mut providers {
        let settings = &mut provider.settings;
        if settings.id.is_empty()
            || settings.id.len() > 64
            || !settings
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            || !ids.insert(settings.id.clone())
            || settings.models.is_empty()
            || settings.models.len() > 1000
            || !settings.models.iter().any(|m| m.id == settings.model)
            || provider.token.is_empty()
        {
            return Err("插件供应商声明无效".into());
        }
        let url = url::Url::parse(&settings.base_url).map_err(|e| e.to_string())?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("插件供应商必须使用本机回环接口".into());
        }
        settings.id = format!("{PREFIX}{plugin}:{}", settings.id);
        settings.models = agent_core::normalized_model_settings(settings);
    }
    Ok(providers)
}

pub(crate) fn providers() -> Vec<AgentProviderSettings> {
    process::connections()
        .into_iter()
        .flat_map(|(_, runtime)| {
            runtime
                .providers
                .lock()
                .map(|rows| rows.iter().map(|p| p.settings.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect()
}

fn location(id: &str) -> Result<(&str, &str), String> {
    id.strip_prefix(PREFIX)
        .and_then(|value| value.split_once(':'))
        .ok_or_else(|| "插件供应商标识无效".into())
}

pub(crate) fn token(id: &str) -> Result<String, String> {
    let (plugin, _) = location(id)?;
    for (owner, runtime) in process::connections() {
        if owner != plugin {
            continue;
        }
        return runtime
            .providers
            .lock()
            .map_err(|e| e.to_string())?
            .iter()
            .find(|p| p.settings.id == id)
            .map(|p| p.token.clone())
            .ok_or_else(|| "插件账号未连接，请在插件页刷新".into());
    }
    Err("插件已停止，请在插件页启用".into())
}

pub(crate) async fn quota(id: String) -> Result<AgentQuotaView, String> {
    let (plugin, local) = location(&id)?;
    let runtime = process::connections()
        .into_iter()
        .find(|(owner, _)| owner == plugin)
        .map(|(_, runtime)| runtime)
        .ok_or("插件已停止，请在插件页启用")?;
    serde_json::from_value(
        runtime
            .rpc(
                "provider-quota",
                serde_json::json!({"providerId":local}),
                "provider-quota",
            )
            .await?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row() -> serde_json::Value {
        serde_json::json!([{"id":"local","name":"Package provider","kind":"open_ai_compatible","baseUrl":"http://127.0.0.1:12345/v1","model":"m","models":[{"id":"m","name":"Model"}],"token":"private"}])
    }
    #[test]
    fn namespaces_providers_and_keeps_tokens_out_of_settings() {
        let rows = decode("0123456789abcdef", row()).unwrap();
        assert_eq!(rows[0].settings.id, "plugin:host:0123456789abcdef:local");
        assert!(!serde_json::to_string(&rows[0].settings)
            .unwrap()
            .contains("private"));
        assert_eq!(rows[0].token, "private");
    }
    #[test]
    fn rejects_external_endpoints_and_invalid_model_selection() {
        for endpoint in [
            "https://example.com/v1",
            "http://127.0.0.1.evil.test/v1",
            "http://user@127.0.0.1/v1",
        ] {
            let mut rows = row();
            rows[0]["baseUrl"] = endpoint.into();
            assert!(decode("owner", rows).is_err());
        }
        let mut rows = row();
        rows[0]["model"] = "missing".into();
        assert!(decode("owner", rows).is_err());
        let mut rows = row();
        rows[0]["id"] = "workbuddy:cn".into();
        assert!(decode("owner", rows).is_err());
        let mut rows = row();
        let duplicate = rows[0].clone();
        rows.as_array_mut().unwrap().push(duplicate);
        assert!(decode("owner", rows).is_err());
    }
}
