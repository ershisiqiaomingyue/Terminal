use crate::ProviderKind;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const MODEL_CAPABILITY_REQUEST_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilityDocument {
    pub version: u32,
    #[serde(default)]
    pub models: Vec<ModelCapabilityRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilityRecord {
    pub model: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(rename = "contextWindow", default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(rename = "supportsReasoning", default)]
    pub supports_reasoning: Option<bool>,
    #[serde(rename = "supportsVision", default)]
    pub supports_vision: Option<bool>,
    #[serde(rename = "reasoningEfforts", default)]
    pub reasoning_efforts: Vec<String>,
    #[serde(rename = "reasoningBudget", default)]
    pub reasoning_budget: bool,
    #[serde(default)]
    pub source: Option<String>,
}

pub fn read_model_capabilities(path: &Path) -> Vec<ModelCapabilityRecord> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<ModelCapabilityDocument>(&content)
        .ok()
        .filter(|document| document.version > 0)
        .map(|document| document.models)
        .unwrap_or_default()
}

pub fn write_model_capabilities(
    path: &Path,
    document: &ModelCapabilityDocument,
) -> Result<(), String> {
    let content = serde_json::to_string_pretty(document)
        .map_err(|error| format!("cannot serialize model capability document: {error}"))?;
    fs::write(path, content)
        .map_err(|error| format!("cannot write model capability document: {error}"))
}

pub async fn fetch_model_capabilities(
    client: &Client,
    url: &str,
) -> Result<ModelCapabilityDocument, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("model capability request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "model capability request returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("model capability document is invalid: {error}"))?;
    let document = parse_model_capability_document(payload)
        .ok_or_else(|| "model capability document is empty or has an invalid format".to_string())?;
    if document.version == 0 || document.models.is_empty() {
        return Err("model capability document is empty or has an invalid version".to_string());
    }
    Ok(document)
}

