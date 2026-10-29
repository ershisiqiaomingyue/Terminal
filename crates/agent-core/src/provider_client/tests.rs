#[test]
fn compatible_reasoning_fields_and_explicit_channels_stay_out_of_body() {
    for field in ["reasoning_content", "reasoning", "thinking"] {
        let mut delta = serde_json::json!({"content":"中文正文"});
        delta[field] = serde_json::json!("English reasoning");
        let events = super::parse_openai_event(
            crate::ProviderKind::OpenAiCompatible,
            "s",
            &serde_json::json!({"choices":[{"delta":delta}]}),
        )
        .unwrap();
        assert!(events.iter().any(|event| matches!(event, crate::AgentEvent::ReasoningDelta { text, .. } if text == "English reasoning")));
        assert!(events.iter().any(
            |event| matches!(event, crate::AgentEvent::TextDelta { text, .. } if text == "中文正文")
        ));
    }
    let events = super::parse_openai_event(crate::ProviderKind::OpenAiCompatible, "s", &serde_json::json!({"choices":[{"delta":{"content":"English analysis", "channel":"analysis"}}]})).unwrap();
    assert!(
        matches!(&events[..], [crate::AgentEvent::ReasoningDelta { text, .. }] if text == "English analysis")
    );
    let events = super::parse_openai_event(
        crate::ProviderKind::OpenAiCompatible,
        "s",
        &serde_json::json!({"choices":[{"delta":{"content":"English evidence"}}]}),
    )
    .unwrap();
    assert!(
        matches!(&events[..], [crate::AgentEvent::TextDelta { text, .. }] if text == "English evidence")
    );
}
use super::{
    collect_stream_event, messages_endpoint, models_endpoint, parse_sse_data, request_body,
    ProviderImageAttachment, ProviderRequest, UsageSnapshot,
};
use crate::{
    AgentEvent, AgentImageAttachment, AgentMessage, AgentToolCall, MessageRole, ProviderConfig,
    ProviderKind, ToolDefinition,
};
use serde_json::{json, Value};

fn user_with_source(id: &str, content: &str) -> AgentMessage {
    let mut message = AgentMessage::user(content);
    message.source_message_id = Some(id.to_string());
    message
}

fn owned_image(owner: &str, name: &str, payload: &str) -> ProviderImageAttachment {
    ProviderImageAttachment {
        name: name.to_string(),
        media_type: "image/png".to_string(),
        data_url: format!("data:image/png;base64,{payload}"),
        source_message_id: owner.to_string(),
    }
}

fn multimodal_tool_messages() -> Vec<AgentMessage> {
    vec![
        user_with_source("user-initial", "inspect the first screenshot"),
        AgentMessage::assistant_tool_call(AgentToolCall {
            id: "call-image".to_string(),
            name: "read_local_file".to_string(),
            input: json!({ "path": "a.txt" }),
        }),
        AgentMessage::tool("call-image", "file contents"),
        user_with_source("user-steer-image", "also inspect the second screenshot"),
        user_with_source("user-steer-text", "now compare them"),
    ]
}

fn multimodal_tool_request() -> ProviderRequest {
    ProviderRequest::new(
        "session",
        "vision-model",
        multimodal_tool_messages(),
        Vec::new(),
    )
    .with_image_attachments(vec![
        owned_image("user-initial", "initial.png", "AQID"),
        owned_image("user-steer-image", "steer.png", "BAUG"),
    ])
}

fn provider(kind: ProviderKind) -> ProviderConfig {
    ProviderConfig::new(
        "provider",
        "Provider",
        kind,
        "https://relay.example/v1",
        "keyring:provider",
    )
    .expect("valid provider config")
}

async fn local_sse_events(
    kind: ProviderKind,
    frames: &[&str],
) -> Vec<Result<AgentEvent, super::ProviderClientError>> {
    local_sse_events_with_delay(
        kind,
        frames,
        std::time::Duration::ZERO,
        super::PROVIDER_IDLE_TIMEOUT,
    )
    .await
}

async fn local_sse_events_with_delay(
    kind: ProviderKind,
    frames: &[&str],
    header_delay: std::time::Duration,
    response_start_timeout: std::time::Duration,
) -> Vec<Result<AgentEvent, super::ProviderClientError>> {
    local_sse_events_with_transport(kind, frames, header_delay, response_start_timeout, false).await
}

async fn local_sse_events_with_transport(
    kind: ProviderKind,
    frames: &[&str],
    header_delay: std::time::Duration,
    response_start_timeout: std::time::Duration,
    hold_open: bool,
) -> Vec<Result<AgentEvent, super::ProviderClientError>> {
    local_sse_events_with_diagnostics(
        kind,
        frames,
        header_delay,
        response_start_timeout,
        hold_open,
        None,
    )
    .await
}

async fn local_sse_events_with_diagnostics(
    kind: ProviderKind,
    frames: &[&str],
    header_delay: std::time::Duration,
    response_start_timeout: std::time::Duration,
    hold_open: bool,
    diagnostic_observer: Option<super::ProviderDiagnosticObserver>,
) -> Vec<Result<AgentEvent, super::ProviderClientError>> {
    use futures_util::StreamExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let body = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>();
    let server = std::thread::spawn(move || {
        let started = Instant::now();
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        started.elapsed() < Duration::from_secs(5),
                        "HTTP fixture was not called"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("HTTP fixture accept failed: {error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 128 * 1024);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, length)| length.trim().parse::<usize>().unwrap())
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        std::thread::sleep(header_delay);
        let written = if hold_open {
            write!(socket, "HTTP/1.1 200 OK\r\nx-request-id: fixture-trace-1\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n", body.len(), body)
        } else {
            write!(socket, "HTTP/1.1 200 OK\r\nx-request-id: fixture-trace-1\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
        };
        if header_delay < response_start_timeout {
            written.unwrap();
        }
        if hold_open {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let _ = socket.read(&mut buffer);
        }
    });
    let config = ProviderConfig::new(
        "fixture",
        "Fixture",
        kind,
        format!("http://{address}/v1"),
        "fixture-only",
    )
    .unwrap();
    let client = super::HttpProviderClient {
        diagnostic_observer,
        diagnostic_sequence: Default::default(),
        cache_retention_rejected: Default::default(),
        config,
        api_key: "fixture-only".into(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        response_start_timeout,
    };
    let stream = client
        .stream(ProviderRequest::new(
            "s",
            "fixture",
            vec![AgentMessage::user("test")],
            vec![],
        ))
        .await
        .unwrap();
    let events = if hold_open {
        tokio::time::timeout(Duration::from_secs(1), stream.collect())
            .await
            .expect("protocol terminal should finish before HTTP EOF")
    } else {
        stream.collect().await
    };
    server.join().unwrap();
    events
}

#[tokio::test]
async fn every_outbound_model_request_carries_the_product_user_agent() {
    use futures_util::StreamExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    for kind in [
        ProviderKind::OpenAiCompatible,
        ProviderKind::OpenAiResponses,
        ProviderKind::Anthropic,
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            started.elapsed() < Duration::from_secs(5),
                            "HTTP fixture was not called"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("HTTP fixture accept failed: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = socket.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&request).to_string());
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        });

        let config = ProviderConfig::new(
            "fixture",
            "Fixture",
            kind,
            format!("http://{address}/v1"),
            "fixture-only",
        )
        .unwrap();
        let client = super::HttpProviderClient {
            diagnostic_observer: None,
            diagnostic_sequence: Default::default(),
            cache_retention_rejected: Default::default(),
            config,
            api_key: "fixture-only".into(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            response_start_timeout: super::PROVIDER_IDLE_TIMEOUT,
        };
        let stream = client
            .stream(ProviderRequest::new(
                "s",
                "fixture",
                vec![AgentMessage::user("test")],
                vec![],
            ))
            .await
            .unwrap();
        let _: Vec<_> = stream.collect().await;

        let request = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("fixture 没收到请求");
        let user_agent = request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
            .map(|(_, value)| value.trim().to_string());
        assert_eq!(
            user_agent.as_deref(),
            Some(super::PROVIDER_USER_AGENT),
            "{kind:?} 请求没有带上产品 User-Agent"
        );
        assert_eq!(super::PROVIDER_USER_AGENT, "Android Toolkit");
    }
}

#[tokio::test]
async fn transport_diagnostics_distinguish_reasoning_only_completion_and_broken_streams() {
    for (kind, frames, reason, failed, expected_text, expected_reasoning) in [
        (
            ProviderKind::OpenAiCompatible,
            vec![
                r#"{"choices":[{"delta":{"reasoning_content":"private thoughts"},"finish_reason":""}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ],
            Some("stop"),
            false,
            0,
            16,
        ),
        // A stream that just ends — no `[DONE]`, no finish reason — is the
        // broken case now that `[DONE]` itself is a terminal marker.
        (
            ProviderKind::OpenAiCompatible,
            vec![r#"{"choices":[{"delta":{"content":"private body"},"finish_reason":null}]}"#],
            None,
            true,
            12,
            0,
        ),
        // An unrecognized but identifier-shaped reason finishes the response
        // normally and stays visible as the bounded terminal_reason trace.
        (
            ProviderKind::OpenAiCompatible,
            vec![
                r#"{"choices":[{"delta":{"content":"private body"},"finish_reason":"future_stop"}]}"#,
                "[DONE]",
            ],
            Some("future_stop"),
            false,
            12,
            0,
        ),
        (
            ProviderKind::OpenAiCompatible,
            vec![
                r#"{"choices":[{"delta":{"content":"private body"},"finish_reason":null}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                "[DONE]",
            ],
            Some("length"),
            false,
            12,
            0,
        ),
        (
            ProviderKind::Anthropic,
            vec![
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"private thoughts"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
                r#"{"type":"message_stop"}"#,
            ],
            Some("end_turn"),
            false,
            0,
            16,
        ),
        (
            ProviderKind::OpenAiResponses,
            vec![
                r#"{"type":"response.reasoning_text.delta","delta":"private thoughts"}"#,
                r#"{"type":"response.completed","response":{"status":"completed"}}"#,
            ],
            Some("completed"),
            false,
            0,
            16,
        ),
    ] {
        let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = records.clone();
        let events = local_sse_events_with_diagnostics(
            kind,
            &frames,
            std::time::Duration::ZERO,
            super::PROVIDER_IDLE_TIMEOUT,
            false,
            Some(std::sync::Arc::new(move |data| {
                captured.lock().unwrap().push(data)
            })),
        )
        .await;
        assert_eq!(events.iter().any(Result::is_err), failed, "{kind:?}");
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1, "accepted content must not be replayed");
        let record = &records[0];
        assert_eq!(record.http_status, Some(200));
        assert_eq!(record.request_id.as_deref(), Some("fixture-trace-1"));
        assert_eq!(record.terminal_reason.as_deref(), reason);
        assert_eq!(record.saw_protocol_terminal, !failed);
        assert_eq!(record.failure_code.is_some(), failed);
        assert!(!record.retry_scheduled);
        assert_eq!(record.wire_text_characters, expected_text);
        assert_eq!(record.wire_reasoning_characters, expected_reasoning);
        assert!(record.received_bytes > 0);
        assert!(record.header_wait_ms.is_some());
        assert!(!serde_json::to_string(&*records)
            .unwrap()
            .contains("private"));
    }
}

#[tokio::test]
async fn compatible_empty_finish_reason_streams_until_real_terminal() {
    for terminal in ["stop", "tool_calls", "length"] {
        let finish = json!({"choices":[{"delta":{},"finish_reason":terminal}],
            "usage":{"prompt_tokens":20,"completion_tokens":34}})
        .to_string();
        let events = local_sse_events(ProviderKind::OpenAiCompatible, &[
            r#"{"choices":[{"delta":{"role":"assistant","content":""},"finish_reason":""}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"检查请求"},"finish_reason":""}]}"#,
            r#"{"choices":[{"delta":{"content":"OK"},"finish_reason":""}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"inspect","arguments":"{}"}}]},"finish_reason":""}]}"#,
            &finish, "[DONE]",
        ]).await;
        assert!(events.iter().all(Result::is_ok));
        assert!(events
            .iter()
            .any(|event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "OK")));
        assert!(events.iter().any(|event| matches!(event, Ok(AgentEvent::ReasoningDelta { text, .. }) if text == "检查请求")));
        assert!(events.iter().any(|event| matches!(event, Ok(AgentEvent::ToolCallStarted { tool_name, .. }) if tool_name == "inspect")));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(AgentEvent::Usage {
                input_tokens: Some(20),
                output_tokens: Some(34),
                ..
            })
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(AgentEvent::Completed { .. })))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::OutputTruncated { .. }))),
            terminal == "length"
        );
        let text_index = events
            .iter()
            .position(
                |event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "OK"),
            )
            .unwrap();
        let complete_index = events
            .iter()
            .position(|event| matches!(event, Ok(AgentEvent::Completed { .. })))
            .unwrap();
        assert!(complete_index > text_index);
    }
}

