use crate::provider_client::AgentEventStream;
use crate::{
    finish_text_delta, finish_tool_markup_delta, normalize_stream_event, AgentEvent,
    AgentRoundState, AgentSteerRequest, AgentUsage, PendingToolCall, ProviderClientError,
    ToolStreamState,
};
use futures_util::StreamExt;
use serde_json::json;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// The result of one provider stream, before the host starts executing tools.
///
/// Protocol-specific stream normalization, markup parsing, and partial
/// tool-call assembly belong in the reusable core so the desktop app and a
/// future CLI host cannot drift apart.
pub struct ModelRoundResult {
    pub events: Vec<AgentEvent>,
    pub text: String,
    pub reasoning_content: String,
    pub output_tokens: Option<u64>,
    pub output_truncated: bool,
    pub usage: Option<AgentUsage>,
    pub responses_reasoning_items: Vec<serde_json::Value>,
    pub anthropic_thinking_blocks: Vec<serde_json::Value>,
    pub anthropic_content_blocks: Vec<serde_json::Value>,
    pub pending_calls: Vec<PendingToolCall>,
    /// OpenAI-compatible providers may send a complete tool-call array as
    /// ordinary text. These calls are synthesized after the stream closes,
    /// so hosts must not render the raw JSON that preceded them.
    pub synthesized_plain_tool_calls: bool,
    pub steered: Option<AgentSteerRequest>,
    /// A client-side safety guard stopped the provider stream after usable
    /// output had already arrived. Hosts should preserve `text`, surface this
    /// diagnostic as recoverable status, and finish the turn without retrying
    /// the same stream automatically.
    pub interruption: Option<ModelRoundInterruption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoundInterruption {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug)]
pub enum ModelRoundError {
    Cancelled {
        pending_calls: Vec<PendingToolCall>,
    },
    Provider {
        error: ProviderClientError,
        pending_calls: Vec<PendingToolCall>,
    },
}

impl ModelRoundError {
    pub fn into_pending_calls(self) -> Vec<PendingToolCall> {
        match self {
            Self::Cancelled { pending_calls } | Self::Provider { pending_calls, .. } => {
                pending_calls
            }
        }
    }

    pub fn provider_error(&self) -> Option<&ProviderClientError> {
        match self {
            Self::Cancelled { .. } => None,
            Self::Provider { error, .. } => Some(error),
        }
    }
}

#[derive(Default)]
struct RepetitionWindow {
    characters: VecDeque<char>,
    unchecked_characters: usize,
    consecutive_whitespace: usize,
    consecutive_line_breaks: usize,
    observed_characters: usize,
}

impl RepetitionWindow {
    #[cfg(test)]
    fn push(&mut self, text: &str) -> bool {
        self.push_prefix(text).is_some()
    }

