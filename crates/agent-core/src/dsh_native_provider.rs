use crate::dsh_provider::DshChunkEncoder;
use crate::provider_client::AgentEventStream;
use crate::{
    finish_text_delta, finish_tool_markup_delta, normalize_stream_event, AgentEvent,
    ToolStreamState,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub type DshProviderObserver = Arc<dyn Fn(Value) + Send + Sync>;

pub type DshChunkStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<Value, DshStreamError>> + Send>>;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct DshStreamError {
    pub code: String,
    pub message: String,
}

impl DshStreamError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Share stream normalization and repetition guards with the existing native host.
/// A broken/truncated stream throws; it must never produce a successful DSH finish.
pub fn encode_provider_stream(
    events: AgentEventStream,
    session_id: &str,
    kind: crate::ProviderKind,
    model: &str,
    observer: DshProviderObserver,
) -> DshChunkStream {
    encode_provider_stream_for_route(events, session_id, kind, model, None, observer)
}

pub fn encode_provider_stream_for_route(
    mut events: AgentEventStream,
    session_id: &str,
    kind: crate::ProviderKind,
    model: &str,
    route_id: Option<String>,
    observer: DshProviderObserver,
) -> DshChunkStream {
    let session_id = session_id.to_owned();
    let model = model.to_owned();
    Box::pin(async_stream::try_stream! {
        let mut encoder = DshChunkEncoder::new(kind, model).with_route_id(route_id);
        let mut stream_state = ToolStreamState::default();
        let mut repetition = crate::round::RepetitionGuard::default();
        let mut channels = crate::output_channels::OutputChannels::default();
        let mut completed = false;
        loop {
            let item = tokio::time::timeout(crate::PROVIDER_IDLE_TIMEOUT + Duration::from_secs(5), events.next()).await
            .map_err(|_| DshStreamError::new("PROVIDER_IDLE_TIMEOUT", "等待模型响应超时，已保留此前内容。"))?;
            let Some(event) = item else { break; };
            let event = event.map_err(|error| DshStreamError::new(error.code(), error.to_string()))?;
            completed |= matches!(event, AgentEvent::Completed { .. });
            for mut event in normalize_stream_event(event, &mut stream_state) {
                let repeated = repetition.observe_prefix(&mut event);
                let routed = match event {
                    AgentEvent::TextDelta { text, .. } => channels.push(&session_id, &text),
                    event @ AgentEvent::AnthropicThinkingFrame { .. } => vec![event],
                    event => { let mut routed = channels.finish(&session_id); routed.push(event); routed }
                };
                for event in routed {
                    if matches!(event, AgentEvent::Status { .. } | AgentEvent::Usage { .. }) {
                        observer(json!({"type":"native-provider-event", "sessionId":session_id, "event":event}));
                    }
                    for chunk in encoder.push(event).map_err(|message| DshStreamError::new("PROVIDER_STREAM_ERROR", message))? {
                        yield chunk;
                    }
                }
                if repeated {
                    for event in channels.finish(&session_id) {
                        for chunk in encoder.push(event).map_err(|message| DshStreamError::new("PROVIDER_STREAM_ERROR", message))? { yield chunk; }
                    }
                    Err(DshStreamError::new("repetitive_output", "检测到连续重复输出，已停止生成并保留此前内容。"))?;
                }
            }
        }
        if !completed {
            Err(DshStreamError::new("PROVIDER_STREAM_INCOMPLETE", "模型连接在终态前结束，已保留此前内容。"))?;
        }
        let mut remaining = finish_text_delta(&session_id, &mut stream_state);
        remaining.extend(finish_tool_markup_delta(&session_id, &mut stream_state));
        for mut event in remaining {
            let repeated = repetition.observe_prefix(&mut event);
            let routed = match event {
                AgentEvent::TextDelta { text, .. } => channels.push(&session_id, &text),
                event => vec![event],
            };
            for event in routed {
                for chunk in encoder.push(event).map_err(|message| DshStreamError::new("PROVIDER_STREAM_ERROR", message))? { yield chunk; }
            }
            if repeated { Err(DshStreamError::new("repetitive_output", "检测到连续重复输出，已停止生成。"))?; }
        }
        for event in channels.finish(&session_id) {
            for chunk in encoder.push(event).map_err(|message| DshStreamError::new("PROVIDER_STREAM_ERROR", message))? { yield chunk; }
        }
        for chunk in encoder.finish().map_err(|message| DshStreamError::new("PROVIDER_STREAM_ERROR", message))? { yield chunk; }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderClientError, ProviderKind};
    use futures_util::stream;

    #[tokio::test]
    async fn native_failure_exposes_reported_usage_without_successful_finish() {
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events = stream::iter(vec![
            Ok(AgentEvent::Usage {
                session_id: "s".into(),
                input_tokens: Some(100),
                output_tokens: Some(19),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                reasoning_tokens: None,
            }),
            Err(ProviderClientError::Network("failed".into())),
        ]);
        let captured = observed.clone();
        let chunks = encode_provider_stream(
            Box::pin(events),
            "s",
            ProviderKind::Anthropic,
            "m",
            Arc::new(move |event| captured.lock().unwrap().push(event)),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(!chunks
            .iter()
            .any(|chunk| chunk.as_ref().is_ok_and(|chunk| chunk["type"] == "finish")));
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0]["event"]["type"], "usage");
        assert_eq!(observed[0]["event"]["output_tokens"], 19);
        assert!(observed[0]["event"]["cached_input_tokens"].is_null());
    }

    #[tokio::test]
    async fn private_claude_frames_do_not_break_split_output_channel_detection() {
        let text = "The user wants me to inspect. Based on the conversation, I need to:\n1. Check files\n\n已完成文件检查。";
        let mut frames = vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ];
        for part in [
            "The user wants ",
            "me to inspect. Based on the conversation, I need to:\n",
            "1. Check files\n\n已完成文件检查。",
        ] {
            frames.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":part}}));
        }
        frames.extend([
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_stop"}),
        ]);
        let events = frames
            .into_iter()
            .flat_map(|frame| {
                crate::parse_sse_data(ProviderKind::Anthropic, "s", &frame.to_string())
                    .unwrap()
                    .into_iter()
                    .map(Ok)
            })
            .collect::<Vec<_>>();
        let chunks = encode_provider_stream(
            Box::pin(stream::iter(events)),
            "s",
            ProviderKind::Anthropic,
            "m",
            Arc::new(|_| {}),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let displayed = chunks
            .iter()
            .filter(|chunk| chunk["type"] == "text-delta")
            .filter_map(|chunk| chunk["text"].as_str())
            .collect::<String>();
        assert_eq!(displayed, "已完成文件检查。");
        assert_eq!(
            chunks.last().unwrap()["replayState"]["response"]["anthropicContentBlocks"][0]["text"],
            text
        );
    }

    #[tokio::test]
    async fn native_stream_eof_without_completion_keeps_partial_text_and_never_finishes() {
        let chunks = encode_provider_stream(
            Box::pin(stream::iter(vec![Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "partial".into(),
            })])),
            "s",
            ProviderKind::OpenAiResponses,
            "m",
            Arc::new(|_| {}),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(chunks.iter().any(|chunk| chunk
            .as_ref()
            .is_ok_and(|chunk| chunk["type"] == "text-delta")));
        assert!(!chunks
            .iter()
            .any(|chunk| chunk.as_ref().is_ok_and(|chunk| chunk["type"] == "finish")));
        assert_eq!(
            chunks.last().unwrap().as_ref().unwrap_err().code,
            "PROVIDER_STREAM_INCOMPLETE"
        );
    }

    #[tokio::test]
    async fn native_stream_retains_partial_text_but_never_finishes_a_network_failure() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "已经核对".into(),
            }),
            Err(ProviderClientError::Network("disconnected".into())),
        ]);
        let chunks = encode_provider_stream(
            Box::pin(events),
            "s",
            ProviderKind::Anthropic,
            "m",
            Arc::new(|_| {}),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(chunks.iter().any(|chunk| chunk
            .as_ref()
            .is_ok_and(|chunk| chunk["type"] == "text-delta")));
        assert!(!chunks
            .iter()
            .any(|chunk| chunk.as_ref().is_ok_and(|chunk| chunk["type"] == "finish")));
        assert_eq!(chunks.last().unwrap().as_ref().unwrap_err().code, "network");
    }

    #[tokio::test]
    async fn native_stream_stops_repetition_without_consuming_later_draft() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "已检查文件。".into(),
            }),
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "先核对设备与包状态。".repeat(1000),
            }),
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "later draft".into(),
            }),
        ]);
        let chunks = encode_provider_stream(
            Box::pin(events),
            "s",
            ProviderKind::OpenAi,
            "m",
            Arc::new(|_| {}),
        )
        .collect::<Vec<_>>()
        .await;
        assert_eq!(
            chunks.last().unwrap().as_ref().unwrap_err().code,
            "repetitive_output"
        );
        let rendered = chunks
            .iter()
            .filter_map(|chunk| chunk.as_ref().ok())
            .filter(|chunk| chunk["type"] == "text-delta")
            .filter_map(|chunk| chunk["text"].as_str())
            .collect::<String>();
        assert!(rendered.starts_with("已检查文件。"));
        assert!(!rendered.contains("later draft"));
        assert!(rendered.chars().count() < 250);
    }
}