#[tokio::test]
async fn compatible_empty_finish_reason_cannot_hide_an_explicit_refusal() {
    for terminal in ["content_filter", "function_call"] {
        let terminal_frame = json!({"choices":[{"delta":{},"finish_reason":terminal}]}).to_string();
        let frames = [
            r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":""}]}"#,
            terminal_frame.as_str(),
            "[DONE]",
        ];
        let events = local_sse_events(ProviderKind::OpenAiCompatible, &frames).await;
        assert!(events.iter().any(
            |event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "partial")
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::Completed { .. }))),
            "{terminal}"
        );
        assert!(events.last().unwrap().is_err(), "{terminal}");
    }
}

#[tokio::test]
async fn compatible_stream_closed_by_done_completes_without_inventing_truncation() {
    // Neither a missing finish reason nor an unrecognized one may be turned
    // into a failure: `[DONE]` is the SSE-level end of the response, and the
    // provider never claimed truncation, so nothing may claim it either.
    for terminal in [None, Some("future_stop")] {
        let finish = terminal
            .map(|reason| json!({"choices":[{"delta":{},"finish_reason":reason}]}).to_string());
        let mut frames =
            vec![r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":""}]}"#];
        if let Some(frame) = finish.as_deref() {
            frames.push(frame);
        }
        frames.push("[DONE]");
        let events = local_sse_events(ProviderKind::OpenAiCompatible, &frames).await;
        assert!(events.iter().all(Result::is_ok), "{terminal:?}");
        assert!(events.iter().any(
            |event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "partial")
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(AgentEvent::Completed { .. })))
                .count(),
            1,
            "{terminal:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::OutputTruncated { .. }))),
            "{terminal:?}"
        );
    }
}

