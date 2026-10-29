//! Explicit live test: consumes the logged-in WorkBuddy account's model quota.
//! Run via `node scripts/test-workbuddy-live.mjs`; normal cargo test skips it.
use agent_core::collect_model_round;
use agent_core::turn_recovery::{ModelResponseRecovery, ProgressRecovery, ProgressRecoveryContext};
use agent_core::{
    model_protocol, parse_tool_call_input, register_local_tools, AgentMessage,
    AgentProviderSettings, AgentToolCall, AllowPermission, HttpProviderClient, LocalToolContext,
    MessageRole, ProviderConfig, ProviderKind, ProviderRequest, ToolRegistry, ToolStreamState,
};
use std::{
    fs,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct ProbeDirectory(std::path::PathBuf);
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires explicit live WorkBuddy bridge credentials and consumes model quota"]
async fn workbuddy_native_file_tool_roundtrip() {
    let settings: AgentProviderSettings = serde_json::from_str(
        &std::env::var("ANDROID_TOOL_WORKBUDDY_E2E_PROVIDER").expect("run via live test script"),
    )
    .unwrap();
    let token = std::env::var("ANDROID_TOOL_WORKBUDDY_E2E_TOKEN").unwrap();
    let ids = std::env::var("ANDROID_TOOL_WORKBUDDY_E2E_MODELS").unwrap();
    for id in ids.split(',') {
        let model = settings
            .models
            .iter()
            .find(|m| m.id == id)
            .expect("model unavailable");
        let protocol = model_protocol(&settings, model);
        // The installed DSH WorkBuddy connector exposes completions for all models.
        assert_eq!(protocol, ProviderKind::OpenAiCompatible);
        let client = HttpProviderClient::new(
            ProviderConfig::new(
                &settings.id,
                &settings.name,
                protocol,
                &settings.base_url,
                "live-test-loopback",
            )
            .unwrap(),
            &token,
        )
        .unwrap();
        let nonce = format!(
            "wb-probe-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let probe = ProbeDirectory(std::env::temp_dir().join(&nonce));
        fs::create_dir(&probe.0).unwrap();
        let path = probe.0.join("probe.txt");
        // Keep the expected content out of the prompt and file path, so the
        // final reply must use the actual tool result rather than echo a hint.
        let expected = format!(
            "verified-content-{:x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        fs::write(&path, &expected).unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        register_local_tools(
            &mut registry,
            LocalToolContext::new(
                vec![probe.0.to_string_lossy().into_owned()],
                vec![],
                false,
                cancellation.clone(),
                None,
            ),
        )
        .unwrap();
        let tools = registry
            .list_public()
            .into_iter()
            .filter(|t| t.name == "read_local_file")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 1);
        let mut messages = vec![AgentMessage::user(format!(
            "请读取文件 {}，回复里面的验证字符串。必须真实调用 read_local_file，不能只说准备读取。",
            path.display()
        ))];
        let mut recovery = ModelResponseRecovery::default();
        let mut calls_executed = 0;
        let mut finished = false;
        let mut streamed_text = false;
        let mut reported_usage = false;
        for round_index in 0..5 {
            let request = ProviderRequest::new(&nonce, id, messages.clone(), tools.clone())
                .with_system_prompt("你是可靠的工具助手。请求读取文件时使用原生工具调用；只依据真实工具结果回答。取得验证字符串后直接输出字符串，不要重复读取。")
                .with_max_tokens(2048).with_usage_reporting(true)
                .with_reasoning_effort(std::env::var("ANDROID_TOOL_E2E_REASONING_EFFORT").ok())
                .with_native_reasoning_effort(true);
            let stream = client.stream(request).await.expect("start provider stream");
            let (_sender, mut steer) = tokio::sync::mpsc::channel(1);
            let round = tokio::time::timeout(
                Duration::from_secs(120),
                collect_model_round(
                    stream,
                    &nonce,
                    &mut ToolStreamState::default(),
                    &mut steer,
                    &cancellation,
                    &["read_local_file".into()],
                ),
            )
            .await
            .expect("live round timed out")
            .expect("parse live stream");
            assert!(!round.output_truncated, "{id}: truncated reply");
            assert!(round.interruption.is_none(), "{id}: interrupted reply");
            assert!(
                !round.synthesized_plain_tool_calls,
                "{id}: expected native function call"
            );
            reported_usage |= round
                .usage
                .as_ref()
                .is_some_and(|u| u.input_tokens.is_some() && u.output_tokens.is_some());
            streamed_text |= !round.text.is_empty();
            let mut response = AgentMessage::user(&round.text);
            response.role = MessageRole::Assistant;
            response.reasoning_content =
                (!round.reasoning_content.is_empty()).then_some(round.reasoning_content);
            response.responses_reasoning_items = round.responses_reasoning_items;
            response.anthropic_thinking_blocks = round.anthropic_thinking_blocks;
            response.anthropic_content_blocks = round.anthropic_content_blocks;
            if round.pending_calls.is_empty() {
                if calls_executed > 0 && round.text.contains(&expected) {
                    finished = true;
                    messages.push(response);
                    println!("LIVE_RESULT model={id} protocol=open_ai_compatible rounds={} native_calls={calls_executed} streamed_text={streamed_text} usage={reported_usage} file_result_verified=true", round_index + 1);
                    break;
                }
                match recovery.retry_progress_only(
                    &mut messages,
                    &response,
                    ProgressRecoveryContext {
                        tools_available: true,
                        final_only: false,
                        output_truncated: false,
                    },
                ) {
                    ProgressRecovery::Retry => continue,
                    other => {
                        panic!("{id}: no native tool call or verified final reply ({other:?})")
                    }
                }
            }
            let calls = round
                .pending_calls
                .into_iter()
                .map(|(id, (name, input, arguments))| {
                    let input = parse_tool_call_input(input, &arguments, &name).unwrap();
                    assert_eq!(name, "read_local_file");
                    let target = std::path::Path::new(input["path"].as_str().unwrap());
                    assert_eq!(
                        fs::canonicalize(target).unwrap(),
                        fs::canonicalize(&path).unwrap()
                    );
                    AgentToolCall { id, name, input }
                })
                .collect::<Vec<_>>();
            response.tool_calls = calls.clone();
            messages.push(response);
            for call in calls {
                assert_eq!(calls_executed, 0, "{id}: duplicate file read");
                let result = registry
                    .execute(&call.name, call.input, &AllowPermission)
                    .await
                    .unwrap();
                assert!(result.to_string().contains(&expected));
                calls_executed += 1;
                messages.push(AgentMessage::tool(call.id, result.to_string()));
            }
        }
        assert!(
            finished && streamed_text && reported_usage,
            "{id}: incomplete roundtrip"
        );
        // A new user turn must retain the real result, rather than force a
        // second read. Keep tools available so a duplicate isn't hidden by
        // disabling the tool list on this request.
        messages.push(AgentMessage::user("沿用上轮真实读取的验证字符串，不要重复读文件。逐行输出编号 01 到 20，每行写一段不同的简短中文说明，最后另起一行输出原验证字符串和 END_OF_REPLY。"));
        let request = ProviderRequest::new(&nonce, id, messages, tools.clone())
            .with_system_prompt("优先复用历史成功工具结果；新用户消息不代表文件变化。只需基于历史内容回答时不要调用工具。")
            .with_max_tokens(4096).with_usage_reporting(true)
            .with_reasoning_effort(std::env::var("ANDROID_TOOL_E2E_REASONING_EFFORT").ok())
            .with_native_reasoning_effort(true);
        let stream = client.stream(request).await.unwrap();
        let (_sender, mut steer) = tokio::sync::mpsc::channel(1);
        let followup = tokio::time::timeout(Duration::from_secs(120), collect_model_round(
            stream, &nonce, &mut ToolStreamState::default(), &mut steer, &cancellation, &["read_local_file".into()],
        )).await.expect("followup timed out").expect("followup stream failed");
        assert!(followup.pending_calls.is_empty(), "{id}: unnecessary reread on followup");
        assert!(!followup.output_truncated && followup.interruption.is_none(), "{id}: followup interrupted");
        assert!(followup.text.contains(&expected) && followup.text.contains("END_OF_REPLY"), "{id}: missing historical result or reply tail");
        println!("LIVE_HISTORY model={id} second_turn_extra_reads=0 historical_result_verified=true reply_tail_verified=true");
    }
}
