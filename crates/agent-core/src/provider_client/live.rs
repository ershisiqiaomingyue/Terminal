//! Agent2Api / 本机网关的真实 provider 冒烟测试。
//!
//! 这些测试默认被 `#[ignore]` 跳过：它们需要一个真实网关和一把 API key，
//! 而凭据留在系统钥匙串里（`provider_store::read_provider_api_key`），
//! 不能也不应该被复制进测试代码。需要时这样跑：
//!
//! ```text
//! cd src-tauri
//! $env:AGENT_LIVE_PROVIDER_KEY = "<Agent2Api 的 key>"
//! $env:AGENT_LIVE_PROVIDER_URL = "http://127.0.0.1:3065/v1"      # 可选，默认就是这个
//! $env:AGENT_LIVE_PROVIDER_MODEL = "deepseek-v4.1-flash"          # 可选，默认就是这个
//! cargo test -p agent-core --lib --provider_client::tests::live -- --ignored --nocapture
//! ```
//!
//! 断言的是「链路真的通了」这件事本身：至少收到一段可见文本、以终态事件收尾，
//! 并打印是否拿到了 usage 上报。任何一步失败都会以可读的原因 panic，而不是静默通过。

use crate::{
    AgentEvent, AgentMessage, HttpProviderClient, ProviderConfig, ProviderKind, ProviderRequest,
    ToolDefinition,
};
use futures_util::StreamExt;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3065/v1";
const DEFAULT_MODEL: &str = "deepseek-v4.1-flash";

fn live_config() -> Option<(ProviderConfig, String, String)> {
    let key = std::env::var("AGENT_LIVE_PROVIDER_KEY").ok()?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return None;
    }
    let base_url =
        std::env::var("AGENT_LIVE_PROVIDER_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let model =
        std::env::var("AGENT_LIVE_PROVIDER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let config = ProviderConfig::new(
        "Agent2Api",
        "Agent2Api",
        ProviderKind::OpenAiResponses,
        base_url,
        "AGENT_LIVE_PROVIDER_KEY",
    )
    .expect("provider 配置应合法");
    Some((config, key, model))
}

#[tokio::test]
#[ignore = "需要真实网关 + AGENT_LIVE_PROVIDER_KEY"]
async fn live_gateway_streams_a_visible_delta_and_reports_usage() {
    let Some((config, key, model)) = live_config() else {
        panic!("未设置 AGENT_LIVE_PROVIDER_KEY，无法做真实 provider 联调");
    };
    let client = HttpProviderClient::new(config, key).expect("应能构造 provider 客户端");
    let request = ProviderRequest::new(
        "live-smoke",
        model.clone(),
        vec![AgentMessage::user("只回复两个字：收到")],
        Vec::new(),
    )
    .with_max_tokens(32)
    .with_usage_reporting(true);

    let mut stream = client
        .stream(request)
        .await
        .unwrap_or_else(|error| panic!("无法打开 {model} 的事件流: {error}"));

    let mut visible = String::new();
    let mut saw_usage = false;
    let mut terminal = false;
    while let Some(event) = stream.next().await {
        let event = event.unwrap_or_else(|error| panic!("{model} 事件流中途失败: {error}"));
        match &event {
            AgentEvent::TextDelta { text, .. } => visible.push_str(text),
            AgentEvent::Usage { .. } => saw_usage = true,
            AgentEvent::Completed { .. } | AgentEvent::Cancelled { .. } => terminal = true,
            AgentEvent::Error { message, .. } => panic!("{model} 返回错误事件: {message}"),
            _ => {}
        }
    }

    assert!(!visible.trim().is_empty(), "{model} 没有返回任何可见文本");
    assert!(terminal, "{model} 的流没有以终态事件收尾");
    eprintln!("live provider ok: model={model} text={visible:?} usage={saw_usage}");
}

/// 产品路径的真实形态：带工具定义发一轮，验证网关能回工具调用，
/// 而不是只回文本。工具调用是 Agent 轮次的核心链路（G4-A/G6 都建立在它上面）。
#[tokio::test]
#[ignore = "需要真实网关 + AGENT_LIVE_PROVIDER_KEY"]
async fn live_gateway_emits_a_tool_call_when_a_tool_is_offered() {
    let Some((config, key, model)) = live_config() else {
        panic!("未设置 AGENT_LIVE_PROVIDER_KEY，无法做真实 provider 联调");
    };
    let client = HttpProviderClient::new(config, key).expect("应能构造 provider 客户端");
    let tool = ToolDefinition::new(
        "echo_text",
        "把给定文本原样返回。",
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }),
    );
    let request = ProviderRequest::new(
        "live-tool",
        model.clone(),
        vec![AgentMessage::user(
            "用 echo_text 工具回传 hello，不要直接回答。",
        )],
        vec![tool],
    )
    .with_max_tokens(128)
    .with_usage_reporting(true);

    let mut stream = client
        .stream(request)
        .await
        .unwrap_or_else(|error| panic!("无法打开 {model} 的事件流: {error}"));

    let mut started: Option<(String, serde_json::Value)> = None;
    let mut input_delta = String::new();
    let mut terminal = false;
    while let Some(event) = stream.next().await {
        let event = event.unwrap_or_else(|error| panic!("{model} 事件流中途失败: {error}"));
        match &event {
            AgentEvent::ToolCallStarted {
                tool_name, input, ..
            } => {
                started = Some((tool_name.clone(), input.clone()));
            }
            AgentEvent::ToolCallInputDelta { delta, .. } => input_delta.push_str(delta),
            AgentEvent::Completed { .. } | AgentEvent::Cancelled { .. } => terminal = true,
            AgentEvent::Error { message, .. } => panic!("{model} 返回错误事件: {message}"),
            _ => {}
        }
    }

    assert!(terminal, "{model} 的流没有以终态事件收尾");
    let (tool_name, inline_input) = started.expect("流里没有任何工具调用事件");
    assert_eq!(
        tool_name, "echo_text",
        "{model} 发出了非预期的工具调用：{tool_name}"
    );
    // Responses 协议把完整入参放进 `response.output_item.added`（结构化），
    // Chat Completions 则走 `ToolCallInputDelta` 增量。两条路径至少命中一条。
    let has_structured_input = inline_input
        .as_object()
        .is_some_and(|object| !object.is_empty());
    assert!(
        has_structured_input || !input_delta.trim().is_empty(),
        "{model} 的工具调用既没有结构化入参也没有入参增量"
    );
    eprintln!(
        "live tool call ok: model={model} tool={tool_name} input={inline_input} delta={input_delta:?}"
    );
}
