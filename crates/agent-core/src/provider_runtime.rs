use crate::models::{effective_context_window, output_reserve};
use crate::provider::ProviderKind;
use crate::provider_contract::{
    AgentModelSettings, AgentModelSummary, AgentProviderSettings, AgentProviderSummary,
};
use crate::{
    known_context_window, model_supports_reasoning, ModelCapabilityRecord, MIN_FINAL_OUTPUT_TOKENS,
    MIN_TOOL_OUTPUT_TOKENS,
};
use std::collections::HashSet;

pub fn normalized_protocol(kind: ProviderKind) -> ProviderKind {
    match kind {
        ProviderKind::DeepSeek | ProviderKind::Kimi | ProviderKind::Glm | ProviderKind::MiMo => {
            ProviderKind::Anthropic
        }
        protocol => protocol,
    }
}

pub fn provider_protocol(provider: &AgentProviderSettings) -> ProviderKind {
    normalized_protocol(provider.kind)
}

pub fn protocol_from_registry_name(value: &str) -> Option<ProviderKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "open_ai" | "openai_chat" | "openai_chat_completions" => Some(ProviderKind::OpenAi),
        "open_ai_responses" | "openai_responses" => Some(ProviderKind::OpenAiResponses),
        "open_ai_compatible" | "openai_compatible" => Some(ProviderKind::OpenAiCompatible),
        _ => None,
    }
}

pub fn apply_cached_model_capabilities(
    providers: &mut [AgentProviderSettings],
    capabilities: &[ModelCapabilityRecord],
) {
    for provider in providers {
        provider.models = normalized_model_settings(provider);
        let brand = infer_provider_brand(provider);
        for model in &mut provider.models {
            let Some(record) = crate::find_model_capability(capabilities, &model.id, Some(&brand))
            else {
                continue;
            };
            model.context_window = model.context_window.or(record.context_window);
            let registered_protocols = record
                .protocols
                .iter()
                .filter_map(|value| protocol_from_registry_name(value))
                .collect::<Vec<_>>();
            let single_protocol = (registered_protocols.len() == 1)
                .then(|| registered_protocols.first().copied())
                .flatten();
            if !registered_protocols.is_empty() {
                model.protocols = registered_protocols;
            }
            if model.protocol.is_none() {
                model.protocol = single_protocol;
            }
            model.supports_vision = model.supports_vision.or(record.supports_vision);
            model.supports_reasoning = record.supports_reasoning.or(model.supports_reasoning);
        }
    }
}