    fn push_prefix(&mut self, text: &str) -> Option<usize> {
        const SHORT_MAX_PERIOD: usize = 256;
        const MAX_PERIOD: usize = 8192;
        const MIN_REPETITIONS: usize = 8;
        const MIN_REPEATED_CHARACTERS: usize = 160;
        const LONG_MIN_REPETITIONS: usize = 3;
        const WINDOW_SIZE: usize = MAX_PERIOD * LONG_MIN_REPETITIONS;
        for (offset, character) in text.char_indices() {
            self.observed_characters = self.observed_characters.wrapping_add(1);
            if character.is_whitespace() {
                self.consecutive_whitespace += 1;
                self.consecutive_line_breaks += usize::from(character == '\n');
                // Normal indentation and paragraph spacing remain untouched.
                // A model emitting blank lines indefinitely cannot progress.
                if self.consecutive_line_breaks >= 64 || self.consecutive_whitespace >= 2048 {
                    return Some(offset);
                }
            } else {
                self.consecutive_whitespace = 0;
                self.consecutive_line_breaks = 0;
            }
            self.characters.push_back(character);
            if self.characters.len() > WINDOW_SIZE {
                self.characters.pop_front();
            }
            self.unchecked_characters += 1;
            if self.characters.len() < MIN_REPEATED_CHARACTERS || self.unchecked_characters < 16 {
                continue;
            }
            self.unchecked_characters = 0;
            let characters = self.characters.make_contiguous();
            let end = characters.len();
            // Sample large periods less frequently; keep short-loop latency unchanged.
            let max_period = if self.observed_characters % 128 < 16 {
                MAX_PERIOD
            } else {
                1024
            };
            for period in 4..=max_period.min(end / LONG_MIN_REPETITIONS) {
                let repetitions = if period <= SHORT_MAX_PERIOD {
                    MIN_REPETITIONS.max(MIN_REPEATED_CHARACTERS.div_ceil(period))
                } else {
                    LONG_MIN_REPETITIONS
                };
                let repeated_length = period * repetitions;
                if repeated_length > end {
                    continue;
                }
                let unit = &characters[end - period..];
                if !characters[end - repeated_length..end]
                    .chunks_exact(period)
                    .all(|chunk| chunk == unit)
                {
                    continue;
                }
                let distinct_letters = unit
                    .iter()
                    .filter(|character| character.is_alphanumeric())
                    .copied()
                    .collect::<HashSet<_>>();
                if distinct_letters.len() >= 4 {
                    return Some(offset);
                }
            }
        }
        None
    }
}

#[derive(Default)]
pub(crate) struct RepetitionGuard {
    text: RepetitionWindow,
    reasoning: RepetitionWindow,
}

impl RepetitionGuard {
    #[cfg(test)]
    fn observe(&mut self, event: &AgentEvent) -> bool {
        self.observe_prefix(&mut event.clone())
    }

    pub(crate) fn observe_prefix(&mut self, event: &mut AgentEvent) -> bool {
        match event {
            AgentEvent::TextDelta { .. } | AgentEvent::ReasoningDelta { .. } => {
                let window = if matches!(event, AgentEvent::TextDelta { .. }) {
                    &mut self.text
                } else {
                    &mut self.reasoning
                };
                let text = match event {
                    AgentEvent::TextDelta { text, .. }
                    | AgentEvent::ReasoningDelta { text, .. } => text,
                    _ => unreachable!(),
                };
                if let Some(offset) = window.push_prefix(text) {
                    text.truncate(offset);
                    text.truncate(text.trim_end().len());
                    return true;
                }
                false
            }
            AgentEvent::ToolCallStarted { .. } => {
                *self = Self::default();
                false
            }
            _ => false,
        }
    }

    fn interruption() -> ModelRoundInterruption {
        ModelRoundInterruption {
            code: "repetitive_output".to_string(),
            message: "检测到模型连续重复输出，已停止本轮生成以避免继续消耗 token。已保留此前生成的内容；可以调整问题或切换模型后重试。".to_string(),
            retryable: true,
        }
    }
}

fn interrupted_round_result(
    round_state: AgentRoundState,
    emitted: Vec<AgentEvent>,
    steered: Option<AgentSteerRequest>,
    interruption: ModelRoundInterruption,
) -> ModelRoundResult {
    ModelRoundResult {
        text: round_state.text,
        reasoning_content: round_state.reasoning_content,
        output_tokens: round_state.output_tokens,
        output_truncated: round_state.output_truncated,
        usage: round_state.usage,
        responses_reasoning_items: round_state.responses_reasoning_items,
        anthropic_thinking_blocks: round_state.anthropic_thinking_blocks,
        anthropic_content_blocks: round_state.anthropic_content_blocks,
        pending_calls: round_state.pending_calls,
        events: emitted,
        synthesized_plain_tool_calls: false,
        steered,
        interruption: Some(interruption),
    }
}