#[tokio::test]
async fn action_only_protocol_completion_reaches_shared_recovery() {
    use crate::agent_runtime::response_recovery::{
        ModelResponseRecovery, ProgressRecovery, ProgressRecoveryContext,
    };
    let text = "现在写布局和文案。";
    for (kind, frames) in [
        (
            ProviderKind::OpenAi,
            vec![
                json!({"choices":[{"delta":{"content":text},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ],
        ),
        (
            ProviderKind::OpenAiCompatible,
            vec![
                json!({"choices":[{"delta":{"content":text},"finish_reason":""}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ],
        ),
        (
            ProviderKind::DeepSeek,
            vec![
                json!({"choices":[{"delta":{"content":text},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
            ],
        ),
        (
            ProviderKind::OpenAiResponses,
            vec![
                json!({"type":"response.output_text.delta","delta":text}),
                json!({"type":"response.completed","response":{"status":"completed"}}),
            ],
        ),
        (
            ProviderKind::Anthropic,
            vec![
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
                json!({"type":"message_stop"}),
            ],
        ),
    ] {
        let strings: Vec<_> = frames.iter().map(Value::to_string).collect();
        let mut wire: Vec<_> = strings.iter().map(String::as_str).collect();
        wire.push("[DONE]");
        let events = local_sse_events(kind, &wire).await;
        let mut round = crate::AgentRoundState::default();
        for event in events {
            round.ingest(event.unwrap());
        }
        assert_eq!(round.text, text, "{kind:?}");
        assert!(round.pending_calls.is_empty());
        let mut response = AgentMessage::user(&round.text);
        response.role = crate::MessageRole::Assistant;
        let mut messages = vec![AgentMessage::tool("previous-write", "written")];
        assert_eq!(
            ModelResponseRecovery::default().retry_progress_only(
                &mut messages,
                &response,
                ProgressRecoveryContext {
                    tools_available: true,
                    final_only: false,
                    output_truncated: round.output_truncated,
                }
            ),
            ProgressRecovery::Retry,
            "{kind:?}"
        );
        assert_eq!(messages[0].tool_call_id.as_deref(), Some("previous-write"));
        assert!(messages.iter().all(|message| message.tool_calls.is_empty()));
    }
}

#[tokio::test]
async fn http_protocol_terminals_complete_without_waiting_for_transport_eof() {
    let cases: &[(ProviderKind, &[&str])] = &[
        (
            ProviderKind::OpenAi,
            &[
                r#"{"choices":[{"delta":{"content":"answer"},"finish_reason":null}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
                "[DONE]",
            ],
        ),
        (
            ProviderKind::Anthropic,
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"answer"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
                r#"{"type":"message_stop"}"#,
            ],
        ),
        (
            ProviderKind::OpenAiResponses,
            &[
                r#"{"type":"response.output_text.delta","delta":"answer"}"#,
                r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":3}}}"#,
            ],
        ),
    ];
    for (kind, frames) in cases {
        let events = local_sse_events_with_transport(
            *kind,
            frames,
            std::time::Duration::ZERO,
            super::PROVIDER_IDLE_TIMEOUT,
            true,
        )
        .await;
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(AgentEvent::Completed { .. })))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(AgentEvent::Usage {
                output_tokens: Some(3),
                ..
            })
        )));
    }
}

#[tokio::test]
async fn http_response_after_old_fifteen_second_deadline_completes_without_retry() {
    let events = local_sse_events_with_delay(
        ProviderKind::OpenAi,
        &[
            r#"{"choices":[{"delta":{"content":"delayed answer"},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ],
        std::time::Duration::from_secs(16),
        super::PROVIDER_IDLE_TIMEOUT,
    )
    .await;
    assert!(events.iter().all(Result::is_ok));
    assert!(events.iter().any(
        |event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "delayed answer")
    ));
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(AgentEvent::Completed { .. }))));
    assert!(!events.iter().any(
        |event| matches!(event, Ok(AgentEvent::Status { status, .. }) if status == "provider_retry")
    ));
}

#[tokio::test]
async fn http_response_deadline_does_not_replay_the_request() {
    let events = local_sse_events_with_delay(
        ProviderKind::OpenAi,
        &[],
        std::time::Duration::from_millis(200),
        std::time::Duration::from_millis(80),
    )
    .await;
    assert_eq!(events.len(), 1);
    let error = events[0].as_ref().unwrap_err();
    assert!(matches!(error, super::ProviderClientError::Timeout(_)));
    assert_eq!(error.code(), "provider_timeout");
    assert!(!super::provider_error_is_retryable(error));
}

#[tokio::test]
async fn http_done_closes_streams_that_never_sent_a_protocol_terminal() {
    // `[DONE]` ends the SSE response even when the relay never sent its own
    // terminal event, so content that already arrived is kept and the turn
    // completes instead of failing on a protocol technicality.
    for (kind, frame) in [
        (
            ProviderKind::OpenAi,
            r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}"#,
        ),
        (
            ProviderKind::OpenAiResponses,
            r#"{"type":"response.output_text.delta","delta":"partial"}"#,
        ),
        (
            ProviderKind::Anthropic,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
        ),
    ] {
        let events = local_sse_events(kind, &[frame, "[DONE]"]).await;
        assert!(events.iter().all(Result::is_ok), "{kind:?}");
        assert!(
            events.iter().any(
                |event| matches!(event, Ok(AgentEvent::TextDelta { text, .. }) if text == "partial")
            ),
            "{kind:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(AgentEvent::Completed { .. })))
                .count(),
            1,
            "{kind:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::OutputTruncated { .. }))),
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn http_chat_terminal_reasons_reach_dsh_without_lost_usage_or_hidden_refusals() {
    use futures_util::StreamExt;
    for reason in [
        "stop",
        "length",
        "content_filter",
        "future_stop",
        "function_call",
    ] {
        let terminal = json!({"choices":[{"delta":{},"finish_reason":reason}],
                "usage":{"prompt_tokens":100,"completion_tokens":19}})
        .to_string();
        let events = local_sse_events(
            ProviderKind::OpenAi,
            &[
                r#"{"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}"#,
                &terminal,
                "[DONE]",
            ],
        )
        .await;
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = observed.clone();
        let chunks = crate::dsh_native_provider::encode_provider_stream(
            Box::pin(futures_util::stream::iter(events)),
            "s",
            ProviderKind::OpenAi,
            "fixture",
            std::sync::Arc::new(move |event| captured.lock().unwrap().push(event)),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(chunks.iter().any(|chunk| chunk
            .as_ref()
            .is_ok_and(|chunk| chunk["type"] == "text-delta" && chunk["text"] == "partial")));
        let finishes = chunks
            .iter()
            .filter_map(|chunk| chunk.as_ref().ok())
            .filter(|chunk| chunk["type"] == "finish")
            .collect::<Vec<_>>();
        // `future_stop` is unrecognized but is not a refusal: it finishes and is
        // reported as a plain stop, with no truncation invented for it.
        if matches!(reason, "stop" | "length" | "future_stop") {
            assert_eq!(finishes.len(), 1);
            assert_eq!(
                finishes[0]["reason"]["kind"],
                if reason == "length" {
                    "max-tokens"
                } else {
                    "stop"
                }
            );
        } else {
            assert!(finishes.is_empty());
            assert_eq!(
                chunks.last().unwrap().as_ref().unwrap_err().code,
                "provider_incomplete"
            );
        }
        let observed = observed.lock().unwrap();
        assert!(observed.iter().any(
            |event| event["event"]["type"] == "usage" && event["event"]["output_tokens"] == 19
        ));
    }
}

#[test]
fn openai_chat_keeps_images_on_their_original_users_across_tool_rounds() {
    let request = multimodal_tool_request();
    let estimated_image_tokens =
        crate::estimate_image_attachment_tokens(&request.image_attachments);
    let body =
        request_body(&provider(ProviderKind::OpenAi), &request).expect("OpenAI request body");
    let messages = body["messages"].as_array().expect("chat messages");

    assert_eq!(
        messages[0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,AQID"
    );
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(
        messages[3]["content"][1]["image_url"]["url"],
        "data:image/png;base64,BAUG"
    );
    assert!(messages[4]["content"].is_string());
    let serialized_image_count = messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "image_url")
        .count();
    assert_eq!(serialized_image_count, 2);
    assert_eq!(estimated_image_tokens, serialized_image_count * 1_024);
}

#[test]
fn responses_keeps_images_on_their_original_users_across_tool_rounds() {
    let body = request_body(
        &provider(ProviderKind::OpenAiResponses),
        &multimodal_tool_request(),
    )
    .expect("Responses request body");
    let input = body["input"].as_array().expect("Responses input");

    assert_eq!(
        input[0]["content"][1]["image_url"],
        "data:image/png;base64,AQID"
    );
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(
        input[3]["content"][1]["image_url"],
        "data:image/png;base64,BAUG"
    );
    assert_eq!(input[4]["content"].as_array().map(Vec::len), Some(1));
}

#[test]
fn failed_provider_frames_keep_reported_usage_without_fabricating_a_terminal_event() {
    for (kind, frame) in [
        (
            ProviderKind::Anthropic,
            json!({"type":"message_delta", "delta":{"stop_reason":"model_context_window_exceeded"}, "usage":{"output_tokens":19}}),
        ),
        (
            ProviderKind::Anthropic,
            json!({"type":"message_delta", "delta":{"stop_reason":"pause_turn"}, "usage":{"output_tokens":19}}),
        ),
        (
            ProviderKind::Anthropic,
            json!({"type":"message_delta", "delta":{"stop_reason":"future_stop"}, "usage":{"output_tokens":19}}),
        ),
        (
            ProviderKind::OpenAiResponses,
            json!({"type":"response.failed", "response":{"usage":{"output_tokens":19}}}),
        ),
        (
            ProviderKind::OpenAi,
            json!({"error":{"message":"failed"}, "usage":{"completion_tokens":19}}),
        ),
    ] {
        let mut latest = Some(UsageSnapshot {
            input_tokens: Some(100),
            cached_input_tokens: Some(30),
            ..Default::default()
        });
        assert!(super::remember_failed_usage(
            kind,
            "s",
            &frame.to_string(),
            &mut latest
        ));
        let event = latest.unwrap().into_event("s");
        assert!(matches!(
            event,
            AgentEvent::Usage {
                input_tokens: Some(100),
                output_tokens: Some(19),
                cached_input_tokens: Some(30),
                ..
            }
        ));
    }
    let mut unknown = None;
    assert!(!super::remember_failed_usage(
        ProviderKind::Anthropic,
        "s",
        "{\"type\":\"error\"}",
        &mut unknown
    ));
    assert!(unknown.is_none());
}

#[test]
fn chat_output_limits_are_truncated_across_compatible_protocols() {
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::OpenAiCompatible,
        ProviderKind::DeepSeek,
        ProviderKind::Kimi,
        ProviderKind::Glm,
        ProviderKind::MiMo,
    ] {
        let events = parse_sse_data(
            kind,
            "s",
            &json!({
                "choices":[{"delta":{},"finish_reason":"length"}],
                "usage":{"prompt_tokens":100,"completion_tokens":19}
            })
            .to_string(),
        )
        .unwrap();
        let mut round = crate::AgentRoundState::default();
        for event in events {
            round.ingest(event);
        }
        assert!(round.output_truncated, "{kind:?}");
        assert_eq!(round.output_tokens, Some(19));
    }
}

#[test]
fn chat_refusals_fail_closed_while_stop_like_finishes_complete() {
    for reason in ["content_filter", "function_call"] {
        let frame = json!({"choices":[{"delta":{},"finish_reason":reason}],
                "usage":{"prompt_tokens":100,"completion_tokens":19}});
        let error = parse_sse_data(ProviderKind::OpenAi, "s", &frame.to_string()).unwrap_err();
        assert!(
            matches!(&error, super::ProviderClientError::Incomplete { reason: actual, .. } if actual == reason)
        );
        assert!(!super::provider_error_is_retryable(&error));
        let mut usage = None;
        assert!(super::remember_failed_usage(
            ProviderKind::OpenAi,
            "s",
            &frame.to_string(),
            &mut usage
        ));
        assert_eq!(usage.unwrap().output_tokens, Some(19));
    }
    // `future_stop` is the unrecognized-but-finished case: it completes like
    // `stop`/`tool_calls` and never claims truncation.
    for reason in [
        json!("stop"),
        json!("tool_calls"),
        json!("future_stop"),
        json!(null),
    ] {
        let events = parse_sse_data(
            ProviderKind::OpenAi,
            "s",
            &json!({
                "choices":[{"delta":{},"finish_reason":reason}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Completed { .. })),
            !reason.is_null()
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::OutputTruncated { .. })));
    }
}

#[test]
fn anthropic_output_limit_is_not_a_normal_finish_and_context_limit_is_not_retried_blindly() {
    let events = parse_sse_data(ProviderKind::Anthropic, "s", &json!({
            "type":"message_delta", "delta":{"stop_reason":"max_tokens"}, "usage":{"output_tokens":4096}
        }).to_string()).unwrap();
    let mut round = crate::AgentRoundState::default();
    for event in events {
        round.ingest(event);
    }
    assert!(round.output_truncated);
    assert_eq!(round.output_tokens, Some(4096));
    let error = parse_sse_data(
        ProviderKind::Anthropic,
        "s",
        &json!({
            "type":"message_delta", "delta":{"stop_reason":"model_context_window_exceeded"}
        })
        .to_string(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "provider_incomplete");
    assert!(!super::provider_error_is_retryable(&error));
}

#[test]
fn anthropic_paused_or_unknown_terminal_reasons_never_claim_normal_completion() {
    for reason in ["pause_turn", "future_stop"] {
        let error = parse_sse_data(
            ProviderKind::Anthropic,
            "s",
            &json!({
                "type":"message_delta", "delta":{"stop_reason":reason}
            })
            .to_string(),
        )
        .unwrap_err();
        assert!(
            matches!(&error, super::ProviderClientError::Incomplete { reason: actual, .. } if actual == reason)
        );
        assert!(!super::provider_error_is_retryable(&error));
    }
    for reason in [
        json!("end_turn"),
        json!("stop_sequence"),
        json!("tool_use"),
        json!("refusal"),
        json!(null),
    ] {
        let events = parse_sse_data(
            ProviderKind::Anthropic,
            "s",
            &json!({
                "type":"message_delta", "delta":{"stop_reason":reason}, "usage":{"output_tokens":7}
            })
            .to_string(),
        )
        .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Usage {
                output_tokens: Some(7),
                ..
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            AgentEvent::Completed { .. } | AgentEvent::OutputTruncated { .. }
        )));
    }
}

#[test]
fn anthropic_full_content_replays_original_order_without_duplicating_ui_text() {
    let expected = vec![
        json!({"type":"text","text":"before"}),
        json!({"type":"thinking","thinking":"reason","signature":"opaque"}),
        json!({"type":"tool_use","id":"call-1","name":"read_file","input":{"path":"x"}}),
        json!({"type":"text","text":"after","citations":[{"type":"page_location","document_index":0,"start_page_number":1,"end_page_number":2,"cited_text":"quote"}]}),
    ];
    let frames = [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"before"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"reason"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"opaque"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"after"}}),
        json!({"type":"content_block_delta","index":3,"delta":{"type":"citations_delta","citation":expected[3]["citations"][0]}}),
        json!({"type":"content_block_stop","index":3}),
    ];
    let mut round = crate::AgentRoundState::default();
    let mut encoder = crate::dsh_provider::DshChunkEncoder::new(ProviderKind::Anthropic, "claude");
    for frame in frames {
        for event in parse_sse_data(ProviderKind::Anthropic, "session", &frame.to_string()).unwrap()
        {
            encoder.push(event.clone()).unwrap();
            let (event, emitted) = round.ingest(event);
            if matches!(event, AgentEvent::AnthropicThinkingFrame { .. }) {
                assert!(!emitted);
            }
        }
    }
    assert!(round.anthropic_content_blocks.is_empty());
    assert!(round.validate_anthropic_replay().is_err());
    for event in parse_sse_data(
        ProviderKind::Anthropic,
        "session",
        "{\"type\":\"message_stop\"}",
    )
    .unwrap()
    {
        encoder.push(event.clone()).unwrap();
        round.ingest(event);
    }
    assert_eq!(round.anthropic_content_blocks, expected);
    let chunks = encoder.finish().unwrap();
    assert_eq!(
        chunks.last().unwrap()["replayState"]["response"]["anthropicContentBlocks"],
        json!(expected)
    );
    let mut assistant = crate::AgentMessage::user("display text must not be re-added");
    assistant.role = MessageRole::Assistant;
    assistant.anthropic_content_blocks = round.anthropic_content_blocks;
    assistant.anthropic_thinking_blocks = round.anthropic_thinking_blocks;
    let assistant: crate::AgentMessage =
        serde_json::from_value(serde_json::to_value(assistant).unwrap()).unwrap();
    let request = ProviderRequest::new(
        "session",
        "claude",
        vec![assistant, crate::AgentMessage::tool("call-1", "result")],
        vec![],
    );
    let body = request_body(&provider(ProviderKind::Anthropic), &request).unwrap();
    let mut actual = body["messages"][0]["content"].as_array().unwrap().clone();
    for block in &mut actual {
        block.as_object_mut().unwrap().remove("cache_control");
    }
    assert_eq!(actual, expected);
}