pub fn migrate_legacy_provider_protocols(providers: &mut [AgentProviderSettings]) -> bool {
    let mut migrated = false;
    for provider in providers {
        let inferred_brand = infer_provider_brand(provider);
        if provider.brand.as_deref() != Some(inferred_brand.as_str()) {
            provider.brand = Some(inferred_brand.clone());
            migrated = true;
        }
        let has_explicit_model_protocol = provider
            .models
            .iter()
            .any(|model| model.protocol.is_some() || !model.protocols.is_empty());
        let legacy_kind = match provider.kind {
            ProviderKind::DeepSeek => Some(ProviderKind::DeepSeek),
            ProviderKind::Kimi => Some(ProviderKind::Kimi),
            ProviderKind::Glm => Some(ProviderKind::Glm),
            ProviderKind::MiMo => Some(ProviderKind::MiMo),
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible
                if !has_explicit_model_protocol =>
            {
                match inferred_brand.as_str() {
                    "deepseek" => Some(ProviderKind::DeepSeek),
                    "kimi" => Some(ProviderKind::Kimi),
                    "glm" => Some(ProviderKind::Glm),
                    "mimo" => Some(ProviderKind::MiMo),
                    "qwen" | "anthropic" => Some(ProviderKind::Anthropic),
                    "openai" | "grok" => Some(ProviderKind::OpenAiResponses),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(lookup_kind) = legacy_kind else {
            continue;
        };
        let target_protocol = normalized_protocol(lookup_kind);
        if provider.kind != target_protocol {
            provider.kind = target_protocol;
            migrated = true;
        }
        if let Some(context_window) = known_context_window(lookup_kind, &provider.model) {
            if provider.context_window.is_none() {
                provider.context_window = Some(context_window);
                migrated = true;
            }
        }
        for model in &mut provider.models {
            if model.protocol.is_none() {
                model.protocol = Some(target_protocol);
                migrated = true;
            }
            if model.protocols.is_empty() {
                model.protocols = vec![target_protocol];
                migrated = true;
            }
            if let Some(context_window) = known_context_window(lookup_kind, &model.id) {
                if model.context_window.is_none() {
                    model.context_window = Some(context_window);
                    migrated = true;
                }
            }
        }
    }
    migrated
}

fn normalized_protocols(protocols: &[ProviderKind]) -> Vec<ProviderKind> {
    let mut normalized = Vec::new();
    protocols
        .iter()
        .copied()
        .map(normalized_protocol)
        .for_each(|protocol| {
            if !normalized.contains(&protocol) {
                normalized.push(protocol);
            }
        });
    normalized
}

pub fn model_protocol(
    provider: &AgentProviderSettings,
    model: &AgentModelSettings,
) -> ProviderKind {
    let explicit_protocol = model.protocol.map(normalized_protocol);
    let capabilities = normalized_protocols(&model.protocols);
    if capabilities.is_empty() {
        return explicit_protocol.unwrap_or_else(|| provider_protocol(provider));
    }
    if let Some(protocol) = explicit_protocol.filter(|protocol| capabilities.contains(protocol)) {
        return protocol;
    }
    let provider_protocol = provider_protocol(provider);
    if capabilities.contains(&provider_protocol) {
        provider_protocol
    } else {
        capabilities[0]
    }
}

pub fn protocol_label(protocol: ProviderKind) -> &'static str {
    match protocol {
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::OpenAiResponses => "OpenAI Responses",
        ProviderKind::OpenAi => "OpenAI Chat Completions",
        ProviderKind::OpenAiCompatible => "OpenAI 兼容协议",
        ProviderKind::DeepSeek => "DeepSeek",
        ProviderKind::Kimi => "Kimi",
        ProviderKind::Glm => "GLM",
        ProviderKind::MiMo => "MiMo",
    }
}

pub fn infer_provider_brand(provider: &AgentProviderSettings) -> String {
    if let Some(brand) = provider
        .brand
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return brand.trim().to_ascii_lowercase();
    }
    let model_names = provider
        .models
        .iter()
        .map(|model| format!("{} {}", model.id, model.name))
        .collect::<Vec<_>>()
        .join(" ");
    let identity = format!(
        "{} {} {} {} {}",
        provider.id, provider.name, provider.base_url, provider.model, model_names
    )
    .to_ascii_lowercase();
    if identity.contains("deepseek") {
        "deepseek".to_string()
    } else if identity.contains("moonshot") || identity.contains("kimi") {
        "kimi".to_string()
    } else if identity.contains("bigmodel")
        || identity.contains("glm")
        || identity.contains("zhipu")
        || identity.contains("智谱")
    {
        "glm".to_string()
    } else if identity.contains("qwen")
        || identity.contains("aliyun")
        || identity.contains("dashscope")
    {
        "qwen".to_string()
    } else if identity.contains("minimax") {
        "minimax".to_string()
    } else if identity.contains("mimo") || identity.contains("xiaomi") {
        "mimo".to_string()
    } else if identity.contains("gemini") || identity.contains("google") {
        "gemini".to_string()
    } else if identity.contains("grok") || identity.contains("x.ai") {
        "grok".to_string()
    } else if identity.contains("anthropic") || provider.kind == ProviderKind::Anthropic {
        "anthropic".to_string()
    } else if identity.contains("openai") || identity.contains("gpt") {
        "openai".to_string()
    } else {
        "custom".to_string()
    }
}

pub fn normalized_model_settings(provider: &AgentProviderSettings) -> Vec<AgentModelSettings> {
    let mut models = provider
        .models
        .iter()
        .filter_map(|model| {
            let id = model.id.trim();
            if id.is_empty() {
                return None;
            }
            Some(AgentModelSettings {
                reasoning_efforts: model.reasoning_efforts.clone(),
                icon_brand: model.icon_brand.clone(),
                billing: model.billing.clone(),
                id: id.to_string(),
                name: if model.name.trim().is_empty() {
                    id.to_string()
                } else {
                    model.name.trim().to_string()
                },
                protocol: model.protocol.map(normalized_protocol),
                protocols: normalized_protocols(&model.protocols),
                context_window: normalized_context_window(model.context_window),
                max_output_tokens: normalized_output_tokens(model.max_output_tokens),
                supports_tools: model.supports_tools,
                supports_vision: model.supports_vision,
                supports_usage: model.supports_usage,
                supports_reasoning: model.supports_reasoning,
            })
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        let legacy_model = provider.model.trim();
        if !legacy_model.is_empty() {
            models.push(AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: legacy_model.to_string(),
                name: legacy_model.to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: normalized_context_window(provider.context_window),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            });
        }
    } else if !provider.model.trim().is_empty()
        && !models.iter().any(|model| model.id == provider.model.trim())
    {
        models.insert(
            0,
            AgentModelSettings {
                reasoning_efforts: None,
                icon_brand: None,
                billing: None,
                id: provider.model.trim().to_string(),
                name: provider.model.trim().to_string(),
                protocol: None,
                protocols: Vec::new(),
                context_window: normalized_context_window(provider.context_window),
                max_output_tokens: None,
                supports_tools: None,
                supports_vision: None,
                supports_usage: None,
                supports_reasoning: None,
            },
        );
    }

    for model in &mut models {
        model.context_window = resolved_model_context_window(provider, model);
        model.supports_reasoning = model_supports_reasoning(
            model_protocol(provider, model),
            &model.id,
            model.supports_reasoning,
        );
    }

    models
}

pub fn sanitized_model_settings(models: Vec<AgentModelSettings>) -> Vec<AgentModelSettings> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter_map(|model| {
            let id = model.id.trim().to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            Some(AgentModelSettings {
                reasoning_efforts: model.reasoning_efforts.clone(),
                icon_brand: model.icon_brand.clone(),
                billing: model.billing.clone(),
                name: if model.name.trim().is_empty() {
                    id.clone()
                } else {
                    model.name.trim().to_string()
                },
                id,
                protocol: model.protocol.map(normalized_protocol),
                protocols: normalized_protocols(&model.protocols),
                context_window: normalized_context_window(model.context_window),
                max_output_tokens: normalized_output_tokens(model.max_output_tokens),
                supports_tools: model.supports_tools,
                supports_vision: model.supports_vision,
                supports_usage: model.supports_usage,
                supports_reasoning: model.supports_reasoning,
            })
        })
        .collect()
}