/// Accept the small native registry format as well as the public models.dev
/// catalog. The latter is intentionally reduced to recent flagship families
/// so a provider picker never receives thousands of unrelated legacy entries.
pub fn parse_model_capability_document(
    payload: serde_json::Value,
) -> Option<ModelCapabilityDocument> {
    if let Ok(document) = serde_json::from_value::<ModelCapabilityDocument>(payload.clone()) {
        if document.version > 0 && !document.models.is_empty() {
            return Some(document);
        }
    }

    let providers = payload.as_object()?;
    let mut models: BTreeMap<String, (u8, ModelCapabilityRecord)> = BTreeMap::new();
    for (provider_id, provider) in providers {
        let Some(provider_models) = provider
            .get("models")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        for model in provider_models.values() {
            let Some(raw_id) = model.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let id = raw_id.trim().to_ascii_lowercase();
            let basename = id.rsplit('/').next().unwrap_or(&id).to_string();
            let Some(brand) = catalog_model_brand(&basename) else {
                continue;
            };
            if !is_recent_catalog_model(&basename)
                || !recent_release(
                    model
                        .get("release_date")
                        .and_then(serde_json::Value::as_str),
                )
            {
                continue;
            }
            let context_window = model
                .pointer("/limit/context")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value >= 1_024);
            let Some(context_window) = context_window else {
                continue;
            };
            let supports_reasoning = model.get("reasoning").and_then(serde_json::Value::as_bool);
            let supports_vision = model
                .pointer("/modalities/input")
                .and_then(serde_json::Value::as_array)
                .map(|modalities| {
                    modalities
                        .iter()
                        .any(|modality| modality.as_str() == Some("image"))
                });
            let options = model
                .get("reasoning_options")
                .and_then(serde_json::Value::as_array);
            let reasoning_efforts = options
                .into_iter()
                .flatten()
                .filter(|option| {
                    option.get("type").and_then(serde_json::Value::as_str) == Some("effort")
                })
                .flat_map(|option| {
                    option
                        .get("values")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(serde_json::Value::as_str)
                .filter(|value| {
                    matches!(
                        *value,
                        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                    )
                })
                .map(str::to_string)
                .collect();
            let reasoning_budget = options.into_iter().flatten().any(|option| {
                option.get("type").and_then(serde_json::Value::as_str) == Some("budget_tokens")
            });
            let record = ModelCapabilityRecord {
                model: id.clone(),
                provider: None,
                context_window: Some(context_window),
                protocols: Vec::new(),
                supports_reasoning,
                supports_vision,
                reasoning_efforts,
                reasoning_budget,
                source: Some(format!(
                    "https://models.dev/api.json#{provider_id}/{raw_id}"
                )),
            };
            let primary = matches!(
                (Some(brand.as_str()), provider_id.as_str()),
                (Some("openai"), "openai")
                    | (Some("anthropic"), "anthropic")
                    | (Some("deepseek"), "deepseek")
                    | (Some("kimi"), "moonshotai")
                    | (Some("glm"), "zai")
                    | (Some("minimax"), "minimax")
                    | (Some("grok"), "xai")
                    | (Some("qwen"), "alibaba")
                    | (Some("mimo"), "xiaomi")
                    | (Some("gemini"), "google")
            );
            if primary || provider_id == "openrouter" {
                insert_catalog_record(&mut models, id, if primary { 0 } else { 1 }, record.clone());
            }
            // An unqualified fallback uses the conservative minimum among
            // gateways, while a manufacturer's entry always has priority.
            if primary
                || provider_id == "openrouter"
                || provider_id.starts_with(&brand)
                || (brand == "qwen" && provider_id.starts_with("alibaba"))
            {
                insert_catalog_record(&mut models, basename, if primary { 0 } else { 2 }, record);
            }
        }
    }

    if models.is_empty() {
        return None;
    }
    Some(ModelCapabilityDocument {
        version: 2,
        models: models.into_values().map(|(_, record)| record).collect(),
    })
}

fn insert_catalog_record(
    models: &mut BTreeMap<String, (u8, ModelCapabilityRecord)>,
    id: String,
    priority: u8,
    mut record: ModelCapabilityRecord,
) {
    if let Some((previous_priority, previous)) = models.get(&id) {
        if *previous_priority < priority
            || (*previous_priority == priority && previous.context_window <= record.context_window)
        {
            return;
        }
    }
    record.model = id.clone();
    models.insert(id, (priority, record));
}

fn recent_release(release: Option<&str>) -> bool {
    let Some(release) = release else {
        return false;
    };
    let normalized = match release.len() {
        4 => format!("{release}-01-01"),
        7 => format!("{release}-01"),
        _ => release.to_string(),
    };
    let Ok(date) = chrono::NaiveDate::parse_from_str(&normalized, "%Y-%m-%d") else {
        return false;
    };
    let today = chrono::Utc::now().date_naive();
    date <= today && date >= today - chrono::Duration::days(366)
}

fn catalog_model_brand(model_id: &str) -> Option<String> {
    let brand = if model_id.starts_with("deepseek-") {
        "deepseek"
    } else if model_id.starts_with("claude-") {
        "anthropic"
    } else if model_id.starts_with("gpt-") {
        "openai"
    } else if model_id.starts_with("qwen") {
        "qwen"
    } else if model_id.starts_with("kimi-") || model_id.starts_with("moonshot-") {
        "kimi"
    } else if model_id.starts_with("glm-") {
        "glm"
    } else if model_id.starts_with("minimax-") {
        "minimax"
    } else if model_id.starts_with("grok-") {
        "grok"
    } else if model_id.starts_with("mimo-") {
        "mimo"
    } else if model_id.starts_with("gemini-") {
        "gemini"
    } else {
        return None;
    };
    Some(brand.to_string())
}

fn is_recent_catalog_model(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    if id.starts_with("deepseek-v4")
        || matches!(
            id.as_str(),
            "deepseek-flash" | "deepseek-pro" | "deepseek-chat" | "deepseek-reasoner"
        )
    {
        return true;
    }
    if id.starts_with("claude-") && !id.starts_with("claude-3") {
        return true;
    }
    if let Some(version) = id.strip_prefix("gpt-") {
        return version.starts_with('6')
            || version.starts_with("5.5")
            || version.starts_with("5.6")
            || version.starts_with("5.7")
            || version.starts_with("5.8")
            || version.starts_with("5.9");
    }
    if id.starts_with("qwen3") || id.starts_with("qwen4") {
        return true;
    }
    if id.starts_with("kimi-k2") || id.starts_with("kimi-k3") {
        return true;
    }
    if id.starts_with("glm-4.7") || id.starts_with("glm-5") {
        return true;
    }
    if id.starts_with("minimax-m2") || id.starts_with("minimax-m3") {
        return true;
    }
    if id.starts_with("grok-4") || id.starts_with("grok-5") || id.starts_with("grok-code-fast") {
        return true;
    }
    id.starts_with("mimo-v2") || id.starts_with("gemini-3")
}

pub fn model_capability_http_client() -> Result<Client, String> {
    crate::network::http_client_builder()?
        .timeout(std::time::Duration::from_secs(
            MODEL_CAPABILITY_REQUEST_TIMEOUT_SECS,
        ))
        .build()
        .map_err(|error| format!("cannot create model capability client: {error}"))
}

pub fn known_context_window(kind: ProviderKind, model_id: &str) -> Option<u32> {
    if let Some(window) = find_model_capability(bundled_model_capabilities(), model_id, None)
        .and_then(|record| record.context_window)
    {
        return Some(window);
    }
    let model_id = normalize_model_id(model_id)?;
    if model_id.is_empty() {
        return None;
    }
    if let Some(window) = explicit_context_window(&model_id) {
        return (window >= 1_024).then_some(window);
    }

    let provider_window = match kind {
        ProviderKind::DeepSeek => deepseek_context_window(&model_id),
        ProviderKind::Kimi => kimi_context_window(&model_id),
        ProviderKind::OpenAi | ProviderKind::OpenAiResponses => openai_context_window(&model_id),
        ProviderKind::Anthropic => anthropic_context_window(&model_id),
        ProviderKind::Glm => glm_context_window(&model_id),
        ProviderKind::MiMo => mimo_context_window(&model_id),
        ProviderKind::OpenAiCompatible => None,
    };
    provider_window.or_else(|| generic_context_window(&model_id))
}

/// Return a native reasoning capability only when the model identity is known
/// to expose one. Unknown models stay unresolved so protocol-specific fields
/// are not sent blindly to custom gateways.
pub fn known_reasoning_support(kind: ProviderKind, model_id: &str) -> Option<bool> {
    if let Some(support) = find_model_capability(bundled_model_capabilities(), model_id, None)
        .and_then(|record| record.supports_reasoning)
    {
        return Some(support);
    }
    let model_id = normalize_model_id(model_id)?;
    if model_id.is_empty() {
        return None;
    }
    let supported = match kind {
        ProviderKind::OpenAi | ProviderKind::OpenAiResponses | ProviderKind::OpenAiCompatible => {
            known_model(
                &model_id,
                &[
                    "o1",
                    "o1-mini",
                    "o3",
                    "o3-mini",
                    "o4-mini",
                    "gpt-5",
                    "gpt-5.1",
                    "gpt-5.2",
                    "gpt-5.6",
                    "gpt-5.6-luna",
                    "gpt-5.6-sol",
                    "gpt-5.6-terra",
                    "gpt-6.1-sol",
                    "grok-3-mini",
                    "grok-4.1-fast-reasoning",
                ],
            )
        }
        ProviderKind::Anthropic => known_model(
            &model_id,
            &[
                "claude-3-7-sonnet",
                "claude-3.7-sonnet",
                "claude-opus-4",
                "claude-opus-4-1",
                "claude-opus-4-8",
                "claude-sonnet-4",
                "claude-sonnet-4-5",
                "claude-haiku-4-5",
                "claude-fable-5",
                "claude-sonnet-5",
            ],
        ),
        _ => model_id.contains("reason") || model_id.contains("thinking"),
    };
    supported.then_some(true)
}

pub fn model_supports_reasoning(
    kind: ProviderKind,
    model_id: &str,
    explicit: Option<bool>,
) -> Option<bool> {
    // A known reasoning model must win over stale capability data persisted by
    // an older model discovery response. Unknown/custom models still honor an
    // explicit provider declaration.
    known_reasoning_support(kind, model_id).or(explicit)
}

fn explicit_context_window(model_id: &str) -> Option<u32> {
    let start = model_id.rfind('[')?;
    if !model_id.ends_with(']') || start == 0 {
        return None;
    }
    let value = &model_id[start + 1..model_id.len() - 1];
    let (number, multiplier) = if let Some(number) = value.strip_suffix('m') {
        (number, 1_000_000u64)
    } else if let Some(number) = value.strip_suffix('k') {
        (number, 1_000u64)
    } else {
        return None;
    };
    let number = number.parse::<u64>().ok()?;
    u32::try_from(number.checked_mul(multiplier)?).ok()
}

fn normalize_model_id(model_id: &str) -> Option<String> {
    let model_id = model_id.trim().to_ascii_lowercase();
    if model_id.is_empty() {
        return None;
    }
    Some(
        model_id
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
    )
}

pub fn bundled_model_capabilities() -> &'static [ModelCapabilityRecord] {
    static MODELS: std::sync::OnceLock<Vec<ModelCapabilityRecord>> = std::sync::OnceLock::new();
    MODELS.get_or_init(|| {
        serde_json::from_str::<ModelCapabilityDocument>(include_str!("model-capabilities.json"))
            .map(|document| document.models)
            .unwrap_or_default()
    })
}