fn ingest_event<F>(
    event: AgentEvent,
    round_state: &mut AgentRoundState,
    emitted: &mut Vec<AgentEvent>,
    notify_observer: bool,
    observer: &mut F,
) where
    F: FnMut(&AgentEvent),
{
    let (event, should_emit) = round_state.ingest(event);
    if let AgentEvent::ToolCallInputDelta {
        call_id,
        tool_index,
        delta,
        ..
    } = &event
    {
        let resolved_call_id = tool_index
            .and_then(|index| round_state.tool_ids_by_index.get(&index))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(call_id);
        if let Some((_, (_, _, arguments))) = round_state
            .pending_calls
            .iter_mut()
            .find(|(id, _)| id == resolved_call_id)
        {
            arguments.push_str(delta);
        }
    }
    if should_emit {
        let preview_prefix = match &event {
            AgentEvent::ToolCallStarted {
                session_id,
                call_id,
                tool_index,
                ..
            } => round_state
                .pending_calls
                .iter()
                .find(|(id, _)| id == call_id)
                .filter(|(_, (_, _, arguments))| !arguments.is_empty())
                .map(|(_, (_, _, arguments))| AgentEvent::ToolCallInputDelta {
                    session_id: session_id.clone(),
                    call_id: call_id.clone(),
                    tool_index: *tool_index,
                    delta: arguments.clone(),
                }),
            _ => None,
        };
        if notify_observer {
            observer(&event);
        }
        emitted.push(event);
        // Both streaming observers and replay consumers receive the prefix
        // accumulated before a relay supplied the real call ID.
        if let Some(prefix) = preview_prefix {
            if notify_observer {
                observer(&prefix);
            }
            emitted.push(prefix);
        }
    }
}