fn normalized_context_window(context_window: Option<u32>) -> Option<u32> {
    context_window.filter(|value| *value >= 1_024)
}

/// 单轮输出上限的下限。小于这个值的声明没有意义（连一次正文回复都写不完），
/// 按「未声明」处理，交回内置输出预算。
pub const MIN_DECLARED_OUTPUT_TOKENS: u32 = 256;

fn normalized_output_tokens(max_output_tokens: Option<u32>) -> Option<u32> {
    max_output_tokens.filter(|value| *value >= MIN_DECLARED_OUTPUT_TOKENS)
}

/// 把内置输出预算夹到模型声明的上限之内。
///
/// 内置预算（`max_agent_output_tokens`）按上下文窗口推算，对「输出上限远小于
/// 窗口」的模型（例如窗口 1M、单轮只能出 4K 的中转）会请求过大的 `max_tokens`，
/// 上游要么直接报错、要么静默截断。用户声明的上限只做缩小，不放大：
/// 预算本来就更小（上下文快满）时仍然以预算为准。
pub fn clamp_declared_output_tokens(requested: u32, declared: Option<u32>) -> u32 {
    match normalized_output_tokens(declared) {
        Some(limit) => requested.min(limit),
        None => requested,
    }
}

/// 设置面板「上下文窗口」输入框的默认预填值。
///
/// 它只是输入框的初始提示，不是用户声明；旧版本保存供应商时还会把它原样写进
/// provider / model 层级，所以持久化值等于它时必须视为「未声明」，让内置目录的
/// 已知窗口（例如 1M 模型）继续生效。与 src/platform/modelCapabilities.ts 的
/// PREFILLED_CONTEXT_WINDOW 保持一致。
pub const PREFILLED_CONTEXT_WINDOW: u32 = 65_536;

/// 持久化窗口是否由用户显式声明。等于面板预填值时无法与自动写入区分，按未声明处理。
fn is_declared_context_window(value: u32) -> bool {
    value != PREFILLED_CONTEXT_WINDOW
}

pub fn resolved_model_context_window(
    provider: &AgentProviderSettings,
    model: &AgentModelSettings,
) -> Option<u32> {
    let protocol = model_protocol(provider, model);
    let configured_model_count = provider
        .models
        .iter()
        .filter(|model| !model.id.trim().is_empty())
        .count();
    let applicable_provider_window = (configured_model_count <= 1
        || model.id.trim() == provider.model.trim())
    .then(|| normalized_context_window(provider.context_window))
    .flatten();
    let declared = normalized_context_window(model.context_window).or(applicable_provider_window);
    // 用户显式填写的窗口优先；未声明（或只是面板/目录预填）时以内置窗口为准，
    // 两者都没有才是真正的未知模型，交给 effective_context_window 保守估算。
    match declared {
        Some(value) if is_declared_context_window(value) => Some(value),
        _ => known_context_window(protocol, &model.id),
    }
}