pub fn find_model_capability<'a>(
    records: &'a [ModelCapabilityRecord],
    model_id: &str,
    brand: Option<&str>,
) -> Option<&'a ModelCapabilityRecord> {
    let id = model_id.trim().to_ascii_lowercase();
    let eligible = |record: &&ModelCapabilityRecord| {
        record
            .provider
            .as_deref()
            .map(|provider| brand.is_some_and(|brand| provider.eq_ignore_ascii_case(brand)))
            .unwrap_or(true)
    };
    if let Some(record) = records
        .iter()
        .filter(eligible)
        .find(|record| record.model.eq_ignore_ascii_case(&id))
    {
        return Some(record);
    }
    let basename = normalize_model_id(&id)?;
    records.iter().filter(eligible).find(|record| {
        !record.model.contains('/')
            && (record.model.eq_ignore_ascii_case(&basename)
                || known_model(&basename, &[record.model.as_str()]))
    })
}

pub fn selectable_reasoning_efforts(
    record: &ModelCapabilityRecord,
    kind: ProviderKind,
) -> Vec<String> {
    if record.supports_reasoning != Some(true) {
        return Vec::new();
    }
    if !record.reasoning_efforts.is_empty() {
        return record
            .reasoning_efforts
            .iter()
            .filter(|effort| matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max"))
            .cloned()
            .collect();
    }
    // Budget-based thinking is supported by our Anthropic request adapter.
    if kind == ProviderKind::Anthropic && record.reasoning_budget {
        return ["low", "medium", "high", "max"]
            .map(str::to_string)
            .to_vec();
    }
    Vec::new()
}

fn known_model(model_id: &str, base_ids: &[&str]) -> bool {
    base_ids.iter().any(|base_id| {
        if model_id == *base_id {
            return true;
        }
        let Some(suffix) = model_id
            .strip_prefix(base_id)
            .and_then(|value| value.strip_prefix('-').or_else(|| value.strip_prefix(':')))
        else {
            return false;
        };
        suffix == "latest"
            || suffix == "preview"
            || suffix == "free"
            || suffix == "online"
            || is_date_variant(suffix)
            || (suffix.starts_with("preview-") && is_preview_date_variant(suffix))
    })
}

fn is_date_variant(value: &str) -> bool {
    value.is_ascii()
        && ((value.len() == 8 && value.chars().all(|character| character.is_ascii_digit()))
            || (value.len() == 4
                && value.chars().all(|character| character.is_ascii_digit())
                && value[..2]
                    .parse::<u8>()
                    .map(|month| (1..=12).contains(&month))
                    .unwrap_or(false)
                && value[2..]
                    .parse::<u8>()
                    .map(|day| (1..=31).contains(&day))
                    .unwrap_or(false))
            || (value.len() == 10
                && value.as_bytes()[4] == b'-'
                && value.as_bytes()[7] == b'-'
                && value[..4].parse::<u16>().is_ok()
                && value[5..7]
                    .parse::<u8>()
                    .map(|month| (1..=12).contains(&month))
                    .unwrap_or(false)
                && value[8..]
                    .parse::<u8>()
                    .map(|day| (1..=31).contains(&day))
                    .unwrap_or(false)))
}

fn is_preview_date_variant(value: &str) -> bool {
    let Some(date) = value.strip_prefix("preview-") else {
        return false;
    };
    date.is_ascii()
        && date.len() == 5
        && date.as_bytes()[2] == b'-'
        && date[..2]
            .parse::<u8>()
            .map(|month| (1..=12).contains(&month))
            .unwrap_or(false)
        && date[3..]
            .parse::<u8>()
            .map(|day| (1..=31).contains(&day))
            .unwrap_or(false)
}

fn window_for(model_id: &str, base_ids: &[&str], window: u32) -> Option<u32> {
    known_model(model_id, base_ids).then_some(window)
}

fn deepseek_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &[
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-chat",
            "deepseek-reasoner",
        ],
        1_000_000,
    )
}