#[test]
fn anthropic_replay_rejects_torn_unknown_and_duplicate_blocks() {
    for frames in [
        vec![
            json!({"type":"content_block_start","index":0,"content_block":"malformed"}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"unsafe"}}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"wrong block kind"}}),
            json!({"type":"content_block_stop","index":0}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"read_file","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
            json!({"type":"content_block_stop","index":0}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta","text":"unknown"}}),
            json!({"type":"content_block_stop","index":0}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"duplicate"}}),
            json!({"type":"content_block_stop","index":0}),
        ],
        vec![
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"missing index zero"}}),
            json!({"type":"content_block_stop","index":1}),
        ],
    ] {
        let mut round = crate::AgentRoundState::default();
        for frame in frames.into_iter().chain([json!({"type":"message_stop"})]) {
            for event in
                parse_sse_data(ProviderKind::Anthropic, "session", &frame.to_string()).unwrap()
            {
                round.ingest(event);
            }
        }
        assert!(round.anthropic_content_blocks.is_empty());
        assert!(round.validate_anthropic_replay().is_err());
    }
}

#[test]
fn anthropic_many_small_deltas_keep_full_content_and_initial_text() {
    let mut round = crate::AgentRoundState::default();
    for frame in [json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"initial-"}})]
            .into_iter().chain((0..10000).map(|_| json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"abc"}})))
            .chain([json!({"type":"content_block_stop","index":0}), json!({"type":"message_stop"})])
        {
            for event in parse_sse_data(ProviderKind::Anthropic, "s", &frame.to_string()).unwrap() { round.ingest(event); }
        }
    assert_eq!(round.text, format!("initial-{}", "abc".repeat(10000)));
    assert_eq!(round.anthropic_content_blocks[0]["text"], round.text);
    assert!(round.validate_anthropic_replay().is_ok());
}

#[test]
fn anthropic_replays_signed_and_redacted_thinking_verbatim_before_tools() {
    let frames = [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"private thought"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"opaque"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"encrypted-redacted"}}),
        json!({"type":"content_block_stop","index":1}),
    ];
    let mut round = crate::AgentRoundState::default();
    for frame in frames {
        for event in parse_sse_data(ProviderKind::Anthropic, "session", &frame.to_string()).unwrap()
        {
            let (event, emitted) = round.ingest(event);
            if matches!(event, AgentEvent::AnthropicThinkingFrame { .. }) {
                assert!(!emitted);
            }
        }
    }
    assert_eq!(round.reasoning_content, "private thought");
    assert_eq!(round.anthropic_thinking_blocks.len(), 2);
    let mut assistant = crate::AgentMessage::assistant_tool_call(crate::AgentToolCall {
        id: "call-1".into(),
        name: "read_file".into(),
        input: json!({}),
    });
    assistant.content = "commentary".into();
    assistant.reasoning_content = Some(round.reasoning_content);
    assistant.anthropic_thinking_blocks = round.anthropic_thinking_blocks.clone();
    let request = ProviderRequest::new(
        "session",
        "claude",
        vec![assistant, crate::AgentMessage::tool("call-1", "result")],
        vec![],
    );
    let body = request_body(&provider(ProviderKind::Anthropic), &request).unwrap();
    assert_eq!(
        body["messages"][0]["content"][0],
        round.anthropic_thinking_blocks[0]
    );
    assert_eq!(
        body["messages"][0]["content"][1],
        round.anthropic_thinking_blocks[1]
    );
    assert_eq!(body["messages"][0]["content"][2]["type"], "text");
    assert_eq!(body["messages"][0]["content"][3]["type"], "tool_use");
}