pub fn expand_agent_models(providers: &[AgentProviderSettings]) -> Vec<AgentModelSummary> {
    providers
        .iter()
        .flat_map(|provider| {
            normalized_model_settings(provider)
                .into_iter()
                .map(|model| {
                    let context_window = resolved_model_context_window(provider, &model);
                    let protocol = model_protocol(provider, &model);
                    let model_id = model.id.clone();
                    let supports_reasoning =
                        model_supports_reasoning(protocol, &model_id, model.supports_reasoning);
                    AgentModelSummary {
                        name: model.name,
                        reasoning_efforts: model.reasoning_efforts,
                        icon_brand: model.icon_brand,
                        billing: model.billing,
                        id: format!("{}:{}", provider.id, model_id),
                        provider_id: provider.id.clone(),
                        provider: provider.name.clone(),
                        base_url: provider.base_url.clone(),
                        model: model_id,
                        kind: protocol,
                        protocol: Some(protocol),
                        protocols: model.protocols,
                        brand: Some(infer_provider_brand(provider)),
                        context_window,
                        supports_tools: model.supports_tools,
                        supports_vision: model.supports_vision,
                        supports_usage: model.supports_usage,
                        supports_reasoning,
                        input_price_per_million: provider.input_price_per_million,
                        output_price_per_million: provider.output_price_per_million,
                        currency: provider.currency.clone(),
                        usage_script: provider.usage_script.clone(),
                        usage_user_id: provider.usage_user_id.clone(),
                    }
                })
        })
        .collect()
}

pub fn expand_agent_providers(providers: &[AgentProviderSettings]) -> Vec<AgentProviderSummary> {
    providers
        .iter()
        .map(|provider| AgentProviderSummary {
            prompt_cache_retention: provider.prompt_cache_retention.clone(),
            id: provider.id.clone(),
            name: provider.name.clone(),
            kind: provider_protocol(provider),
            brand: Some(infer_provider_brand(provider)),
            base_url: provider.base_url.clone(),
            model: provider.model.clone(),
            models: expand_agent_models(std::slice::from_ref(provider)),
        })
        .collect()
}

pub fn max_agent_output_tokens(
    context_window: Option<u32>,
    context_tokens: usize,
    final_response_requested: bool,
) -> u32 {
    let requested = output_reserve(context_window, final_response_requested);
    let available = effective_context_window(context_window).saturating_sub(context_tokens);
    let minimum = if final_response_requested {
        MIN_FINAL_OUTPUT_TOKENS
    } else {
        MIN_TOOL_OUTPUT_TOKENS
    }
    .min(effective_context_window(context_window));
    if available < minimum {
        0
    } else {
        requested.min(available).max(minimum) as u32
    }
}

/// Output budget for a round that continues an answer cut off by the previous
/// round's output ceiling.
///
/// The final phase normally asks for `FINAL_OUTPUT_TOKENS` (2048). On protocols
/// that count reasoning tokens as output (`open_ai_responses`, `anthropic`, …) a
/// long answer that was truncated is often resumed by a model that thinks first
/// and writes second, so 2048 can be spent entirely on reasoning and the round
/// ends with zero visible text — which the turn then has to treat as an empty
/// response. A continuation only extends prose that is already under way, so it
/// is granted the tool-round allowance (`TOOL_OUTPUT_TOKENS`, 8192) instead, leaving room to
/// actually write after thinking. Never shrinks below the ordinary final budget.
pub fn max_agent_output_tokens_continued_output(
    context_window: Option<u32>,
    context_tokens: usize,
) -> u32 {
    let requested = output_reserve(context_window, false).max(output_reserve(context_window, true));
    let available = effective_context_window(context_window).saturating_sub(context_tokens);
    let minimum = MIN_FINAL_OUTPUT_TOKENS.min(effective_context_window(context_window));
    if available < minimum {
        0
    } else {
        requested.min(available).max(minimum) as u32
    }
}