fn kimi_context_window(model_id: &str) -> Option<u32> {
    window_for(model_id, &["kimi-k3"], 1_000_000)
        .or_else(|| {
            window_for(
                model_id,
                &[
                    "kimi-k2.7-code",
                    "kimi-k2.7-code-highspeed",
                    "kimi-k2.6",
                    "kimi-k2.5",
                ],
                256_000,
            )
        })
        .or_else(|| match model_id {
            "moonshot-v1-8k" => Some(8_192),
            "moonshot-v1-32k" => Some(32_768),
            "moonshot-v1-128k" => Some(131_072),
            _ => None,
        })
}

fn anthropic_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &["claude-fable-5", "claude-opus-4-8", "claude-sonnet-5"],
        1_000_000,
    )
    .or_else(|| {
        window_for(
            model_id,
            &[
                "claude-3-opus",
                "claude-3-sonnet",
                "claude-3-haiku",
                "claude-3.5-sonnet",
                "claude-3.5-haiku",
                "claude-3-5-sonnet",
                "claude-3-5-haiku",
                "claude-3-7-sonnet",
                "claude-opus-4",
                "claude-opus-4-1",
                "claude-sonnet-4",
                "claude-sonnet-4-5",
                "claude-haiku-4-5",
            ],
            200_000,
        )
    })
}