#[test]
fn anthropic_never_fabricates_unsigned_thinking_from_historical_display_text() {
    let mut assistant = crate::AgentMessage::assistant_tool_call(crate::AgentToolCall {
        id: "call-1".into(),
        name: "read_file".into(),
        input: json!({}),
    });
    assistant.reasoning_content = Some("legacy thought without a signature".into());
    let request = ProviderRequest::new(
        "session",
        "claude",
        vec![assistant, crate::AgentMessage::tool("call-1", "result")],
        vec![],
    );
    let body = request_body(&provider(ProviderKind::Anthropic), &request).unwrap();
    assert!(body["messages"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .all(|block| block["type"] != "thinking"));
}

#[test]
fn incomplete_claude_signatures_are_not_replayed() {
    let mut round = crate::AgentRoundState::default();
    for frame in [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"partial"}}),
        json!({"type":"content_block_stop","index":0}),
    ] {
        for event in parse_sse_data(ProviderKind::Anthropic, "session", &frame.to_string()).unwrap()
        {
            round.ingest(event);
        }
    }
    assert!(round.anthropic_thinking_blocks.is_empty());
    assert_eq!(round.reasoning_content, "partial");
}

#[test]
fn anthropic_keeps_owned_images_and_grouped_tool_results() {
    let body = request_body(
        &provider(ProviderKind::Anthropic),
        &multimodal_tool_request(),
    )
    .expect("Anthropic request body");
    let messages = body["messages"].as_array().expect("Anthropic messages");

    assert_eq!(messages[0]["content"][1]["source"]["data"], "AQID");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[3]["content"][1]["source"]["data"], "BAUG");
    assert!(messages[4]["content"][0]["cache_control"].is_object());
}

#[test]
fn rejects_an_image_without_its_original_user_message() {
    let request = ProviderRequest::new(
        "session",
        "vision-model",
        vec![user_with_source("another-user", "hello")],
        Vec::new(),
    )
    .with_image_attachments(vec![owned_image("missing-user", "orphan.png", "AQID")]);

    assert!(request_body(&provider(ProviderKind::OpenAi), &request)
        .expect_err("orphan image must be rejected")
        .contains("no matching user message owner"));
}

#[test]
fn unknown_vision_capability_reaches_provider_but_explicit_false_is_rejected() {
    let image = AgentImageAttachment {
        path: None,
        name: "screen.png".to_string(),
        media_type: "image/png".to_string(),
        data_url: "data:image/png;base64,AQID".to_string(),
    };
    let attachments =
        crate::normalize_image_attachments(std::slice::from_ref(&image), None, "user-image")
            .expect("an unknown capability must allow the provider to decide");
    let request = ProviderRequest::new(
        "session",
        "unprobed-model",
        vec![user_with_source("user-image", "inspect this image")],
        Vec::new(),
    )
    .with_image_attachments(attachments);

    let body = request_body(&provider(ProviderKind::OpenAi), &request)
        .expect("the image should reach provider serialization");
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,AQID"
    );
    assert!(crate::normalize_image_attachments(
        std::slice::from_ref(&image),
        Some(false),
        "user-image",
    )
    .expect_err("an explicitly unsupported capability must reject locally")
    .contains("不支持图片输入"));
}

#[test]
fn repetitive_output_is_not_automatically_retried() {
    let error = super::ProviderClientError::Repetition("repeated response".to_string());
    assert_eq!(error.code(), "repetitive_output");
    assert!(!super::provider_error_is_retryable(&error));
}

#[test]
fn server_failures_get_one_retry_and_context_limit_errors_return_for_compaction() {
    use super::{ProviderClientError, ProviderStatusError};

    let transient = ProviderClientError::Status {
        kind: ProviderStatusError::InternalServerError,
        message: "HTTP 500: upstream unavailable".into(),
    };
    assert!(super::provider_error_is_retryable(&transient));
    assert_eq!(super::provider_retry_limit(&transient), 2);

    let context_limit = ProviderClientError::Status {
        kind: ProviderStatusError::InternalServerError,
        message: "HTTP 500: maximum context length exceeded".into(),
    };
    assert!(super::provider_error_is_retryable(&context_limit));
    assert_eq!(super::provider_retry_limit(&context_limit), 1);

    let gateway_unavailable = ProviderClientError::Status {
        kind: ProviderStatusError::from_status(503),
        message: "HTTP 503: temporarily unavailable".into(),
    };
    assert_eq!(super::provider_retry_limit(&gateway_unavailable), 5);
}

#[test]
fn builds_model_discovery_endpoints_from_provider_base_urls() {
    assert_eq!(
        models_endpoint(ProviderKind::OpenAiCompatible, "https://relay.example/v1").unwrap(),
        "https://relay.example/v1/models"
    );
    assert_eq!(
        models_endpoint(ProviderKind::Glm, "https://open.bigmodel.cn/api/paas/v4").unwrap(),
        "https://open.bigmodel.cn/api/paas/v4/models"
    );
    assert!(models_endpoint(ProviderKind::OpenAi, "relay.example/v1").is_err());
}

#[test]
fn builds_message_endpoints_for_anthropic_and_openai_families() {
    assert_eq!(
        messages_endpoint(ProviderKind::Anthropic, "https://relay.example/anthropic"),
        "https://relay.example/anthropic/v1/messages"
    );
    assert_eq!(
        messages_endpoint(ProviderKind::OpenAiResponses, "https://api.openai.com/v1"),
        "https://api.openai.com/v1/responses"
    );
}

#[test]
fn caches_anthropic_current_user_and_prior_history() {
    let config = ProviderConfig::new(
        "provider",
        "Provider",
        ProviderKind::Anthropic,
        "https://relay.example/anthropic",
        "keyring:provider",
    )
    .expect("valid provider config");
    let history = AgentMessage {
        role: MessageRole::Assistant,
        content: "previous answer".to_string(),
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        source_message_id: None,
    };
    let request = ProviderRequest::new(
        "session",
        "claude-sonnet",
        vec![history, AgentMessage::user("current request")],
        Vec::new(),
    );
    let body = request_body(&config, &request).expect("request body");
    let messages = body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .expect("anthropic messages");
    assert!(messages[0]["content"][0]["cache_control"].is_object());
    assert!(messages[1]["content"][0]["cache_control"].is_object());
}

#[test]
fn moves_anthropic_breakpoint_after_tool_results_for_the_next_turn() {
    let config = ProviderConfig::new(
        "provider",
        "Provider",
        ProviderKind::Anthropic,
        "https://relay.example/anthropic",
        "keyring:provider",
    )
    .expect("valid provider config");
    let tool_call = AgentToolCall {
        id: "call-1".to_string(),
        name: "read_local_file".to_string(),
        input: json!({ "path": "a.txt" }),
    };
    let request = ProviderRequest::new(
        "session",
        "claude-sonnet",
        vec![
            AgentMessage::user("inspect a.txt"),
            AgentMessage::assistant_tool_call(tool_call),
            AgentMessage::tool("call-1", "file contents"),
            AgentMessage::user("now summarize it"),
        ],
        Vec::new(),
    );
    let body = request_body(&config, &request).expect("request body");
    let messages = body["messages"].as_array().expect("anthropic messages");

    assert!(messages[0]["content"]["cache_control"].is_null());
    assert!(messages[1]["content"][0]["cache_control"].is_null());
    assert!(messages[2]["content"][0]["cache_control"].is_object());
    assert!(messages[3]["content"][0]["cache_control"].is_object());
}

#[test]
fn caches_anthropic_tool_result_when_the_next_request_is_a_tool_continuation() {
    let config = ProviderConfig::new(
        "provider",
        "Provider",
        ProviderKind::Anthropic,
        "https://relay.example/anthropic",
        "keyring:provider",
    )
    .expect("valid provider config");
    let request = ProviderRequest::new(
        "session",
        "claude-sonnet",
        vec![
            AgentMessage::user("inspect a.txt"),
            AgentMessage::assistant_tool_call(AgentToolCall {
                id: "call-1".to_string(),
                name: "read_local_file".to_string(),
                input: json!({ "path": "a.txt" }),
            }),
            AgentMessage::tool("call-1", "file contents"),
        ],
        Vec::new(),
    );
    let body = request_body(&config, &request).expect("request body");
    let messages = body["messages"].as_array().expect("anthropic messages");
    assert!(messages[0]["content"]["cache_control"].is_null());
    assert!(messages[2]["content"][0]["cache_control"].is_object());
}

