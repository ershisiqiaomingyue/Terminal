use crate::{ConnectionStatus, ProviderKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderSettings {
    #[serde(
        rename = "promptCacheRetention",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt_cache_retention: Option<String>,
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub model: String,
    #[serde(rename = "contextWindow")]
    pub context_window: Option<u32>,
    #[serde(rename = "inputPricePerMillion", default)]
    pub input_price_per_million: Option<f64>,
    #[serde(rename = "outputPricePerMillion", default)]
    pub output_price_per_million: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(rename = "quotaUrls", default, skip_serializing_if = "Option::is_none")]
    pub quota_urls: Option<ProviderQuotaUrls>,
    #[serde(
        rename = "usageScript",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_script: Option<String>,
    #[serde(
        rename = "usageUserId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_user_id: Option<String>,
    #[serde(default)]
    pub models: Vec<AgentModelSettings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaUrls {
    pub balance_url: Option<String>,
    pub spending_url: Option<String>,
}

impl ProviderQuotaUrls {
    pub fn resolve(base_url: &str, endpoint: &str) -> Result<url::Url, String> {
        let base = url::Url::parse(base_url).map_err(|_| "Base URL 无效")?;
        let endpoint = endpoint.trim();
        let url = if endpoint.starts_with('/') && !endpoint.starts_with("//") {
            base.join(endpoint)
        } else {
            url::Url::parse(endpoint)
        }
        .map_err(|_| "查询接口需要完整 HTTP(S) URL 或以 / 开头的路径")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("查询接口仅支持 HTTP(S)，不能包含账号密码或片段".to_string());
        }
        if base.scheme() == "https" && url.scheme() == "http" {
            return Err("查询接口不能从 HTTPS 降级到 HTTP".to_string());
        }
        Ok(url)
    }

    pub fn normalized(self, base_url: &str) -> Result<Option<Self>, String> {
        let normalize = |value: Option<String>| -> Result<Option<String>, String> {
            value
                .filter(|value| !value.trim().is_empty())
                .map(|value| Self::resolve(base_url, &value).map(|url| url.to_string()))
                .transpose()
        };
        let balance_url = normalize(self.balance_url)?;
        let spending_url = normalize(self.spending_url)?;
        Ok(
            (balance_url.is_some() || spending_url.is_some()).then_some(Self {
                balance_url,
                spending_url,
            }),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelBilling {
    #[serde(default)]
    pub multiplier: Option<f64>,
    #[serde(default)]
    pub badges: Vec<String>,
    #[serde(default)]
    pub free: bool,
    #[serde(default)]
    pub rate_unknown: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelSettings {
    #[serde(
        rename = "reasoningEfforts",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_efforts: Option<Vec<String>>,
    #[serde(rename = "iconBrand", default, skip_serializing_if = "Option::is_none")]
    pub icon_brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<AgentModelBilling>,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<ProviderKind>,
    #[serde(rename = "contextWindow", default)]
    pub context_window: Option<u32>,
    /// 用户（或上游接口）为该模型声明的单轮输出上限；None 表示沿用内置
    /// 输出预算（按上下文窗口推算）。见 turn.rs 的 max_tokens 夹取。
    #[serde(
        rename = "maxOutputTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_output_tokens: Option<u32>,
    #[serde(rename = "supportsTools", default)]
    pub supports_tools: Option<bool>,
    #[serde(rename = "supportsVision", default)]
    pub supports_vision: Option<bool>,
    #[serde(rename = "supportsUsage", default)]
    pub supports_usage: Option<bool>,
    #[serde(rename = "supportsReasoning", default)]
    pub supports_reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderCheckRequest {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "inputPricePerMillion", default)]
    pub input_price_per_million: Option<f64>,
    #[serde(rename = "outputPricePerMillion", default)]
    pub output_price_per_million: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(rename = "quotaUrls", default, skip_serializing_if = "Option::is_none")]
    pub quota_urls: Option<ProviderQuotaUrls>,
    #[serde(
        rename = "usageScript",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_script: Option<String>,
    #[serde(
        rename = "accessToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub access_token: Option<String>,
    #[serde(
        rename = "usageUserId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelDiscoveryRequest {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<ProviderKind>,
    #[serde(rename = "contextWindow")]
    pub context_window: Option<u32>,
    /// 发现模型时上游自报的单轮输出上限（OpenRouter 的
    /// `top_provider.max_completion_tokens` 等）；用户可在设置面板里改写。
    #[serde(
        rename = "maxOutputTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_output_tokens: Option<u32>,
    #[serde(rename = "supportsTools", default)]
    pub supports_tools: Option<bool>,
    #[serde(rename = "supportsVision", default)]
    pub supports_vision: Option<bool>,
    #[serde(rename = "supportsUsage", default)]
    pub supports_usage: Option<bool>,
    #[serde(rename = "supportsReasoning", default)]
    pub supports_reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelSummary {
    #[serde(
        rename = "reasoningEfforts",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_efforts: Option<Vec<String>>,
    pub name: String,
    #[serde(rename = "iconBrand", default, skip_serializing_if = "Option::is_none")]
    pub icon_brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<AgentModelBilling>,
    pub id: String,
    #[serde(rename = "providerId")]
    pub provider_id: String,
    pub provider: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub model: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub protocol: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<ProviderKind>,
    #[serde(rename = "contextWindow")]
    pub context_window: Option<u32>,
    #[serde(rename = "supportsTools", default)]
    pub supports_tools: Option<bool>,
    #[serde(rename = "supportsVision", default)]
    pub supports_vision: Option<bool>,
    #[serde(rename = "supportsUsage", default)]
    pub supports_usage: Option<bool>,
    #[serde(rename = "supportsReasoning", default)]
    pub supports_reasoning: Option<bool>,
    #[serde(rename = "inputPricePerMillion", default)]
    pub input_price_per_million: Option<f64>,
    #[serde(rename = "outputPricePerMillion", default)]
    pub output_price_per_million: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(
        rename = "usageScript",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_script: Option<String>,
    #[serde(
        rename = "usageUserId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub usage_user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProviderSummary {
    #[serde(rename = "promptCacheRetention", default)]
    pub prompt_cache_retention: Option<String>,
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub model: String,
    pub models: Vec<AgentModelSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConnectionView {
    pub id: String,
    pub name: String,
    pub status: ConnectionStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentQuotaView {
    pub status: String,
    pub source: Option<String>,
    #[serde(rename = "remainingAmount")]
    pub remaining_amount: Option<f64>,
    #[serde(rename = "remainingTokens")]
    pub remaining_tokens: Option<u64>,
    #[serde(rename = "totalAmount")]
    pub total_amount: Option<f64>,
    #[serde(rename = "usedAmount")]
    pub used_amount: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(rename = "planName", default)]
    pub plan_name: Option<String>,
    #[serde(default)]
    pub extra: Option<String>,
    pub currency: Option<String>,
    #[serde(rename = "inputPricePerMillion")]
    pub input_price_per_million: Option<f64>,
    #[serde(rename = "outputPricePerMillion")]
    pub output_price_per_million: Option<f64>,
    pub message: String,
    #[serde(rename = "checkedAt")]
    pub checked_at: String,
}

#[cfg(test)]
mod quota_url_tests {
    use super::*;

    #[test]
    fn quota_urls_resolve_and_reject_unsafe_endpoints() {
        let base = "https://example.com/v1";
        assert_eq!(
            ProviderQuotaUrls::resolve(base, "/balance")
                .unwrap()
                .as_str(),
            "https://example.com/balance"
        );
        for invalid in [
            "file:///tmp/key",
            "javascript:alert(1)",
            "https://user:pass@example.com/balance",
            "http://example.com/balance",
            "//other.com/balance",
            "balance",
            "https://example.com/balance#fragment",
        ] {
            assert!(
                ProviderQuotaUrls::resolve(base, invalid).is_err(),
                "{invalid}"
            );
        }
        assert!(ProviderQuotaUrls::default()
            .normalized(base)
            .unwrap()
            .is_none());
        let config = ProviderQuotaUrls {
            balance_url: Some(" /balance ".into()),
            spending_url: None,
        }
        .normalized(base)
        .unwrap()
        .unwrap();
        assert_eq!(
            config.balance_url.as_deref(),
            Some("https://example.com/balance")
        );
    }

    #[test]
    fn quota_urls_preserve_old_provider_configs_and_roundtrip() {
        let mut provider: AgentProviderSettings = serde_json::from_value(serde_json::json!({
            "id":"old", "name":"Old", "kind":"open_ai_compatible", "baseUrl":"https://example.com/v1", "model":"test", "contextWindow":null
        })).unwrap();
        assert!(provider.quota_urls.is_none());
        provider.quota_urls = Some(ProviderQuotaUrls {
            balance_url: Some("/balance".into()),
            spending_url: Some("/spending".into()),
        });
        let encoded = serde_json::to_value(&provider).unwrap();
        assert_eq!(encoded["quotaUrls"]["spendingUrl"], "/spending");
        let decoded: AgentProviderSettings = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            decoded.quota_urls.unwrap().balance_url.as_deref(),
            Some("/balance")
        );
    }

    #[test]
    fn quota_query_draft_distinguishes_saved_settings_from_explicit_auto_query() {
        let mut request = serde_json::json!({"id":"test", "name":"Test", "kind":"open_ai_compatible", "baseUrl":"https://example.com/v1", "model":"test", "apiKey":null});
        let saved_query: AgentProviderCheckRequest =
            serde_json::from_value(request.clone()).unwrap();
        assert!(saved_query.quota_urls.is_none());
        request["quotaUrls"] = serde_json::json!({"balanceUrl":null,"spendingUrl":null});
        let draft_query: AgentProviderCheckRequest =
            serde_json::from_value(request.clone()).unwrap();
        assert!(draft_query
            .quota_urls
            .unwrap()
            .normalized(&draft_query.base_url)
            .unwrap()
            .is_none());
        request["quotaUrls"] =
            serde_json::json!({"balanceUrl":"/unsaved-balance","spendingUrl":null});
        let draft_query: AgentProviderCheckRequest = serde_json::from_value(request).unwrap();
        assert_eq!(
            draft_query
                .quota_urls
                .unwrap()
                .normalized(&draft_query.base_url)
                .unwrap()
                .unwrap()
                .balance_url
                .as_deref(),
            Some("https://example.com/unsaved-balance")
        );
    }
}