fn openai_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-luna", "gpt-5.6-terra"],
        1_050_000,
    )
    .or_else(|| {
        window_for(
            model_id,
            &["gpt-5", "gpt-5.1", "gpt-5.2", "gpt-5-mini", "gpt-5-nano"],
            400_000,
        )
    })
    .or_else(|| {
        window_for(
            model_id,
            &["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano"],
            1_047_576,
        )
    })
    .or_else(|| window_for(model_id, &["o3", "o3-mini", "o4-mini"], 200_000))
    .or_else(|| {
        window_for(
            model_id,
            &["gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "gpt-4.5-preview"],
            128_000,
        )
    })
}

fn glm_context_window(model_id: &str) -> Option<u32> {
    window_for(model_id, &["glm-4.7", "glm-4.7-flashx", "glm-4.6"], 200_000).or_else(|| {
        window_for(
            model_id,
            &[
                "glm-4.5",
                "glm-4.5-air",
                "glm-4.5-x",
                "glm-4.5-airx",
                "glm-4.5-flash",
                "glm-4",
                "glm-4-plus",
                "glm-4-air",
                "glm-4-flash",
            ],
            128_000,
        )
    })
}

fn mimo_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &["mimo-v2.5", "mimo-v2.5-pro", "mimo-v2.5-pro-ultraspeed"],
        1_000_000,
    )
}

fn qwen_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &["qwen3-coder-plus", "qwen3-coder-flash"],
        1_000_000,
    )
    .or_else(|| window_for(model_id, &["qwen3-max", "qwen3-coder-next"], 262_144))
    .or_else(|| {
        window_for(
            model_id,
            &[
                "qwen2.5-coder-32b-instruct",
                "qwen2.5-72b-instruct",
                "qwq-32b",
            ],
            131_072,
        )
    })
}

fn grok_context_window(model_id: &str) -> Option<u32> {
    window_for(
        model_id,
        &["grok-4.1-fast-reasoning", "grok-4.1-fast-non-reasoning"],
        2_000_000,
    )
    .or_else(|| {
        window_for(
            model_id,
            &["grok-4", "grok-4.5", "grok-code-fast-1"],
            256_000,
        )
    })
    .or_else(|| window_for(model_id, &["grok-3", "grok-3-mini", "grok-2-1212"], 131_072))
}

