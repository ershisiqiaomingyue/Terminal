use super::*;

fn config(kind: ProviderKind) -> ProviderConfig {
    ProviderConfig::new("p", "p", kind, "https://example.com/v1", "key").unwrap()
}

#[test]
fn default_protocol_requests_enable_cache_adaptation_without_retention_configuration() {
    let request = ProviderRequest::new(
        "session",
        "model",
        vec![AgentMessage::user("question")],
        vec![],
    )
    .with_system_prompt("stable instructions");
    let anthropic = request_body(&config(ProviderKind::Anthropic), &request).unwrap();
    assert_eq!(anthropic["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        anthropic["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    assert!(anthropic["system"][0]["cache_control"]["ttl"].is_null());

    let first = request_body(&config(ProviderKind::OpenAiResponses), &request).unwrap();
    assert_eq!(first["instructions"], "stable instructions");
    assert_eq!(first["prompt_cache_key"], "session");
    assert!(first["prompt_cache_retention"].is_null());
    let mut continuation = request;
    continuation
        .messages
        .push(AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "read".into(),
            name: "read".into(),
            input: json!({"path":"file.ts"}),
        }));
    continuation
        .messages
        .push(AgentMessage::tool("read", "original result"));
    let next = request_body(&config(ProviderKind::OpenAiResponses), &continuation).unwrap();
    assert_eq!(first["input"][0], next["input"][0]);
    assert_eq!(first["instructions"], next["instructions"]);
    assert_eq!(first["prompt_cache_key"], next["prompt_cache_key"]);
}

#[test]
fn first_user_and_images_are_cached_before_tool_continuation() {
    let mut user = AgentMessage::user("large question");
    user.source_message_id = Some("u1".into());
    let request = ProviderRequest::new("s", "claude-sonnet-4-5", vec![user], vec![])
        .with_image_attachments(vec![ProviderImageAttachment {
            name: "a.png".into(),
            media_type: "image/png".into(),
            data_url: "data:image/png;base64,AQID".into(),
            source_message_id: "u1".into(),
        }]);
    let first = request_body(&config(ProviderKind::Anthropic), &request).unwrap();
    assert_eq!(
        first["messages"][0]["content"][1]["cache_control"]["type"],
        "ephemeral"
    );
    let mut next = request;
    next.messages
        .push(AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "read".into(),
            name: "read".into(),
            input: json!({}),
        }));
    next.messages
        .push(AgentMessage::tool("read", "original result"));
    let next = request_body(&config(ProviderKind::Anthropic), &next).unwrap();
    assert_eq!(first["messages"][0], next["messages"][0]);
    assert_eq!(
        next["messages"][2]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn cache_breakpoints_skip_empty_and_signed_thinking_blocks() {
    let thinking = json!({"type":"thinking","thinking":"original", "signature":"signed"});
    let mut message =
        json!({"role":"assistant","content":[{"type":"text","text":"answer"}, thinking.clone()]});
    requests::add_anthropic_cache_breakpoint(&mut message);
    assert_eq!(message["content"][1], thinking);
    assert!(message["content"][0]["cache_control"].is_object());
    let mut empty = json!({"role":"user","content":""});
    requests::add_anthropic_cache_breakpoint(&mut empty);
    assert_eq!(empty["content"], "");
}

#[test]
fn retention_is_opt_in_and_gated_without_touching_tool_inputs() {
    let request = ProviderRequest::new(
        "s",
        "claude-sonnet-4-5",
        vec![AgentMessage::user("q")],
        vec![],
    )
    .with_system_prompt("fixed");
    let default = request_body(&config(ProviderKind::Anthropic), &request).unwrap();
    assert!(default["system"][0]["cache_control"]["ttl"].is_null());
    let extended = request
        .clone()
        .with_prompt_cache_retention(Some("extended".into()));
    let body = request_body(&config(ProviderKind::Anthropic), &extended).unwrap();
    assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["ttl"],
        "1h"
    );
    for (kind, model, expected) in [
        (ProviderKind::OpenAiResponses, "gpt-5.4", Some("24h")),
        (ProviderKind::OpenAi, "gpt-4.1", Some("24h")),
        (ProviderKind::OpenAiCompatible, "unknown-relay-model", None),
        (ProviderKind::OpenAiResponses, "gpt-5.6", None),
        (ProviderKind::DeepSeek, "deepseek-chat", None),
    ] {
        let mut request = extended.clone();
        request.model = model.into();
        assert_eq!(
            request_body(&config(kind), &request).unwrap()["prompt_cache_retention"].as_str(),
            expected
        );
    }
    let mut fake = extended;
    fake.model = "deepseek-v4-pro".into();
    assert!(
        request_body(&config(ProviderKind::Anthropic), &fake).unwrap()["system"][0]
            ["cache_control"]["ttl"]
            .is_null()
    );
    let mut body = json!({"messages":[{"content":[{"type":"tool_use","input":{"cache_control":{"ttl":"original"}},"cache_control":{"type":"ephemeral","ttl":"1h"}}]}]});
    assert!(remove_cache_retention(&mut body));
    assert_eq!(
        body["messages"][0]["content"][0]["input"]["cache_control"]["ttl"],
        "original"
    );
    assert!(!remove_cache_retention(&mut body));
}