#[test]
fn anthropic_runtime_controls_keep_the_cached_system_and_tool_prefix_stable() {
    let config = provider(ProviderKind::Anthropic);
    let tool = ToolDefinition::new("read_local_file", "read file", json!({"type":"object"}));
    let mut request = ProviderRequest::new(
        "s",
        "claude-sonnet",
        vec![
            AgentMessage::system("stable leading constraints"),
            AgentMessage::user("inspect source.cpp"),
            AgentMessage::assistant_tool_call(AgentToolCall {
                id: "read".into(),
                name: "read_local_file".into(),
                input: json!({"path":"source.cpp"}),
            }),
            AgentMessage::tool("read", "read failed"),
        ],
        vec![tool],
    )
    .with_system_prompt("stable instructions");
    request.messages[2].anthropic_content_blocks = vec![
        json!({"type":"thinking","thinking":"inspect the source","signature":"signed-original"}),
        json!({"type":"text","text":"I will read the file."}),
        json!({"type":"tool_use","id":"read","name":"read_local_file","input":{"path":"source.cpp"}}),
    ];
    let before = request_body(&config, &request).unwrap();
    let mut next = request;
    next.messages.push(AgentMessage::system(
        "The file read failed; adjust the path and continue.",
    ));
    let after = request_body(&config, &next).unwrap();
    assert_eq!(before["system"], after["system"]);
    assert_eq!(before["tools"], after["tools"]);
    assert_eq!(
        before["messages"][2]["content"][0]["content"],
        after["messages"][2]["content"][0]["content"]
    );
    assert_eq!(before["messages"][1], after["messages"][1]);
    assert_eq!(
        after["messages"][1]["content"][0]["signature"],
        "signed-original"
    );
    assert_eq!(after["messages"][3]["role"], "user");
    assert!(after["messages"][3]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("adjust the path"));
    assert_eq!(after["system"].as_array().unwrap().len(), 2);
}

#[test]
fn anthropic_large_parallel_tool_group_retains_prompt_and_result_cache_anchors() {
    let calls = (0..25)
        .map(|i| AgentToolCall {
            id: format!("read-{i}"),
            name: "read_local_file".into(),
            input: json!({"path":format!("file-{i}.cpp")}),
        })
        .collect::<Vec<_>>();
    let mut assistant = AgentMessage::assistant_tool_call(calls[0].clone());
    assistant.tool_calls = calls.clone();
    let mut messages = vec![AgentMessage::user("inspect these files"), assistant];
    messages.extend(
        calls
            .into_iter()
            .map(|call| AgentMessage::tool(call.id, "file content")),
    );
    let request = ProviderRequest::new(
        "s",
        "claude-sonnet",
        messages,
        vec![ToolDefinition::new(
            "read_local_file",
            "read file",
            json!({"type":"object"}),
        )],
    )
    .with_system_prompt("stable instructions");
    let body = request_body(&provider(ProviderKind::Anthropic), &request).unwrap();
    assert!(body["messages"][0]["content"][0]["cache_control"].is_object());
    assert!(body["messages"][2]["content"][24]["cache_control"].is_object());
    assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 25);
    fn count_marks(value: &Value) -> usize {
        match value {
            Value::Object(map) => {
                usize::from(map.contains_key("cache_control"))
                    + map.values().map(count_marks).sum::<usize>()
            }
            Value::Array(values) => values.iter().map(count_marks).sum(),
            _ => 0,
        }
    }
    assert_eq!(count_marks(&body), 4);
}

#[test]
fn sends_native_reasoning_parameters_for_each_supported_protocol() {
    let responses = ProviderConfig::new(
        "responses",
        "Responses",
        ProviderKind::OpenAiResponses,
        "https://api.openai.com/v1",
        "keyring:responses",
    )
    .expect("valid Responses provider");
    let responses_body = request_body(
        &responses,
        &ProviderRequest::new(
            "session",
            "o3",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_effort(Some("high".to_string())),
    )
    .expect("Responses request body");
    assert_eq!(responses_body["reasoning"]["effort"], "high");

    let openai = ProviderConfig::new(
        "openai",
        "OpenAI",
        ProviderKind::OpenAi,
        "https://api.openai.com/v1",
        "keyring:openai",
    )
    .expect("valid OpenAI provider");
    let openai_body = request_body(
        &openai,
        &ProviderRequest::new(
            "session",
            "o3",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_effort(Some("low".to_string())),
    )
    .expect("OpenAI request body");
    assert_eq!(openai_body["reasoning_effort"], "low");

    let anthropic = ProviderConfig::new(
        "anthropic",
        "Anthropic",
        ProviderKind::Anthropic,
        "https://api.anthropic.com/v1",
        "keyring:anthropic",
    )
    .expect("valid Anthropic provider");
    let anthropic_body = request_body(
        &anthropic,
        &ProviderRequest::new(
            "session",
            "claude-sonnet-4",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_max_tokens(8_192)
        .with_reasoning_effort(Some("medium".to_string())),
    )
    .expect("Anthropic request body");
    assert_eq!(anthropic_body["thinking"]["type"], "enabled");
    assert_eq!(anthropic_body["thinking"]["budget_tokens"], 4_096);
}

#[test]
fn modern_claude_uses_adaptive_effort_without_a_legacy_budget() {
    let config = ProviderConfig::new(
        "anthropic",
        "Anthropic",
        ProviderKind::Anthropic,
        "https://api.anthropic.com/v1",
        "keyring:anthropic",
    )
    .unwrap();
    for model in [
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-mythos-preview",
        "claude-opus-4-6-20260205",
        "claude-opus-4.8",
    ] {
        let request =
            ProviderRequest::new("s", model, vec![AgentMessage::user("hello")], Vec::new())
                .with_reasoning_effort(Some("max".into()));
        let body = request_body(&config, &request).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive", "{model}");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert!(body["thinking"].get("budget_tokens").is_none());
        assert_eq!(body["output_config"]["effort"], "max");
    }
    for model in [
        "claude-sonnet-4-5",
        "claude-opus-4-5",
        "relay-alias",
        "claude-opus-5-fake",
    ] {
        let request =
            ProviderRequest::new("s", model, vec![AgentMessage::user("hello")], Vec::new())
                .with_reasoning_effort(Some("medium".into()))
                .with_max_tokens(8192);
        let body = request_body(&config, &request).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled", "{model}");
        assert_eq!(body["thinking"]["budget_tokens"], 4096);
    }
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "s",
            "claude-sonnet-4-6",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        ),
    )
    .unwrap();
    assert!(
        body.get("thinking").is_none(),
        "do not turn thinking on without a user setting"
    );
}

#[test]
fn sends_responses_system_prompt_as_instructions_without_storage() {
    let config = ProviderConfig::new(
        "responses",
        "Responses",
        ProviderKind::OpenAiResponses,
        "https://relay.example/v1",
        "keyring:responses",
    )
    .expect("valid Responses provider");
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "gpt-5.6-luna",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_system_prompt("stable instructions"),
    )
    .expect("Responses request body");

    assert_eq!(body["instructions"], "stable instructions");
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
    assert_eq!(body["input"][0]["role"], "user");
}

#[test]
fn keeps_optional_fields_scoped_to_their_protocol() {
    let compatible = ProviderConfig::new(
        "compatible",
        "Compatible",
        ProviderKind::OpenAiCompatible,
        "https://relay.example/v1",
        "keyring:compatible",
    )
    .expect("valid compatible provider");
    let compatible_body = request_body(
        &compatible,
        &ProviderRequest::new(
            "session",
            "relay-model",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        ),
    )
    .expect("compatible request body");
    assert_eq!(compatible_body["prompt_cache_key"], "session");
    assert_eq!(compatible_body["stream_options"]["include_usage"], true);

    let responses = ProviderConfig::new(
        "responses",
        "Responses",
        ProviderKind::OpenAiResponses,
        "https://relay.example/v1",
        "keyring:responses",
    )
    .expect("valid Responses provider");
    let responses_body = request_body(
        &responses,
        &ProviderRequest::new(
            "session",
            "relay-model",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_encrypted_content(false),
    )
    .expect("Responses request body");
    assert!(responses_body.get("include").is_none());
    assert_eq!(responses_body["store"], false);
}

#[test]
fn replays_responses_reasoning_items_before_tool_calls() {
    let config = ProviderConfig::new(
        "responses",
        "Responses",
        ProviderKind::OpenAiResponses,
        "https://relay.example/v1",
        "keyring:responses",
    )
    .expect("valid Responses provider");
    let reasoning = json!({
        "type": "reasoning",
        "id": "rs_1",
        "summary": [],
        "encrypted_content": "opaque"
    });
    let mut assistant = AgentMessage::assistant_tool_call(AgentToolCall {
        id: "call-1".to_string(),
        name: "read_local_file".to_string(),
        input: json!({ "path": "a.txt" }),
    });
    assistant.responses_reasoning_items.push(reasoning.clone());
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "gpt-5.6-luna",
            vec![assistant, AgentMessage::tool("call-1", "contents")],
            Vec::new(),
        ),
    )
    .expect("Responses request body");

    assert_eq!(body["input"][0], reasoning);
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][2]["type"], "function_call_output");
}