pub fn resolve_agent_model<'a>(
    providers: &'a [AgentProviderSettings],
    model_id: &str,
) -> Option<(&'a AgentProviderSettings, AgentModelSettings)> {
    providers.iter().find_map(|provider| {
        normalized_model_settings(provider)
            .into_iter()
            .find(|model| format!("{}:{}", provider.id, model.id) == model_id)
            .map(|model| (provider, model))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        apply_cached_model_capabilities, clamp_declared_output_tokens, expand_agent_models,
        max_agent_output_tokens, max_agent_output_tokens_continued_output,
        migrate_legacy_provider_protocols, model_protocol, normalized_model_settings,
        normalized_protocol, provider_protocol, resolve_agent_model, resolved_model_context_window,
        sanitized_model_settings, PREFILLED_CONTEXT_WINDOW,
    };
    use crate::models::{effective_context_window, FINAL_OUTPUT_TOKENS, TOOL_OUTPUT_TOKENS};
    use crate::{AgentModelSettings, AgentProviderSettings, ProviderKind};

    fn make_provider(
        kind: ProviderKind,
        model: &str,
        context_window: Option<u32>,
        models: Vec<AgentModelSettings>,
    ) -> AgentProviderSettings {
        AgentProviderSettings {
            prompt_cache_retention: None,
            id: "provider".to_string(),
            name: "Provider".to_string(),
            kind,
            brand: None,
            base_url: "https://provider.example".to_string(),
            model: model.to_string(),
            context_window,
            input_price_per_million: None,
            output_price_per_million: None,
            currency: None,
            usage_script: None,
            usage_user_id: None,
            quota_urls: None,
            models,
        }
    }

    fn model(id: &str) -> AgentModelSettings {
        AgentModelSettings {
            reasoning_efforts: None,
            icon_brand: None,
            billing: None,
            id: id.to_string(),
            name: id.to_string(),
            protocol: None,
            protocols: Vec::new(),
            context_window: None,
            max_output_tokens: None,
            supports_tools: None,
            supports_vision: None,
            supports_usage: None,
            supports_reasoning: None,
        }
    }

    #[test]
    fn preserves_plugin_billing_icons_and_display_names() {
        let source: AgentModelSettings = serde_json::from_value(serde_json::json!({
            "id": "hy3", "name": "Hy3", "iconBrand": "workbuddy-hunyuan", "reasoningEfforts": ["low", "high"],
            "billing": { "multiplier": 0.0, "badges": ["限时免费"], "free": true }
        })).unwrap();
        let sanitized = sanitized_model_settings(vec![source]);
        let provider = make_provider(ProviderKind::OpenAiCompatible, "hy3", None, sanitized);
        let summaries = expand_agent_models(&[provider]);
        let value = serde_json::to_value(&summaries[0]).unwrap();
        assert_eq!(value["name"], "Hy3");
        assert_eq!(value["iconBrand"], "workbuddy-hunyuan");
        assert_eq!(
            value["reasoningEfforts"],
            serde_json::json!(["low", "high"])
        );
        assert_eq!(value["billing"]["multiplier"], 0.0);
        assert_eq!(value["billing"]["badges"][0], "限时免费");
        assert_eq!(value["billing"]["free"], true);
    }

    #[test]
    fn clamps_declared_output_tokens_without_ever_raising_the_budget() {
        // 没声明：内置预算原样通过。
        assert_eq!(clamp_declared_output_tokens(8_192, None), 8_192);
        // 声明值比预算小：按声明夹住（窗口大但单轮只能出几 K 的中转）。
        assert_eq!(clamp_declared_output_tokens(8_192, Some(4_096)), 4_096);
        // 声明值比预算大：不放大，预算本来就考虑过上下文占用。
        assert_eq!(clamp_declared_output_tokens(2_048, Some(16_384)), 2_048);
        // 低于下限的声明按「未声明」处理，避免把输出压成 0。
        assert_eq!(clamp_declared_output_tokens(8_192, Some(64)), 8_192);
    }

    #[test]
    fn normalizes_declared_output_tokens_against_the_floor() {
        let declared = AgentModelSettings {
            max_output_tokens: Some(4_096),
            ..model("declared")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "declared",
            None,
            vec![declared],
        );
        assert_eq!(
            normalized_model_settings(&provider)[0].max_output_tokens,
            Some(4_096)
        );

        let too_small = AgentModelSettings {
            max_output_tokens: Some(32),
            ..model("tiny")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "tiny",
            None,
            vec![too_small],
        );
        assert_eq!(
            normalized_model_settings(&provider)[0].max_output_tokens,
            None
        );
    }

    #[test]
    fn resolve_agent_model_carries_the_declared_output_limit() {
        let declared = AgentModelSettings {
            max_output_tokens: Some(4_096),
            ..model("limited")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "limited",
            None,
            vec![declared],
        );
        let (_, selected) =
            resolve_agent_model(std::slice::from_ref(&provider), "provider:limited")
                .expect("model should resolve");
        assert_eq!(selected.max_output_tokens, Some(4_096));
    }

    #[test]
    fn declared_output_limit_round_trips_as_camel_case() {
        let declared = AgentModelSettings {
            max_output_tokens: Some(8_192),
            ..model("camel")
        };
        assert_eq!(
            serde_json::to_value(&declared).unwrap()["maxOutputTokens"],
            8_192
        );
        // 未声明时不写这个键，旧配置文件不受影响。
        assert!(serde_json::to_value(model("plain"))
            .unwrap()
            .get("maxOutputTokens")
            .is_none());
        let parsed: AgentModelSettings = serde_json::from_value(serde_json::json!({
            "id": "camel", "name": "Camel", "maxOutputTokens": 2048
        }))
        .expect("optional fields must default");
        assert_eq!(parsed.max_output_tokens, Some(2_048));
    }

    #[test]
    fn cached_capabilities_fill_unknown_models_without_overriding_explicit_provider_data() {
        let capabilities = crate::bundled_model_capabilities();
        let mut provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "glm-5.3",
            None,
            vec![
                AgentModelSettings {
                    context_window: Some(128_000),
                    ..model("glm-5.3")
                },
                AgentModelSettings {
                    context_window: Some(256_000),
                    supports_vision: Some(false),
                    ..model("glm-5.3-flash")
                },
                model("glm-5.3-flash"),
            ],
        );
        apply_cached_model_capabilities(std::slice::from_mut(&mut provider), capabilities);
        assert_eq!(provider.models[0].context_window, Some(128_000));
        assert_eq!(provider.models[1].context_window, Some(256_000));
        assert_eq!(provider.models[2].context_window, Some(1_000_000));
        assert_eq!(provider.models[0].supports_vision, Some(false));
        assert_eq!(provider.models[1].supports_vision, Some(false));
        assert_eq!(provider.models[2].supports_vision, Some(true));
    }

    #[test]
    fn normalizes_legacy_protocols_and_deduplicates_capabilities() {
        let source = model("kimi-k3");
        let mut source = AgentModelSettings {
            protocol: Some(ProviderKind::Kimi),
            protocols: vec![
                ProviderKind::Kimi,
                ProviderKind::Anthropic,
                ProviderKind::MiMo,
                ProviderKind::OpenAi,
                ProviderKind::OpenAi,
            ],
            ..source
        };
        let provider = make_provider(ProviderKind::Kimi, "kimi-k3", None, vec![source.clone()]);

        assert_eq!(
            normalized_protocol(ProviderKind::Kimi),
            ProviderKind::Anthropic
        );
        assert_eq!(provider_protocol(&provider), ProviderKind::Anthropic);

        let normalized = normalized_model_settings(&provider);
        assert_eq!(
            normalized[0].protocols,
            vec![ProviderKind::Anthropic, ProviderKind::OpenAi,]
        );
        assert_eq!(normalized[0].protocol, Some(ProviderKind::Anthropic));

        source.protocol = Some(ProviderKind::Glm);
        let sanitized = sanitized_model_settings(vec![source]);
        assert_eq!(sanitized[0].protocol, Some(ProviderKind::Anthropic));
        assert_eq!(sanitized[0].protocols[0], ProviderKind::Anthropic);
    }

    #[test]
    fn selects_only_a_protocol_declared_by_model_capabilities() {
        let provider = make_provider(ProviderKind::OpenAi, "model", None, Vec::new());
        let mut model = model("model");
        model.protocol = Some(ProviderKind::OpenAiResponses);
        model.protocols = vec![ProviderKind::Anthropic, ProviderKind::OpenAi];

        assert_eq!(model_protocol(&provider, &model), ProviderKind::OpenAi);

        model.protocol = None;
        assert_eq!(model_protocol(&provider, &model), ProviderKind::OpenAi);

        let compatible_provider =
            make_provider(ProviderKind::OpenAiCompatible, "model", None, Vec::new());
        model.protocol = Some(ProviderKind::OpenAiResponses);
        model.protocols = vec![ProviderKind::Anthropic, ProviderKind::OpenAiResponses];
        assert_eq!(
            model_protocol(&compatible_provider, &model),
            ProviderKind::OpenAiResponses
        );
    }

    #[test]
    fn routes_models_within_one_provider_to_their_declared_wire_protocol() {
        use crate::{
            messages_endpoint, request_body, AgentMessage, ProviderConfig, ProviderRequest,
        };
        let models = [
            (
                "glm-via-completions",
                ProviderKind::OpenAiCompatible,
                "/v1/chat/completions",
            ),
            (
                "responses-model",
                ProviderKind::OpenAiResponses,
                "/v1/responses",
            ),
            ("anthropic-model", ProviderKind::Anthropic, "/v1/messages"),
        ]
        .into_iter()
        .map(|(id, protocol, _)| AgentModelSettings {
            protocol: Some(protocol),
            protocols: vec![protocol],
            ..model(id)
        })
        .collect();
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "glm-via-completions",
            None,
            models,
        );
        for (selected, (_, protocol, suffix)) in provider.models.iter().zip([
            (
                "glm-via-completions",
                ProviderKind::OpenAiCompatible,
                "/v1/chat/completions",
            ),
            (
                "responses-model",
                ProviderKind::OpenAiResponses,
                "/v1/responses",
            ),
            ("anthropic-model", ProviderKind::Anthropic, "/v1/messages"),
        ]) {
            let resolved = model_protocol(&provider, selected);
            assert_eq!(resolved, protocol);
            assert!(messages_endpoint(resolved, &provider.base_url).ends_with(suffix));
            let config = ProviderConfig::new(
                &provider.id,
                &provider.name,
                resolved,
                &provider.base_url,
                "test-key-ref",
            )
            .unwrap();
            let request = ProviderRequest::new(
                "turn",
                &selected.id,
                vec![AgentMessage::user("hello")],
                vec![],
            )
            .with_system_prompt("system instruction")
            .with_max_tokens(1024);
            let body = request_body(&config, &request).unwrap();
            match resolved {
                ProviderKind::OpenAiResponses => {
                    assert!(body["input"].is_array());
                    assert!(body.get("messages").is_none());
                }
                ProviderKind::Anthropic => {
                    assert!(body["messages"].is_array());
                    assert!(body["system"].is_array());
                    assert_eq!(body["max_tokens"], 1024);
                }
                ProviderKind::OpenAiCompatible => {
                    assert!(body["messages"].is_array());
                    assert_eq!(body["messages"][0]["role"], "system");
                    assert!(body.get("input").is_none());
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn normalizes_context_windows_without_inventing_unknown_values() {
        let selected = model("custom-model");
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "custom-model",
            Some(64_000),
            vec![selected.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider, &selected),
            Some(64_000)
        );

        let small_provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "custom-model",
            Some(1_023),
            vec![selected.clone()],
        );
        let small_model = AgentModelSettings {
            context_window: Some(1_023),
            ..selected.clone()
        };
        assert_eq!(
            resolved_model_context_window(&small_provider, &small_model),
            None
        );
        assert_eq!(
            normalized_model_settings(&small_provider)[0].context_window,
            None
        );

        let unknown_provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "known-model",
            Some(128_000),
            vec![model("known-model"), model("unknown-model")],
        );
        let unknown = model("unknown-model");
        assert_eq!(
            resolved_model_context_window(&unknown_provider, &unknown),
            None
        );
    }

    #[test]
    fn explicit_provider_window_precedes_registered_model_window() {
        let registered = model("qwen3-max");
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "qwen3-max",
            Some(32_000),
            vec![registered.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider, &registered),
            Some(32_000)
        );

        let summaries = expand_agent_models(&[provider]);
        assert_eq!(summaries[0].context_window, Some(32_000));
    }

    #[test]
    fn explicit_model_window_precedes_provider_and_registry_windows() {
        let configured = AgentModelSettings {
            context_window: Some(24_000),
            ..model("qwen3-max")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "qwen3-max",
            Some(32_000),
            vec![configured.clone()],
        );

        assert_eq!(
            resolved_model_context_window(&provider, &configured),
            Some(24_000)
        );
    }

    #[test]
    fn legacy_migration_does_not_overwrite_explicit_context_windows() {
        let configured = AgentModelSettings {
            context_window: Some(24_000),
            ..model("grok-4")
        };
        let mut provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "grok-4",
            Some(32_000),
            vec![configured],
        );
        provider.brand = Some("grok".to_string());

        assert!(migrate_legacy_provider_protocols(std::slice::from_mut(
            &mut provider,
        )));
        assert_eq!(provider.context_window, Some(32_000));
        assert_eq!(provider.models[0].context_window, Some(24_000));
    }

    #[test]
    fn known_reasoning_model_overrides_stale_false_capability() {
        let mut configured = model("gpt-5.6-luna");
        configured.supports_reasoning = Some(false);
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "gpt-5.6-luna",
            Some(1_100_000),
            vec![configured],
        );

        let summaries = expand_agent_models(&[provider]);
        assert_eq!(summaries[0].supports_reasoning, Some(true));
    }

    #[test]
    fn normalized_settings_expose_reasoning_for_current_and_legacy_gpt_models() {
        for explicit in [None, Some(false), Some(true)] {
            let mut configured = model("gpt-6.1-sol");
            configured.supports_reasoning = explicit;
            let provider = make_provider(
                ProviderKind::OpenAiResponses,
                "gpt-6.1-sol",
                None,
                vec![configured],
            );
            let normalized = normalized_model_settings(&provider);
            assert_eq!(normalized[0].id, "gpt-6.1-sol");
            assert_eq!(normalized[0].supports_reasoning, Some(true));
            assert_eq!(
                expand_agent_models(&[provider])[0].supports_reasoning,
                normalized[0].supports_reasoning
            );
        }
        let legacy = make_provider(
            ProviderKind::OpenAiResponses,
            "gpt-6.1-sol",
            None,
            Vec::new(),
        );
        assert_eq!(
            normalized_model_settings(&legacy)[0].supports_reasoning,
            Some(true)
        );
    }

    #[test]
    fn normalized_settings_honor_custom_model_capabilities_and_selected_protocol() {
        for explicit in [None, Some(false), Some(true)] {
            let mut configured = model("custom-model");
            configured.supports_reasoning = explicit;
            let provider = make_provider(
                ProviderKind::OpenAiCompatible,
                "custom-model",
                None,
                vec![configured],
            );
            assert_eq!(
                normalized_model_settings(&provider)[0].supports_reasoning,
                explicit
            );
        }
        let mut configured = model("gpt-6.1-sol");
        configured.protocol = Some(ProviderKind::OpenAiResponses);
        configured.protocols = vec![ProviderKind::OpenAiResponses];
        let provider = make_provider(
            ProviderKind::Anthropic,
            "gpt-6.1-sol",
            None,
            vec![configured],
        );
        assert_eq!(
            normalized_model_settings(&provider)[0].supports_reasoning,
            Some(true)
        );
    }

    #[test]
    fn migrates_legacy_grok_provider_to_responses_without_losing_model_capability() {
        let mut provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "grok-4",
            None,
            vec![model("grok-4")],
        );
        provider.brand = Some("grok".to_string());

        assert!(migrate_legacy_provider_protocols(std::slice::from_mut(
            &mut provider,
        )));
        assert_eq!(provider.kind, ProviderKind::OpenAiResponses);
        assert_eq!(
            provider.models[0].protocol,
            Some(ProviderKind::OpenAiResponses)
        );
        assert_eq!(
            provider.models[0].protocols,
            vec![ProviderKind::OpenAiResponses]
        );
    }

    #[test]
    fn prefilled_context_window_never_shadows_the_known_model_window() {
        // 设置面板的预填值被保存到 model 层级后，1M 模型不能退化成一个 64K 预算。
        let prefilled = AgentModelSettings {
            context_window: Some(PREFILLED_CONTEXT_WINDOW),
            ..model("deepseek-v4-pro")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "deepseek-v4-pro",
            None,
            vec![prefilled.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider, &prefilled),
            Some(1_000_000)
        );
        assert_eq!(
            expand_agent_models(&[provider])[0].context_window,
            Some(1_000_000)
        );

        // 预填值同样可能被写到 provider 层级（单模型供应商）。
        let configured = model("deepseek-v4-pro");
        let provider_level = make_provider(
            ProviderKind::OpenAiCompatible,
            "deepseek-v4-pro",
            Some(PREFILLED_CONTEXT_WINDOW),
            vec![configured.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider_level, &configured),
            Some(1_000_000)
        );
    }

    #[test]
    fn manually_declared_window_still_overrides_the_known_window() {
        let declared = AgentModelSettings {
            context_window: Some(32_000),
            ..model("deepseek-v4-pro")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "deepseek-v4-pro",
            None,
            vec![declared.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider, &declared),
            Some(32_000)
        );
        assert_eq!(
            expand_agent_models(&[provider])[0].context_window,
            Some(32_000)
        );
    }

    #[test]
    fn prefilled_window_on_unknown_model_falls_back_to_the_conservative_budget() {
        let prefilled = AgentModelSettings {
            context_window: Some(PREFILLED_CONTEXT_WINDOW),
            ..model("my-custom-model")
        };
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "my-custom-model",
            None,
            vec![prefilled.clone()],
        );
        let resolved = resolved_model_context_window(&provider, &prefilled);
        assert_eq!(resolved, None);
        assert_eq!(effective_context_window(resolved), 64_000);
    }

    #[test]
    fn known_window_equal_to_the_prefill_value_is_preserved() {
        // 目录里确实存在 65,536 窗口的模型，预填过滤不能把它们也抹掉。
        let configured = model("gemini-3-pro-image");
        let provider = make_provider(
            ProviderKind::OpenAiCompatible,
            "gemini-3-pro-image",
            None,
            vec![configured.clone()],
        );
        assert_eq!(
            resolved_model_context_window(&provider, &configured),
            Some(PREFILLED_CONTEXT_WINDOW)
        );
        let prefilled = AgentModelSettings {
            context_window: Some(PREFILLED_CONTEXT_WINDOW),
            ..configured
        };
        assert_eq!(
            resolved_model_context_window(&provider, &prefilled),
            Some(PREFILLED_CONTEXT_WINDOW)
        );
    }

    #[test]
    fn continued_output_is_allowed_the_tool_round_allowance() {
        // 续写被截断的长回答时，收尾阶段那 2048 的默认额度可能被整轮思考吃光，
        // 正文一字未出（见 `turn_recovery` 的 thinking_only 分支）。
        let window = Some(128_000);
        assert_eq!(
            max_agent_output_tokens(window, 20_000, true),
            FINAL_OUTPUT_TOKENS as u32
        );
        assert_eq!(
            max_agent_output_tokens_continued_output(window, 20_000),
            TOOL_OUTPUT_TOKENS as u32
        );
        // 剩余上下文不足时仍夹到可用空间，且不会低于收尾阶段的下限。
        assert_eq!(
            max_agent_output_tokens_continued_output(window, 120_000),
            8_000
        );
        // 窗口未知时按默认窗口算；已经占满就不给额度。
        assert_eq!(
            max_agent_output_tokens_continued_output(None, effective_context_window(None)),
            0
        );
    }
}