fn gemini_context_window(model_id: &str) -> Option<u32> {
    window_for(model_id, &["gemini-1.5-pro"], 2_097_152)
        .or_else(|| {
            window_for(
                model_id,
                &[
                    "gemini-1.5-flash",
                    "gemini-2.0-flash",
                    "gemini-2.0-flash-lite",
                    "gemini-2.5-pro",
                    "gemini-2.5-flash",
                    "gemini-2.5-flash-lite",
                ],
                1_048_576,
            )
        })
        .or_else(|| window_for(model_id, &["gemini-1.0-pro"], 32_768))
}

fn generic_context_window(model_id: &str) -> Option<u32> {
    deepseek_context_window(model_id)
        .or_else(|| kimi_context_window(model_id))
        .or_else(|| openai_context_window(model_id))
        .or_else(|| anthropic_context_window(model_id))
        .or_else(|| glm_context_window(model_id))
        .or_else(|| mimo_context_window(model_id))
        .or_else(|| qwen_context_window(model_id))
        .or_else(|| grok_context_window(model_id))
        .or_else(|| gemini_context_window(model_id))
}

#[cfg(test)]
mod tests {
    use super::{known_context_window, known_reasoning_support, model_supports_reasoning};
    use crate::ProviderKind;

    #[test]
    fn catalog_prefers_manufacturer_and_preserves_exact_gateway_limits() {
        let date = chrono::Utc::now().date_naive().to_string();
        let catalog = serde_json::json!({
            "openai": {"models": {"gpt-6.1-sol": {"id":"gpt-6.1-sol", "release_date":date,
                "limit":{"context":1_050_000}, "reasoning":true,
                "reasoning_options":[{"type":"effort", "values":["low","medium","high","xhigh","max"]}]}}},
            "openrouter": {"models": {"openai/gpt-6.1-sol": {"id":"openai/gpt-6.1-sol", "release_date":date,
                "limit":{"context":128_000}, "reasoning":true,
                "reasoning_options":[{"type":"effort", "values":["low","high"]}]}}},
            "other": {"models": {"gpt-4o": {"id":"gpt-4o", "release_date":"2024-01-01", "limit":{"context":128_000}}}}
        });
        let document = super::parse_model_capability_document(catalog).unwrap();
        let native = super::find_model_capability(&document.models, "gpt-6.1-sol", None).unwrap();
        assert_eq!(native.context_window, Some(1_050_000));
        assert_eq!(
            super::selectable_reasoning_efforts(native, ProviderKind::OpenAiResponses),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            super::find_model_capability(&document.models, "openai/gpt-6.1-sol", None)
                .unwrap()
                .context_window,
            Some(128_000)
        );
        assert!(super::find_model_capability(&document.models, "gpt-4o", None).is_none());
        assert!(
            super::find_model_capability(&document.models, "gpt-6.1-sol-unannounced", None)
                .is_none()
        );
    }

    #[test]
    fn recent_snapshot_distinguishes_fixed_thinking_from_effort() {
        let records = super::bundled_model_capabilities();
        for (id, window) in [
            ("gpt-5.5", 1_050_000),
            ("gpt-6.1-sol", 1_050_000),
            ("claude-opus-4-7", 1_000_000),
            ("deepseek-v4-pro", 1_000_000),
            ("kimi-k3", 1_048_576),
            ("glm-5.3", 1_000_000),
            ("minimax-m2.7", 204_800),
        ] {
            assert_eq!(
                super::find_model_capability(records, id, None)
                    .unwrap()
                    .context_window,
                Some(window),
                "{id}"
            );
        }
        let fixed = super::find_model_capability(records, "minimax-m2.7", None).unwrap();
        assert_eq!(fixed.supports_reasoning, Some(true));
        assert!(super::selectable_reasoning_efforts(fixed, ProviderKind::Anthropic).is_empty());
        let glm = super::find_model_capability(records, "glm-5.3", None).unwrap();
        assert_eq!(
            super::selectable_reasoning_efforts(glm, ProviderKind::Anthropic),
            ["low", "high", "max"]
        );
        assert!(super::parse_model_capability_document(
            serde_json::json!({"version":0,"models":[]})
        )
        .is_none());
    }