#[test]
fn does_not_replay_responses_reasoning_items_for_plain_assistant_turns() {
    let config = ProviderConfig::new(
        "responses",
        "Responses",
        ProviderKind::OpenAiResponses,
        "https://relay.example/v1",
        "keyring:responses",
    )
    .expect("valid Responses provider");
    let mut assistant = AgentMessage {
        role: MessageRole::Assistant,
        content: "previous answer".to_string(),
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        source_message_id: None,
    };
    assistant.responses_reasoning_items.push(json!({
        "type": "reasoning",
        "id": "rs_plain_1",
        "summary": [],
        "encrypted_content": "opaque"
    }));
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "gpt-5.6-luna",
            vec![assistant, AgentMessage::user("next request")],
            Vec::new(),
        ),
    )
    .expect("Responses request body");

    assert_eq!(body["input"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["input"][0]["role"], "assistant");
    assert_eq!(body["input"][0]["content"][0]["type"], "output_text");
    assert_eq!(body["input"][1]["role"], "user");
    assert!(body["input"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["type"] != "reasoning"));
}

#[test]
fn captures_responses_reasoning_item_events_without_rendering_text() {
    let events = parse_sse_data(
            ProviderKind::OpenAiResponses,
            "session",
            r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"}}"#,
        )
        .expect("Responses reasoning event");
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::ResponsesReasoningItem { item, .. }] if item["id"] == "rs_1"
    ));
}

#[test]
fn responses_output_channels_follow_item_metadata_and_explicit_final_override() {
    let kind = ProviderKind::OpenAiResponses;
    let mut channels = super::ResponsesOutputChannels::default();
    channels.remember(kind, &json!({"type":"response.output_item.added", "output_index":0, "item":{"id":"analysis-1", "type":"message", "channel":"analysis"}}).to_string());
    for reference in [json!({"item_id":"analysis-1"}), json!({"output_index":0})] {
        let mut frame = json!({"type":"response.output_text.delta", "delta":"internal planning"});
        frame
            .as_object_mut()
            .unwrap()
            .extend(reference.as_object().unwrap().clone());
        let wire = frame.to_string();
        let event = parse_sse_data(kind, "session", &wire).unwrap().remove(0);
        assert!(matches!(
            channels.route(kind, &wire, event),
            AgentEvent::ReasoningDelta { .. }
        ));
    }
    let final_frame = json!({"type":"response.output_text.delta", "item_id":"analysis-1", "channel":"final", "delta":"English final answer"}).to_string();
    let event = parse_sse_data(kind, "session", &final_frame)
        .unwrap()
        .remove(0);
    assert!(matches!(
        channels.route(kind, &final_frame, event),
        AgentEvent::TextDelta { .. }
    ));
    channels.remember(kind, &json!({"type":"response.output_item.added", "output_index":1, "item":{"id":"final-1", "type":"message", "channel":"final"}}).to_string());
    let public = json!({"type":"response.output_text.delta", "item_id":"final-1", "delta":"Public English prose"}).to_string();
    let event = parse_sse_data(kind, "session", &public).unwrap().remove(0);
    assert!(matches!(
        channels.route(kind, &public, event),
        AgentEvent::TextDelta { .. }
    ));
}

#[test]
fn responses_reasoning_deltas_do_not_become_public_text_or_replay_done_summaries() {
    for event_type in [
        "response.reasoning_text.delta",
        "response.reasoning_summary_text.delta",
    ] {
        let wire = json!({"type":event_type,"delta":"planning"}).to_string();
        assert!(
            matches!(parse_sse_data(ProviderKind::OpenAiResponses, "session", &wire).unwrap().as_slice(), [AgentEvent::ReasoningDelta { text, .. }] if text == "planning")
        );
    }
    assert!(parse_sse_data(
        ProviderKind::OpenAiResponses,
        "session",
        r#"{"type":"response.reasoning_summary_text.done","text":"planning"}"#
    )
    .unwrap()
    .is_empty());
}

#[test]
fn omits_invalid_reasoning_effort_instead_of_sending_unknown_values() {
    let config = ProviderConfig::new(
        "openai",
        "OpenAI",
        ProviderKind::OpenAiResponses,
        "https://api.openai.com/v1",
        "keyring:openai",
    )
    .expect("valid provider");
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "custom",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_effort(Some("unsupported".to_string())),
    )
    .expect("request body");
    assert!(body.get("reasoning").is_none());
}

#[test]
fn sends_the_recovery_effort_that_turns_reasoning_off() {
    let config = ProviderConfig::new(
        "openai",
        "OpenAI",
        ProviderKind::OpenAiResponses,
        "https://api.openai.com/v1",
        "keyring:openai",
    )
    .expect("valid provider");
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "custom",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_effort(Some("minimal".to_string())),
    )
    .expect("request body");
    // 恢复轮靠这个档位在物理上把思考关掉（Responses 语义下 `minimal` 等同关断），
    // 所以它不能被上面那条通用白名单悄悄丢掉。
    assert_eq!(body["reasoning"]["effort"], "minimal");
}

#[test]
fn never_sends_the_recovery_effort_to_anthropic() {
    let config = ProviderConfig::new(
        "anthropic",
        "Anthropic",
        ProviderKind::Anthropic,
        "https://api.anthropic.com/v1",
        "keyring:anthropic",
    )
    .expect("valid provider");
    let body = request_body(
        &config,
        &ProviderRequest::new(
            "session",
            "claude-fable-5",
            vec![AgentMessage::user("hello")],
            Vec::new(),
        )
        .with_reasoning_effort(Some("minimal".to_string())),
    )
    .expect("request body");
    // Anthropic 只认标准 5 档，未知档位塞进 `output_config.effort` 会被 400 顶回，
    // 所以这一侧干脆不发。
    assert!(body.get("thinking").is_none());
    assert!(body.get("output_config").is_none());
}

#[test]
fn uses_a_stable_cache_key_for_openai_and_compatible_protocols() {
    let messages = vec![AgentMessage::user("hello")];
    let openai = ProviderConfig::new(
        "openai",
        "OpenAI",
        ProviderKind::OpenAi,
        "https://api.openai.com/v1",
        "keyring:openai",
    )
    .expect("valid provider config");
    let body = request_body(
        &openai,
        &ProviderRequest::new("stable-session", "gpt-4.1", messages.clone(), Vec::new()),
    )
    .expect("OpenAI request body");
    assert_eq!(body["prompt_cache_key"], "stable-session");

    let compatible = ProviderConfig::new(
        "relay",
        "Relay",
        ProviderKind::OpenAiCompatible,
        "https://relay.example/v1",
        "keyring:relay",
    )
    .expect("valid provider config");
    let body = request_body(
        &compatible,
        &ProviderRequest::new("stable-session", "custom", messages, Vec::new()),
    )
    .expect("compatible request body");
    assert_eq!(body["prompt_cache_key"], "stable-session");
}

#[test]
fn parses_anthropic_split_usage_without_double_counting_the_round() {
    let start = parse_sse_data(
            ProviderKind::Anthropic,
            "session",
            r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":700,"cache_creation_input_tokens":200}}}"#,
        )
        .expect("message start");
    let delta = parse_sse_data(
        ProviderKind::Anthropic,
        "session",
        r#"{"type":"message_delta","usage":{"output_tokens":40}}"#,
    )
    .expect("message delta");
    assert!(matches!(
        start.as_slice(),
        [AgentEvent::Usage {
            input_tokens: Some(100),
            cached_input_tokens: Some(700),
            cache_creation_input_tokens: Some(200),
            ..
        }]
    ));
    assert!(matches!(
        delta.as_slice(),
        [AgentEvent::Usage {
            input_tokens: None,
            output_tokens: Some(40),
            ..
        }]
    ));
}

#[test]
fn stream_usage_snapshot_uses_the_latest_partial_fields_once() {
    let start = parse_sse_data(
            ProviderKind::Anthropic,
            "session",
            r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":700}}}"#,
        )
        .expect("message start");
    let delta = parse_sse_data(
        ProviderKind::Anthropic,
        "session",
        r#"{"type":"message_delta","usage":{"output_tokens":40}}"#,
    )
    .expect("message delta");
    let mut latest = None::<UsageSnapshot>;
    let mut terminal = false;
    for event in start.into_iter().chain(delta) {
        let _ = collect_stream_event(event, &mut latest, &mut terminal);
    }
    let AgentEvent::Usage {
        input_tokens,
        output_tokens,
        cached_input_tokens,
        ..
    } = latest.expect("usage snapshot").into_event("session")
    else {
        panic!("expected usage event");
    };
    assert_eq!(input_tokens, Some(100));
    assert_eq!(output_tokens, Some(40));
    assert_eq!(cached_input_tokens, Some(700));
}