#[test]
fn retention_fallback_requires_rejected_optional_parameter() {
    for (code, message, expected) in [
        (400, "ttl is unsupported", true),
        (422, "unknown prompt_cache_retention", true),
        (401, "ttl is unsupported", false),
        (400, "invalid model", false),
        (503, "ttl service busy", false),
    ] {
        let error = ProviderClientError::Status {
            kind: ProviderStatusError::from_status(code),
            message: message.into(),
        };
        assert_eq!(cache_retention_unsupported(&error), expected);
    }
}

#[test]
fn forks_share_routing_key_without_changing_event_session() {
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::OpenAiCompatible,
        ProviderKind::OpenAiResponses,
    ] {
        let request =
            ProviderRequest::new("fork", "gpt-5.4", vec![AgentMessage::user("q")], vec![])
                .with_prompt_cache_key("root");
        assert_eq!(
            request_body(&config(kind), &request).unwrap()["prompt_cache_key"],
            "root"
        );
        assert_eq!(request.session_id, "fork");
    }
}

#[tokio::test]
async fn retention_rejection_retries_once_and_remembers_default_for_next_request() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..3 {
            let started = Instant::now();
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(started.elapsed() < Duration::from_secs(5));
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                assert!(bytes.len() < 128 * 1024);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                    let length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .unwrap()
                        .1
                        .trim()
                        .parse::<usize>()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        requests.push(
                            serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                                .unwrap(),
                        );
                        break;
                    }
                }
            }
            let (status, content_type, response) = if index == 0 {
                (
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":{"message":"unknown prompt_cache_retention"}}"#,
                )
            } else {
                ("200 OK", "text/event-stream", "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
            };
            write!(socket,"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
        requests
    });
    let client = HttpProviderClient {
        diagnostic_observer: None,
        diagnostic_sequence: Default::default(),
        config: ProviderConfig::new(
            "p",
            "p",
            ProviderKind::OpenAi,
            format!("http://{address}/v1"),
            "fixture",
        )
        .unwrap(),
        api_key: "fixture".into(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        response_start_timeout: Duration::from_secs(5),
        cache_retention_rejected: Default::default(),
    };
    let request = ProviderRequest::new("fork", "gpt-5.4", vec![AgentMessage::user("q")], vec![])
        .with_prompt_cache_key("root")
        .with_prompt_cache_retention(Some("extended".into()));
    for index in 0..2 {
        let events: Vec<_> = tokio::time::timeout(
            Duration::from_secs(10),
            client.stream(request.clone()).await.unwrap().collect(),
        )
        .await
        .unwrap();
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert!(events.iter().any(|event|matches!(event,Ok(AgentEvent::Completed {session_id,..}) if session_id == "fork")));
        assert_eq!(events.iter().filter(|event|matches!(event,Ok(AgentEvent::Status {status,..}) if status == "cache_retention_fallback")).count(),usize::from(index == 0));
    }
    let requests = server.join().unwrap();
    assert_eq!(requests[0]["prompt_cache_retention"], "24h");
    for body in &requests[1..] {
        assert!(body["prompt_cache_retention"].is_null());
        let mut first = requests[0].clone();
        first
            .as_object_mut()
            .unwrap()
            .remove("prompt_cache_retention");
        assert_eq!(&first, body);
    }
}

#[test]
fn durable_signed_history_is_an_unchanged_prefix_across_turns() {
    let mut assistant = AgentMessage::assistant_tool_call(crate::AgentToolCall {
        id: "read".into(),
        name: "read".into(),
        input: json!({"path":"a.cpp"}),
    });
    assistant.anthropic_content_blocks = vec![
        json!({"type":"thinking","thinking":"original thinking","signature":"native-signature"}),
        json!({"type":"tool_use","id":"read","name":"read","input":{"path":"a.cpp"}}),
    ];
    let request = ProviderRequest::new(
        "s",
        "claude-sonnet-4-5",
        vec![
            AgentMessage::user("read a.cpp"),
            AgentMessage::system("first runtime settings"),
            assistant,
            AgentMessage::tool("read", "original bytes"),
        ],
        vec![],
    )
    .with_system_prompt("fixed system");
    let fingerprint =
        crate::PromptCacheFingerprint::capture(&config(ProviderKind::Anthropic), &request).unwrap();
    let mut messages = request.messages.clone();
    messages.push(AgentMessage::user("next question"));
    let transcript = crate::ModelTranscript {
        version: 1,
        scope: "p".into(),
        turn_id: "t".into(),
        runtime_context: "settings".into(),
        messages,
        images: vec![],
        last_request: Some(fingerprint.clone()),
    };
    let saved = serde_json::to_vec(&transcript).unwrap();
    let restored: crate::ModelTranscript = serde_json::from_slice(&saved).unwrap();
    assert!(restored.is_replayable("p"));
    let mut next = request.clone();
    next.messages = restored.messages;
    next.messages
        .push(AgentMessage::system("new project/skill settings"));
    let body = request_body(&config(ProviderKind::Anthropic), &next).unwrap();
    assert_eq!(
        body["messages"][2]["content"][0]["signature"],
        "native-signature"
    );
    let diagnostic =
        crate::PromptCacheFingerprint::capture(&config(ProviderKind::Anthropic), &next)
            .unwrap()
            .compare(Some(&fingerprint));
    assert_eq!(diagnostic.first_changed_section, "append_only");
    assert_eq!(diagnostic.matching_messages, 4);
}