    #[test]
    fn model_catalog_parses_vision_modalities_and_bundled_glm_capabilities() {
        let date = chrono::Utc::now().date_naive().to_string();
        let document = super::parse_model_capability_document(serde_json::json!({
            "zai": { "models": {
                "glm-5.3": { "id":"glm-5.3", "release_date":date,
                    "limit":{"context":1_000_000}, "modalities":{"input":["text"]} },
                "glm-5.3-flash": { "id":"glm-5.3-flash", "release_date":date,
                    "limit":{"context":1_000_000}, "modalities":{"input":["text","image"]} }
            }}
        }))
        .unwrap();
        assert_eq!(
            super::find_model_capability(&document.models, "glm-5.3", None)
                .unwrap()
                .supports_vision,
            Some(false)
        );
        assert_eq!(
            super::find_model_capability(&document.models, "glm-5.3-flash", None)
                .unwrap()
                .supports_vision,
            Some(true)
        );

        let bundled = super::bundled_model_capabilities();
        assert_eq!(
            super::find_model_capability(bundled, "glm-5.3", None)
                .unwrap()
                .supports_vision,
            Some(false)
        );
        assert_eq!(
            super::find_model_capability(bundled, "glm-5.3-flash", None)
                .unwrap()
                .supports_vision,
            Some(true)
        );
    }

    #[test]
    fn resolves_long_context_models_and_explicit_suffixes() {
        assert_eq!(
            known_context_window(ProviderKind::DeepSeek, "deepseek-v4-pro"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "qwen3-coder-plus"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiResponses, "grok-4.1-fast-reasoning"),
            Some(2_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-k3[1m]"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model[2m]"),
            Some(2_000_000)
        );
    }

    #[test]
    fn leaves_unknown_models_unresolved() {
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-experimental"),
            None
        );
    }

    #[test]
    fn resolves_named_models_across_legacy_and_compatible_providers() {
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "moonshot-v1-128k"),
            Some(131_072)
        );
        assert_eq!(
            known_context_window(ProviderKind::Glm, "glm-4.7"),
            Some(204_800)
        );
        assert_eq!(
            known_context_window(ProviderKind::MiMo, "mimo-v2.5"),
            Some(1_048_576)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "qwen3-max"),
            Some(262_144)
        );
        assert_eq!(
            known_context_window(
                ProviderKind::OpenAiCompatible,
                "grok-4.1-fast-non-reasoning"
            ),
            Some(2_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "models/gemini-2.5-flash"),
            Some(1_048_576)
        );
    }

    #[test]
    fn rejects_sub_minimum_explicit_context_suffixes() {
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model[1k]"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model[0m]"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model[5000000m]"),
            None
        );
    }

    #[test]
    fn resolves_reasoning_only_for_known_or_explicit_models() {
        assert_eq!(
            model_supports_reasoning(ProviderKind::OpenAiResponses, "gpt-6.1-sol", None),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning(ProviderKind::OpenAiResponses, "gpt-6.1-sol", Some(false)),
            Some(true)
        );
        assert_eq!(
            known_reasoning_support(ProviderKind::OpenAiResponses, "o3"),
            Some(true)
        );
        assert_eq!(
            known_reasoning_support(ProviderKind::OpenAiResponses, "gpt-5.6-luna"),
            Some(true)
        );
        assert_eq!(
            known_reasoning_support(ProviderKind::OpenAiCompatible, "gpt-5.6-luna"),
            Some(true)
        );
        assert_eq!(
            known_reasoning_support(ProviderKind::Anthropic, "claude-sonnet-4"),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning(ProviderKind::OpenAiCompatible, "custom-model", None),
            None
        );
        assert_eq!(
            model_supports_reasoning(ProviderKind::OpenAiCompatible, "custom-model", Some(true)),
            Some(true)
        );
        assert_eq!(
            model_supports_reasoning(ProviderKind::OpenAiCompatible, "gpt-5.6-luna", Some(false)),
            Some(true)
        );
    }
}