#[test]
fn parses_openai_chat_cache_usage_and_relay_miss_usage() {
    let standard = parse_sse_data(
            ProviderKind::OpenAi,
            "session",
            r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":700}}}"#,
        )
        .expect("OpenAI usage");
    assert!(matches!(
        standard.as_slice(),
        [AgentEvent::Usage {
            input_tokens: Some(1000),
            output_tokens: Some(40),
            cached_input_tokens: Some(700),
            ..
        }]
    ));

    let relay = parse_sse_data(
            ProviderKind::OpenAi,
            "session",
            r#"{"choices":[],"usage":{"prompt_tokens_details":{"cached_tokens":700,"prompt_cache_miss_tokens":300},"completion_tokens":40}}"#,
        )
        .expect("relay usage");
    assert!(matches!(
        relay.as_slice(),
        [AgentEvent::Usage {
            input_tokens: Some(1000),
            output_tokens: Some(40),
            cached_input_tokens: Some(700),
            ..
        }]
    ));
}

#[test]
fn parses_responses_input_cache_usage() {
    let events = parse_sse_data(
            ProviderKind::OpenAiResponses,
            "session",
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1000,"output_tokens":40,"input_tokens_details":{"cached_tokens":700}}}}"#,
        )
        .expect("Responses usage");
    assert!(matches!(
        events.as_slice(),
        [
            AgentEvent::Usage {
                input_tokens: Some(1000),
                output_tokens: Some(40),
                cached_input_tokens: Some(700),
                ..
            },
            AgentEvent::Completed { .. }
        ]
    ));
}

#[test]
fn chat_matrix_content_blocks_keep_their_text_instead_of_being_dropped() {
    let text_of = |content: Value| {
        let events = parse_sse_data(
            ProviderKind::OpenAiCompatible,
            "session",
            &json!({"choices":[{"delta":{"content":content}}]}).to_string(),
        )
        .expect("block-shaped content frame");
        match events.as_slice() {
            [] => None,
            [AgentEvent::TextDelta { text, .. }] => Some(text.clone()),
            _ => panic!("unexpected events for block-shaped content"),
        }
    };
    assert_eq!(text_of(json!("plain")).as_deref(), Some("plain"));
    // An empty block list is an empty delta, not a reason to drop the frame.
    assert_eq!(text_of(json!([])).as_deref(), Some(""));
    assert_eq!(
        text_of(json!([
            {"type": "output_text", "text": "你好"},
            {"type": "input_text", "text": "，"},
            {"type": "text", "text": "世界"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
            {"text": "！"}, // a text block whose `type` was dropped by a relay
        ]))
        .as_deref(),
        Some("你好，世界！"),
    );
    // A delta that carries no content at all still produces no text event.
    assert!(text_of(Value::Null).is_none());
    // The explicit reasoning channel keeps working for block-shaped content.
    let events = parse_sse_data(
        ProviderKind::OpenAiCompatible,
        "session",
        &json!({"choices":[{"delta":{"content":[{"type":"text","text":"分析"}],"channel":"analysis"}}]})
            .to_string(),
    )
    .expect("reasoning block frame");
    assert!(
        matches!(events.as_slice(), [AgentEvent::ReasoningDelta { text, .. }] if text == "分析")
    );
}

#[test]
fn reasoning_era_openai_models_send_max_completion_tokens_only() {
    for (kind, model, field) in [
        (ProviderKind::OpenAi, "gpt-5.1", "max_completion_tokens"),
        (
            ProviderKind::OpenAiCompatible,
            "openai/gpt-5-mini",
            "max_completion_tokens",
        ),
        (ProviderKind::OpenAi, "o3-mini", "max_completion_tokens"),
        (ProviderKind::OpenAiResponses, "gpt-5", "max_output_tokens"),
        (ProviderKind::Anthropic, "claude-sonnet-4", "max_tokens"),
        (ProviderKind::DeepSeek, "deepseek-reasoner", "max_tokens"),
        (ProviderKind::OpenAi, "gpt-4.1", "max_tokens"),
        (ProviderKind::OpenAiCompatible, "gpt-4o", "max_tokens"),
        (ProviderKind::Glm, "openchat-3.5", "max_tokens"),
    ] {
        let request =
            ProviderRequest::new("session", model, vec![AgentMessage::user("hi")], Vec::new())
                .with_max_tokens(1_234);
        let body = request_body(&provider(kind), &request).expect("request body");
        assert_eq!(body[field], json!(1_234), "{kind:?}/{model}");
        for other in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if other != field {
                assert!(
                    body.get(other).is_none(),
                    "{kind:?}/{model} must not also send {other}"
                );
            }
        }
    }
}

#[test]
fn usage_reasoning_tokens_are_read_from_each_protocol_side_channel() {
    // OpenAI chat reports them under completion_tokens_details.
    let chat = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "completion_tokens_details": {"reasoning_tokens": 7},
        },
    });
    let mut latest = None;
    let mut diagnostic = super::AttemptDiagnostic::new(None, "session", 0, 1, ProviderKind::OpenAi);
    let events = super::events::parse_sse_data_observed(
        ProviderKind::OpenAi,
        "session",
        &chat.to_string(),
        Some(&mut diagnostic),
        &mut latest,
    )
    .expect("chat usage frame");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Usage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            reasoning_tokens: Some(7),
            ..
        }
    )));
    assert_eq!(latest.and_then(|usage| usage.reasoning_tokens), Some(7));
    assert_eq!(diagnostic.data.reasoning_tokens, Some(7));

    // OpenAI Responses reports them under output_tokens_details.
    let responses = json!({"type": "response.completed", "response": {"usage": {
        "input_tokens": 11,
        "output_tokens": 3,
        "output_tokens_details": {"reasoning_tokens": 5},
    }}});
    let mut latest = None;
    super::events::parse_sse_data_observed(
        ProviderKind::OpenAiResponses,
        "session",
        &responses.to_string(),
        None,
        &mut latest,
    )
    .expect("responses usage frame");
    assert_eq!(latest.and_then(|usage| usage.reasoning_tokens), Some(5));

    // Anthropic has no such field, so nothing may be invented for it.
    let anthropic = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 9},
    });
    let mut latest = None;
    let events = super::events::parse_sse_data_observed(
        ProviderKind::Anthropic,
        "session",
        &anthropic.to_string(),
        None,
        &mut latest,
    )
    .expect("anthropic usage frame");
    assert!(events.iter().all(|event| !matches!(
        event,
        AgentEvent::Usage {
            reasoning_tokens: Some(_),
            ..
        }
    )));
    assert_eq!(latest.and_then(|usage| usage.reasoning_tokens), None);

    // A chat usage object without the details sub-object stays None too.
    let plain = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2},
    });
    let mut latest = None;
    let events = super::events::parse_sse_data_observed(
        ProviderKind::OpenAi,
        "session",
        &plain.to_string(),
        None,
        &mut latest,
    )
    .expect("plain usage frame");
    assert!(events.iter().all(|event| !matches!(
        event,
        AgentEvent::Usage {
            reasoning_tokens: Some(_),
            ..
        }
    )));
    assert_eq!(latest.and_then(|usage| usage.reasoning_tokens), None);
}

#[test]
fn usage_reasoning_tokens_reach_the_collected_stream_event() {
    // Production turns aggregate frames with `collect_stream_event` and emit the
    // snapshot with `into_event`; the counter must survive that aggregation and
    // must never be invented when a protocol does not report it.
    let reported = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "completion_tokens_details": {"reasoning_tokens": 7},
        },
    });
    let omitted = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    });
    let mut latest = None;
    let mut saw_terminal = false;
    for frame in [&reported, &omitted] {
        let events = super::events::parse_sse_data_observed(
            ProviderKind::OpenAi,
            "session",
            &frame.to_string(),
            None,
            &mut latest,
        )
        .expect("usage frame");
        for event in events {
            super::stream_state::collect_stream_event(event, &mut latest, &mut saw_terminal);
        }
    }
    match latest.expect("usage snapshot").into_event("session") {
        AgentEvent::Usage {
            input_tokens,
            reasoning_tokens,
            ..
        } => {
            assert_eq!(input_tokens, Some(10));
            // The last frame omitted the counter; the earlier report survives.
            assert_eq!(reasoning_tokens, Some(7));
        }
        other => panic!("expected a usage event, got {other:?}"),
    }

    // A protocol that never reports the counter must not gain a zero.
    let anthropic = json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 9},
    });
    let mut latest = None;
    let mut saw_terminal = false;
    for event in super::events::parse_sse_data_observed(
        ProviderKind::Anthropic,
        "session",
        &anthropic.to_string(),
        None,
        &mut latest,
    )
    .expect("usage frame")
    {
        super::stream_state::collect_stream_event(event, &mut latest, &mut saw_terminal);
    }
    match latest.expect("usage snapshot").into_event("session") {
        AgentEvent::Usage {
            output_tokens,
            reasoning_tokens,
            ..
        } => {
            assert_eq!(output_tokens, Some(9));
            assert_eq!(reasoning_tokens, None);
        }
        other => panic!("expected a usage event, got {other:?}"),
    }
}
