pub use agent_core::{known_context_window, ModelCapabilityRecord};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

const CAPABILITY_CACHE_FILE: &str = "agent-model-capabilities.json";
const CAPABILITY_REGISTRY_URL: &str = "https://models.dev/api.json";
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static AUTOMATIC_REFRESH_STARTED: Mutex<bool> = Mutex::new(false);

fn capability_cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(CAPABILITY_CACHE_FILE))
        .map_err(|error| format!("无法定位模型能力缓存目录: {error}"))
}

pub fn read_cached_capabilities(app: &AppHandle) -> Vec<ModelCapabilityRecord> {
    let mut records = capability_cache_path(app)
        .ok()
        .map(|path| sanitize_capabilities(agent_core::read_model_capabilities(&path)))
        .unwrap_or_default();
    for record in agent_core::bundled_model_capabilities() {
        records.retain(|cached| {
            !(cached.source.is_none() && cached.model.eq_ignore_ascii_case(&record.model))
        });
        if !records
            .iter()
            .any(|cached| cached.model.eq_ignore_ascii_case(&record.model))
        {
            records.push(record.clone());
        }
    }
    records
}

/// One automatic catalog request per process, only when no usable catalog
/// exists or a configured model is missing. Failures retry on the next launch.
pub fn ensure_capabilities_in_background(app: &AppHandle, model_ids: &[String]) {
    let records = read_cached_capabilities(app);
    let has_cache = records.iter().any(|record| record.source.is_some());
    let missing_model = model_ids
        .iter()
        .any(|id| agent_core::find_model_capability(&records, id, None).is_none());
    if has_cache && !missing_model {
        return;
    }
    let Ok(mut started) = AUTOMATIC_REFRESH_STARTED.lock() else {
        return;
    };
    if *started {
        return;
    }
    *started = true;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = refresh_remote_capabilities(app).await {
            eprintln!("{error}");
        }
    });
}

#[tauri::command]
pub fn get_agent_model_capabilities(app: AppHandle) -> Vec<ModelCapabilityRecord> {
    read_cached_capabilities(&app)
}

pub async fn refresh_remote_capabilities(app: AppHandle) -> Result<usize, String> {
    let _refresh = REFRESH_LOCK.lock().await;
    let client = agent_core::model_capability_http_client()?;
    let mut document = agent_core::fetch_model_capabilities(&client, CAPABILITY_REGISTRY_URL)
        .await
        .map_err(|error| format!("模型能力更新失败: {error}"))?;
    document.models = sanitize_capabilities(document.models);
    if document.models.is_empty() {
        return Err("模型能力清单没有可用记录".to_string());
    }
    let path = capability_cache_path(&app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "模型能力缓存目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建模型能力缓存目录: {error}"))?;
    // Merge rather than drop cached records when the rolling catalog ages out.
    for record in read_cached_capabilities(&app) {
        if !document
            .models
            .iter()
            .any(|fresh| fresh.model.eq_ignore_ascii_case(&record.model))
        {
            document.models.push(record);
        }
    }
    let temporary = path.with_extension("json.tmp");
    agent_core::write_model_capabilities(&temporary, &document)
        .map_err(|error| format!("无法保存模型能力清单: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("无法替换模型能力缓存: {error}"))?;
    let _ = app.emit("agent:model-capabilities-changed", document.models.len());
    Ok(document.models.len())
}

fn sanitize_capabilities(records: Vec<ModelCapabilityRecord>) -> Vec<ModelCapabilityRecord> {
    records
        .into_iter()
        .filter(|record| {
            !record.model.trim().is_empty()
                && record
                    .context_window
                    .map(|window| window >= 1_024)
                    .unwrap_or(true)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::known_context_window;
    use super::sanitize_capabilities;
    use agent_core::{ModelCapabilityRecord, ProviderKind};

    #[test]
    fn resolves_provider_specific_long_context_models() {
        assert_eq!(
            known_context_window(ProviderKind::DeepSeek, "deepseek-v4-flash"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-k3"),
            Some(1_048_576)
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-k2.6"),
            Some(262_144)
        );
        assert_eq!(
            known_context_window(ProviderKind::Anthropic, "claude-opus-4-8"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::Glm, "glm-4.7"),
            Some(204_800)
        );
        assert_eq!(
            known_context_window(ProviderKind::MiMo, "mimo-v2.5-pro"),
            Some(1_048_576)
        );
        assert_eq!(
            known_context_window(ProviderKind::Anthropic, "qwen3-coder-plus"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiResponses, "grok-4.1-fast-reasoning"),
            Some(2_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "google/gemini-2.5-pro"),
            Some(1_048_576)
        );
    }

    #[test]
    fn resolves_openai_models_and_compatible_gateways() {
        assert_eq!(
            known_context_window(ProviderKind::OpenAi, "gpt-4.1"),
            Some(1_047_576)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAi, "gpt-5.6"),
            Some(1_050_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiResponses, "gpt-5.6-terra"),
            Some(1_050_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "deepseek-v4-pro"),
            Some(1_000_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "gpt-4o:free"),
            Some(128_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "claude-haiku-4-5-20251001"),
            Some(200_000)
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-k3[1m]"),
            Some(1_000_000)
        );
    }

    #[test]
    fn does_not_guess_unknown_model_limits() {
        assert_eq!(
            known_context_window(ProviderKind::OpenAi, "my-custom-model"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::Kimi, "kimi-experimental"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "qwen3.5-9b-unknown"),
            None
        );
        assert_eq!(
            known_context_window(ProviderKind::OpenAiCompatible, "custom-model[unknown]"),
            None
        );
    }

    #[test]
    fn keeps_unknown_cached_models_unresolved_and_rejects_invalid_windows() {
        let records = sanitize_capabilities(vec![
            ModelCapabilityRecord {
                model: "custom-model".to_string(),
                provider: Some("custom".to_string()),
                context_window: None,
                protocols: vec!["anthropic".to_string()],
                supports_reasoning: None,
                supports_vision: None,
                reasoning_efforts: Vec::new(),
                reasoning_budget: false,
                source: None,
            },
            ModelCapabilityRecord {
                model: "bad-window".to_string(),
                provider: Some("custom".to_string()),
                context_window: Some(512),
                protocols: vec![],
                supports_reasoning: None,
                supports_vision: None,
                reasoning_efforts: Vec::new(),
                reasoning_budget: false,
                source: None,
            },
            ModelCapabilityRecord {
                model: "".to_string(),
                provider: None,
                context_window: Some(64_000),
                protocols: vec![],
                supports_reasoning: None,
                supports_vision: None,
                reasoning_efforts: Vec::new(),
                reasoning_budget: false,
                source: None,
            },
        ]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "custom-model");
        assert_eq!(records[0].context_window, None);
    }
}
