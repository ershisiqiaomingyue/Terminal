use agent_core::{
    AgentEvent, AgentMessage, HttpProviderClient, MessageRole, ProviderConfig, ProviderKind,
    ProviderRequest,
};
use futures_util::StreamExt;
use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use crate::provider_store::{load_provider, read_provider_api_key};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnInput {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendChatRequest {
    pub provider_id: String,
    pub model: String,
    pub session_id: String,
    pub message_id: String,
    pub messages: Vec<ChatTurnInput>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatDelta {
    session_id: String,
    message_id: String,
    text: String,
}

fn chat_message(role: &str, content: String) -> Result<AgentMessage, String> {
    let message_role = match role {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        other => return Err(format!("不支持的消息角色: {other}")),
    };
    Ok(AgentMessage {
        role: message_role,
        content,
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        source_message_id: None,
    })
}

fn provider_kind(
    provider_kind: ProviderKind,
    model_protocol: Option<ProviderKind>,
) -> ProviderKind {
    model_protocol.unwrap_or(provider_kind)
}

#[tauri::command]
pub async fn send_chat_message(app: AppHandle, request: SendChatRequest) -> Result<(), String> {
    let provider = load_provider(&app, request.provider_id.trim())?;
    let model_id = request.model.trim();
    if model_id.is_empty() {
        return Err("还没有选择模型。".to_string());
    }
    let model_settings = provider.models.iter().find(|item| item.id == model_id);
    let kind = provider_kind(
        provider.kind.clone(),
        model_settings.and_then(|item| item.protocol.clone()),
    );
    let api_key = read_provider_api_key(&provider.id)?;
    let config = ProviderConfig::new(
        &provider.id,
        &provider.name,
        kind,
        &provider.base_url,
        &provider.id,
    )
    .map_err(|error| error.to_string())?;
    let client = HttpProviderClient::new(config, api_key).map_err(|error| error.to_string())?;
    let messages = request
        .messages
        .into_iter()
        .filter(|item| !item.content.trim().is_empty())
        .map(|item| chat_message(&item.role, item.content))
        .collect::<Result<Vec<_>, _>>()?;
    if messages.is_empty() {
        return Err("没有可发送的消息。".to_string());
    }
    let mut provider_request =
        ProviderRequest::new(&request.session_id, model_id, messages, Vec::new());
    provider_request.include_usage = false;
    provider_request.include_reasoning_encrypted_content = false;
    provider_request.system_prompt = Some(
        "你是一个自然、可靠的 AI 助手，可以讨论日常生活、观点、创作和技术问题。围绕用户当前的话题直接回答，不要因为应用提供 Android 或编程工具就把所有问题当成工程诊断，也不必先自我介绍或声明自己是工程助手。用用户使用的语言回答，尽量简洁、准确；区分事实、观点和推测，只有与问题相关时才说明能力限制。当前没有接入工具调用，不要假装已经执行了命令或修改了文件。".to_string(),
    );

    let mut stream = client
        .stream(provider_request)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(event) = stream.next().await {
        match event.map_err(|error| error.to_string())? {
            AgentEvent::TextDelta { text, .. } => {
                if text.is_empty() {
                    continue;
                }
                app.emit(
                    "chat:delta",
                    ChatDelta {
                        session_id: request.session_id.clone(),
                        message_id: request.message_id.clone(),
                        text,
                    },
                )
                .map_err(|error| error.to_string())?;
            }
            AgentEvent::Error { message, .. } => return Err(message),
            AgentEvent::Completed { .. } | AgentEvent::Cancelled { .. } => break,
            _ => {}
        }
    }
    Ok(())
}