/// Consume one provider stream and return a normalized model round.
///
/// The host remains responsible for permission decisions and tool execution.
/// This function owns everything up to that boundary, including cancellation
/// while a stream is active. Steering stays queued for a completed-step boundary;
/// a new user message cannot mutate an in-flight HTTP request.
pub async fn collect_model_round_with_observer<F>(
    mut events: AgentEventStream,
    session_id: &str,
    stream_state: &mut ToolStreamState,
    _steer_receiver: &mut mpsc::Receiver<AgentSteerRequest>,
    cancellation: &AtomicBool,
    allowed_tool_names: &[String],
    mut observer: F,
) -> Result<ModelRoundResult, ModelRoundError>
where
    F: FnMut(&AgentEvent),
{
    let mut round_state = AgentRoundState::default();
    let mut repetition_guard = RepetitionGuard::default();
    let mut output_channels = crate::output_channels::OutputChannels::default();
    let mut emitted = Vec::new();
    let steered = None;
    let idle = crate::PROVIDER_IDLE_TIMEOUT + std::time::Duration::from_secs(5);
    let mut idle_deadline = tokio::time::Instant::now() + idle;

    loop {
        let next_event = tokio::select! {
            biased;
            item = events.next() => {
                idle_deadline = tokio::time::Instant::now() + idle;
                item
            }
            _ = tokio::time::sleep_until(idle_deadline) => {
                for event in output_channels.finish(session_id) {
                    ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
                }
                return Err(ModelRoundError::Provider {
                    error: ProviderClientError::Timeout("模型长时间没有新响应，已停止等待并保留此前内容。".to_string()),
                    pending_calls: round_state.pending_calls,
                });
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if cancellation.load(Ordering::Relaxed) { None } else { continue }
            }
        };
        if cancellation.load(Ordering::Relaxed) {
            for event in output_channels.finish(session_id) {
                ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
            }
            return Err(ModelRoundError::Cancelled {
                pending_calls: round_state.pending_calls,
            });
        }
        let Some(event) = next_event else {
            break;
        };
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                for event in output_channels.finish(session_id) {
                    ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
                }
                return Err(ModelRoundError::Provider {
                    error,
                    pending_calls: round_state.pending_calls,
                });
            }
        };
        for mut event in normalize_stream_event(event, stream_state) {
            let repeated = repetition_guard.observe_prefix(&mut event);
            let routed = match event {
                AgentEvent::TextDelta { session_id, text } => {
                    output_channels.push(&session_id, &text)
                }
                event @ AgentEvent::AnthropicThinkingFrame { .. } => vec![event],
                event => {
                    let mut routed = output_channels.finish(session_id);
                    routed.push(event);
                    routed
                }
            };
            for event in routed {
                ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
            }
            if repeated {
                for event in output_channels.finish(session_id) {
                    ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
                }
                return Ok(interrupted_round_result(
                    round_state,
                    emitted,
                    steered,
                    RepetitionGuard::interruption(),
                ));
            }
        }
    }

    if cancellation.load(Ordering::Relaxed) {
        return Err(ModelRoundError::Cancelled {
            pending_calls: round_state.pending_calls,
        });
    }

    for mut event in finish_text_delta(session_id, stream_state) {
        let repeated = repetition_guard.observe_prefix(&mut event);
        let routed = match event {
            AgentEvent::TextDelta { text, .. } => output_channels.push(session_id, &text),
            event => vec![event],
        };
        for event in routed {
            ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
        }
        if repeated {
            for event in output_channels.finish(session_id) {
                ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
            }
            return Ok(interrupted_round_result(
                round_state,
                emitted,
                steered,
                RepetitionGuard::interruption(),
            ));
        }
    }
    for mut event in finish_tool_markup_delta(session_id, stream_state) {
        let repeated = repetition_guard.observe_prefix(&mut event);
        let routed = match event {
            AgentEvent::TextDelta { text, .. } => output_channels.push(session_id, &text),
            event => vec![event],
        };
        for event in routed {
            ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
        }
        if repeated {
            for event in output_channels.finish(session_id) {
                ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
            }
            return Ok(interrupted_round_result(
                round_state,
                emitted,
                steered,
                RepetitionGuard::interruption(),
            ));
        }
    }
    for event in output_channels.finish(session_id) {
        ingest_event(event, &mut round_state, &mut emitted, true, &mut observer);
    }

    round_state
        .validate_anthropic_replay()
        .map_err(|message| ModelRoundError::Provider {
            error: ProviderClientError::Parse(message.into()),
            pending_calls: round_state.pending_calls.clone(),
        })?;

    // A few OpenAI-compatible gateways return tool calls as ordinary text.
    // Convert only a complete array containing allowlisted tool names, so raw
    // tool JSON is never persisted or rendered as the assistant answer.
    let mut synthesized_plain_tool_calls = false;
    if round_state.pending_calls.is_empty() && round_state.anthropic_content_blocks.is_empty() {
        if let Some(calls) =
            crate::stream::parse_plain_json_tool_calls(&round_state.text, allowed_tool_names)
        {
            synthesized_plain_tool_calls = true;
            emitted.retain(|event| !matches!(event, AgentEvent::TextDelta { .. }));
            round_state.text.clear();
            for (index, (tool_name, input)) in calls.into_iter().enumerate() {
                let call_id = format!("json-call-{index}");
                ingest_event(
                    AgentEvent::ToolCallStarted {
                        session_id: session_id.to_string(),
                        call_id: call_id.clone(),
                        tool_index: None,
                        tool_name,
                        input: json!({}),
                    },
                    &mut round_state,
                    &mut emitted,
                    false,
                    &mut observer,
                );
                ingest_event(
                    AgentEvent::ToolCallInputDelta {
                        session_id: session_id.to_string(),
                        call_id,
                        tool_index: None,
                        delta: serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()),
                    },
                    &mut round_state,
                    &mut emitted,
                    false,
                    &mut observer,
                );
            }
        }
    }

    Ok(ModelRoundResult {
        text: round_state.text.clone(),
        reasoning_content: round_state.reasoning_content,
        output_tokens: round_state.output_tokens,
        output_truncated: round_state.output_truncated,
        usage: round_state.usage,
        responses_reasoning_items: round_state.responses_reasoning_items,
        anthropic_thinking_blocks: round_state.anthropic_thinking_blocks,
        anthropic_content_blocks: round_state.anthropic_content_blocks,
        pending_calls: round_state.pending_calls,
        events: emitted,
        synthesized_plain_tool_calls,
        steered,
        interruption: None,
    })
}

pub async fn collect_model_round(
    events: AgentEventStream,
    session_id: &str,
    stream_state: &mut ToolStreamState,
    steer_receiver: &mut mpsc::Receiver<AgentSteerRequest>,
    cancellation: &AtomicBool,
    allowed_tool_names: &[String],
) -> Result<ModelRoundResult, ModelRoundError> {
    collect_model_round_with_observer(
        events,
        session_id,
        stream_state,
        steer_receiver,
        cancellation,
        allowed_tool_names,
        |_| {},
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::atomic::AtomicBool;

    #[tokio::test]
    async fn failed_stream_flushes_a_buffered_visible_prefix_to_the_observer() {
        let mut visible = String::new();
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "The".into(),
            }),
            Err(ProviderClientError::Network("disconnected".into())),
        ]);
        let (_sender, mut steer) = mpsc::channel(1);
        let result = collect_model_round_with_observer(
            Box::pin(events),
            "s",
            &mut ToolStreamState::default(),
            &mut steer,
            &AtomicBool::new(false),
            &[],
            |event| {
                if let AgentEvent::TextDelta { text, .. } = event {
                    visible.push_str(text);
                }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(visible, "The");
    }

    #[tokio::test]
    async fn delayed_call_ids_publish_one_call_with_the_accumulated_argument_prefix() {
        let events = stream::iter(vec![
            Ok(AgentEvent::ToolCallStarted {
                session_id: "s".into(),
                call_id: "tool-index-0".into(),
                tool_index: Some(0),
                tool_name: "read_".into(),
                input: json!({}),
            }),
            Ok(AgentEvent::ToolCallInputDelta {
                session_id: "s".into(),
                call_id: "tool-index-0".into(),
                tool_index: Some(0),
                delta: "{\"path\":".into(),
            }),
            Ok(AgentEvent::ToolCallStarted {
                session_id: "s".into(),
                call_id: "real-call".into(),
                tool_index: Some(0),
                tool_name: "local_file".into(),
                input: json!({}),
            }),
            Ok(AgentEvent::ToolCallInputDelta {
                session_id: "s".into(),
                call_id: "tool-index-0".into(),
                tool_index: Some(0),
                delta: "\"a.txt\"}".into(),
            }),
        ]);
        let (_sender, mut steer) = mpsc::channel(1);
        let mut observed = Vec::new();
        let result = collect_model_round_with_observer(
            Box::pin(events),
            "s",
            &mut ToolStreamState::default(),
            &mut steer,
            &AtomicBool::new(false),
            &[],
            |event| observed.push(event.clone()),
        )
        .await
        .unwrap();
        assert_eq!(result.pending_calls.len(), 1);
        assert_eq!(result.pending_calls[0].0, "real-call");
        assert_eq!(result.pending_calls[0].1 .0, "read_local_file");
        assert_eq!(result.pending_calls[0].1 .2, r#"{"path":"a.txt"}"#);
        assert_eq!(
            observed
                .iter()
                .filter(|event| matches!(event, AgentEvent::ToolCallStarted { .. }))
                .count(),
            1
        );
        let preview = observed
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolCallInputDelta { call_id, delta, .. } => {
                    assert_eq!(call_id, "real-call");
                    Some(delta.as_str())
                }
                _ => None,
            })
            .collect::<String>();
        assert_eq!(preview, result.pending_calls[0].1 .2);
        assert_eq!(observed, result.events);
    }

    #[test]
    fn repetition_window_detects_chinese_sentence_across_token_boundaries() {
        let mut window = RepetitionWindow::default();
        let text = "先核对设备与包状态。".repeat(500);
        let stopped_at = text
            .chars()
            .position(|character| window.push(&character.to_string()));
        assert!(stopped_at.is_some_and(|index| index < 200));
    }

    #[test]
    fn repetition_window_detects_one_large_provider_chunk() {
        assert!(RepetitionWindow::default().push(&"先核对设备与包状态。".repeat(500)));
    }

    #[test]
    fn repetition_window_stops_blank_line_and_space_floods_across_chunks() {
        let mut window = RepetitionWindow::default();
        assert!(!window.push("已保留正文。\n\n"));
        for _ in 0..61 {
            assert!(!window.push("\r\n  "));
        }
        assert!(window.push("\n"));
        assert!(RepetitionWindow::default().push(&"\n".repeat(4088)));
        assert!(RepetitionWindow::default().push(&" ".repeat(2048)));
    }

    #[test]
    fn repetition_window_preserves_normal_indentation_paragraphs_and_tables() {
        let mut window = RepetitionWindow::default();
        for index in 0..1000 {
            assert!(!window.push(&format!("\n\n    value_{index}\n\n| {index} | result |\n")));
        }
        assert!(!window.push(&" ".repeat(512)));
        assert!(!window.push("nonblank resets whitespace\n\n"));
        assert!(!window.push(&" ".repeat(512)));
    }

    #[tokio::test]
    async fn whitespace_flood_retains_text_without_consuming_later_draft() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "session".into(),
                text: "已核对设备。".into(),
            }),
            Ok(AgentEvent::TextDelta {
                session_id: "session".into(),
                text: "\n".repeat(4088),
            }),
            Ok(AgentEvent::TextDelta {
                session_id: "session".into(),
                text: "later draft must not be consumed".into(),
            }),
        ]);
        let (_sender, mut steer) = mpsc::channel(1);
        let result = collect_model_round(
            Box::pin(events),
            "session",
            &mut ToolStreamState::default(),
            &mut steer,
            &AtomicBool::new(false),
            &[],
        )
        .await
        .unwrap();
        assert_eq!(result.text, "已核对设备。");
        assert_eq!(result.interruption.unwrap().code, "repetitive_output");
    }

    #[test]
    fn repetition_window_allows_normal_repeated_words_and_markdown() {
        let mut window = RepetitionWindow::default();
        assert!(!window.push(&"先核对设备与包状态。".repeat(3)));
        assert!(!window.push(&"|---|---|\n".repeat(500)));
        for index in 0..500 {
            assert!(!window.push(&format!(
                "第 {index} 项：核对设备和包状态后，记录本次结果。\n"
            )));
        }
        assert!(window.characters.len() <= 24_576);
    }

    #[test]
    fn repetition_window_detects_long_mixed_language_tool_prose() {
        let block = format!(
            "检查文件头和安装结果。\n\n---\n\nCall Tools:\n{}\nNeed file header. Use PowerShell Format-Hex.\n\nOnly actual function calls execute tools.\n",
            (0..10).map(|index| format!("- tool-{index}: windows__read_local_file with a distinct file argument\n")).collect::<String>()
        );
        assert!((257..=1024).contains(&block.chars().count()));
        let mut window = RepetitionWindow::default();
        let stopped_at = block
            .repeat(8)
            .chars()
            .position(|character| window.push(&character.to_string()));
        assert!(stopped_at.is_some_and(|index| index < block.chars().count() * 4));
    }

    #[test]
    fn repetition_window_detects_planning_blocks_larger_than_the_old_window() {
        let block = (0..25).map(|index| format!("Step {index}: inspect the component and retain its distinct observed evidence.\n")).collect::<String>();
        assert!(block.chars().count() > 1457);
        assert!(!RepetitionWindow::default().push(&block.repeat(2)));
        for size in [1, 37, 1024, block.len() * 5] {
            let mut window = RepetitionWindow::default();
            assert!(block
                .repeat(5)
                .as_bytes()
                .chunks(size)
                .any(|chunk| window.push(std::str::from_utf8(chunk).unwrap())));
        }
    }

    #[test]
    fn repetition_window_allows_two_long_quotes_and_changing_file_results() {
        let mut window = RepetitionWindow::default();
        let results = (0..500).map(|index| format!("第 {index} 个文件的结果：文件头、大小、哈希和包名已经核对，保存第 {index} 项的真实证据。\n")).collect::<String>();
        assert!(!window.push(&results));
        // A long quotation repeated twice is not a paragraph loop.
        let quote = (0..30)
            .map(|index| format!("第 {index} 项的文件核对结果。\n"))
            .collect::<String>();
        assert!(!RepetitionWindow::default().push(&quote.repeat(2)));
    }

    #[test]
    fn repetition_guard_keeps_text_and_reasoning_separate_and_resets_on_tools() {
        let mut guard = RepetitionGuard::default();
        for index in 0..30 {
            for event in [
                AgentEvent::TextDelta {
                    session_id: "session".to_string(),
                    text: "先核对设备与包状态。".repeat(7),
                },
                AgentEvent::ReasoningDelta {
                    session_id: "session".to_string(),
                    text: "先核对设备与包状态。".repeat(7),
                },
                AgentEvent::ToolCallStarted {
                    session_id: "session".to_string(),
                    call_id: format!("call-{index}"),
                    tool_index: None,
                    tool_name: "read_local_file".to_string(),
                    input: json!({"path": format!("file-{index}")}),
                },
            ] {
                assert!(!guard.observe(&event));
            }
        }
        assert!(guard.observe(&AgentEvent::ReasoningDelta {
            session_id: "session".to_string(),
            text: "先核对设备与包状态。".repeat(500),
        }));
    }

    #[tokio::test]
    async fn repetitive_large_chunk_preserves_normal_prefix() {
        let events = stream::iter(vec![Ok(AgentEvent::TextDelta {
            session_id: "session".to_string(),
            text: format!("已经完成文件核对。\n{}", "先核对设备与包状态。".repeat(500)),
        })]);
        let (_sender, mut receiver) = mpsc::channel(1);
        let result = collect_model_round_with_observer(
            Box::pin(events),
            "session",
            &mut ToolStreamState::default(),
            &mut receiver,
            &AtomicBool::new(false),
            &[],
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(result.interruption.unwrap().code, "repetitive_output");
        assert!(result.text.starts_with("已经完成文件核对。\n"));
        assert!(result.text.chars().count() < 220);
    }

    #[tokio::test]
    async fn repetitive_stream_returns_recoverable_partial_text_and_pending_calls() {
        use std::sync::atomic::AtomicUsize;
        let consumed = std::sync::Arc::new(AtomicUsize::new(0));
        let mut events = vec![Ok(AgentEvent::ToolCallStarted {
            session_id: "session".to_string(),
            call_id: "call-1".to_string(),
            tool_index: None,
            tool_name: "read_local_file".to_string(),
            input: json!({"path": "a.txt"}),
        })];
        events.extend((0..500).map(|_| {
            Ok(AgentEvent::TextDelta {
                session_id: "session".to_string(),
                text: "先核对设备与包状态。".to_string(),
            })
        }));
        events.push(Ok(AgentEvent::Completed {
            session_id: "session".to_string(),
        }));
        let stream_consumed = consumed.clone();
        let events = stream::iter(events).inspect(move |_| {
            stream_consumed.fetch_add(1, Ordering::Relaxed);
        });
        let (_sender, mut steer_receiver) = mpsc::channel(1);
        let mut stream_state = ToolStreamState::default();
        let mut observed_text = String::new();
        let result = collect_model_round_with_observer(
            Box::pin(events),
            "session",
            &mut stream_state,
            &mut steer_receiver,
            &AtomicBool::new(false),
            &["read_local_file".to_string()],
            |event| {
                if let AgentEvent::TextDelta { text, .. } = event {
                    observed_text.push_str(text);
                }
            },
        )
        .await;
        let result = result.expect("repetitive output should return the usable partial round");
        let interruption = result
            .interruption
            .as_ref()
            .expect("repetition should be classified as a recoverable interruption");
        assert_eq!(interruption.code, "repetitive_output");
        assert!(interruption.retryable);
        assert_eq!(result.pending_calls.len(), 1);
        assert!(consumed.load(Ordering::Relaxed) < 25);
        assert!(!observed_text.is_empty());
        assert!(observed_text.chars().count() < 200);
        assert_eq!(result.text, observed_text);
    }

    #[tokio::test]
    async fn collects_text_and_tool_arguments_in_arrival_order() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "session".to_string(),
                text: "先说明。".to_string(),
            }),
            Ok(AgentEvent::ToolCallStarted {
                session_id: "session".to_string(),
                call_id: "call-1".to_string(),
                tool_index: Some(0),
                tool_name: "read_local_file".to_string(),
                input: serde_json::json!({}),
            }),
            Ok(AgentEvent::ToolCallInputDelta {
                session_id: "session".to_string(),
                call_id: "call-1".to_string(),
                tool_index: Some(0),
                delta: r#"{"path":"a.txt"}"#.to_string(),
            }),
            Ok(AgentEvent::Completed {
                session_id: "session".to_string(),
            }),
        ]);
        let (_sender, mut steer_receiver) = mpsc::channel(1);
        let mut stream_state = ToolStreamState::default();
        let result = collect_model_round(
            Box::pin(events),
            "session",
            &mut stream_state,
            &mut steer_receiver,
            &AtomicBool::new(false),
            &[
                "read_local_file".to_string(),
                "list_local_files".to_string(),
            ],
        )
        .await
        .expect("round");

        assert_eq!(result.text, "先说明。");
        assert_eq!(result.pending_calls.len(), 1);
        assert_eq!(result.pending_calls[0].0, "call-1");
        assert_eq!(result.pending_calls[0].1 .2, r#"{"path":"a.txt"}"#);
        assert!(matches!(
            result.events.first(),
            Some(AgentEvent::TextDelta { .. })
        ));
        assert!(!result.synthesized_plain_tool_calls);
    }

    #[tokio::test]
    async fn hides_plain_json_tool_payload_from_the_observer_result() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "session".to_string(),
                text: r#"[{"name":"read_local_file","arguments":{"path":"report.md"}}]"#
                    .to_string(),
            }),
            Ok(AgentEvent::Completed {
                session_id: "session".to_string(),
            }),
        ]);
        let (_sender, mut steer_receiver) = mpsc::channel(1);
        let mut stream_state = ToolStreamState::default();
        let result = collect_model_round(
            Box::pin(events),
            "session",
            &mut stream_state,
            &mut steer_receiver,
            &AtomicBool::new(false),
            &["read_local_file".to_string()],
        )
        .await
        .expect("round");

        assert!(result.synthesized_plain_tool_calls);
        assert!(result.text.is_empty());
        assert_eq!(result.pending_calls.len(), 1);
        assert!(result
            .events
            .iter()
            .all(|event| !matches!(event, AgentEvent::TextDelta { .. })));
    }

    #[tokio::test]
    async fn steer_does_not_cut_a_provider_stream_or_claim_its_pending_input() {
        let events = stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                session_id: "session".into(),
                text: "完整回答".into(),
            }),
            Ok(AgentEvent::Completed {
                session_id: "session".into(),
            }),
        ]);
        let (sender, mut steer_receiver) = mpsc::channel(1);
        sender
            .send(AgentSteerRequest {
                message: "补充检查这个文件".to_string(),
                attachments: Vec::new(),
                artifacts: Vec::new(),
                source_message_id: None,
            })
            .await
            .expect("steer");
        let mut stream_state = ToolStreamState::default();
        let result = collect_model_round(
            Box::pin(events),
            "session",
            &mut stream_state,
            &mut steer_receiver,
            &AtomicBool::new(false),
            &["read_local_file".to_string()],
        )
        .await
        .expect("round");
        assert!(result.steered.is_none());
        assert_eq!(result.text, "完整回答");
        assert_eq!(
            steer_receiver.try_recv().unwrap().message,
            "补充检查这个文件"
        );
        assert!(result.pending_calls.is_empty());
    }
}
