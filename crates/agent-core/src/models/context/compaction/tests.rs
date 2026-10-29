use super::build_semantic_checkpoint;
use super::*;
use crate::*;
use serde_json::Value;

// P8-E：一次压缩之后触发线下移一段安全带；没有压缩过时不干预。
#[test]
fn compaction_hysteresis_lowers_the_line_after_a_trigger() {
    let mut hysteresis = CompactionHysteresis::default();
    assert_eq!(hysteresis.effective_trigger(80_000), 80_000);
    assert_eq!(hysteresis.last_trigger(), None);

    // 记录一次 80_000 的触发后，下一次落在 72_000（下移 10%）。
    hysteresis.record_trigger(80_000);
    assert_eq!(hysteresis.effective_trigger(80_000), 72_000);
    assert_eq!(hysteresis.last_trigger(), Some(80_000));

    // 滞回只会下移触发线，不会把触发线抬到计算值之上。
    assert_eq!(hysteresis.effective_trigger(60_000), 60_000);

    // 连续触发会继续前移，但永远不会降到 1 以下。
    for _ in 0..200 {
        let next = hysteresis.effective_trigger(80_000);
        hysteresis.record_trigger(next);
    }
    assert!(hysteresis.effective_trigger(80_000) >= 1);

    // 复位后回到「不干预」。
    hysteresis.reset();
    assert_eq!(hysteresis.effective_trigger(80_000), 80_000);
}

#[test]
fn token_calibrator_converges_with_damping_and_bounds() {
    // P8-C：EMA 校准器——持续高估漂移收敛到 >100%，但单步受限；
    // 单次异常报告影响被双重阻尼压住；绝对界生效。
    let mut calibrator = TokenEstimateCalibrator::default();
    assert_eq!(calibrator.charge_percent(), 100);
    // 持续实收 2 倍：多次观测后逼近 200，但每步最多走 15%×20%=3%。
    for _ in 0..200 {
        calibrator.observe(2_000, 1_000);
    }
    let converged = calibrator.charge_percent();
    assert!(converged > 150, "应收敛到明显大于 100，得到 {converged}");
    assert!(converged <= 200, "绝对上界 2.0，得到 {converged}");
    // 单次 0.5× 异常：影响 ≤ 单步限幅×学习率（≈3%），不跳变。
    let before = calibrator.charge_percent();
    calibrator.observe(500, 1_000);
    assert!(before - calibrator.charge_percent() <= 4);
    // 持续低估（实收一半）允许回调（旧实现只缩不放的问题被修复）。
    let mut downward = TokenEstimateCalibrator::default();
    for _ in 0..200 {
        downward.observe(600, 1_000);
    }
    assert_eq!(downward.charge_percent(), 60);
}

fn summary_fixture() -> (Vec<AgentMessage>, CompactionResult) {
    let source = vec![
        AgentMessage::system("preserve-system"),
        AgentMessage::user("old-goal ".repeat(4_000)),
        AgentMessage::tool("evidence", "original evidence"),
        AgentMessage::user("recent"),
        AgentMessage::user("CURRENT-SECRET"),
    ];
    let result = compact_messages(&source, 2_000);
    assert!(result.compressed);
    assert!(result.checkpoint_payload.is_some());
    (source, result)
}

#[test]
fn model_summary_input_is_bounded_and_excludes_current_and_reasoning() {
    let (mut source, result) = summary_fixture();
    source[0].reasoning_content = Some("PRIVATE-REASONING".into());
    source[0]
        .responses_reasoning_items
        .push(serde_json::json!({"encrypted_content":"PRIVATE-OPAQUE"}));
    let input = model_summary_input(&source, &result, 400).unwrap();
    assert!(estimate_text_tokens(&input) <= 400);
    assert!(!input.contains("CURRENT-SECRET"));
    assert!(!input.contains("PRIVATE-REASONING"));
    assert!(!input.contains("PRIVATE-OPAQUE"));
    assert!(model_summary_input(&source, &result, 32).is_none());
    assert!(model_summary_input(&source, &compact_messages(&source, 1_000_000), 400).is_none());
}

fn summary_reuse_fixture(
    kind: crate::ProviderKind,
) -> (
    crate::ProviderConfig,
    crate::ProviderRequest,
    CompactionResult,
) {
    let config = crate::ProviderConfig::new(
        "provider-a",
        "test",
        kind,
        "https://example.test/v1",
        "test-reference",
    )
    .unwrap();
    let messages = vec![
        AgentMessage::user("old goal"),
        AgentMessage::assistant_tool_call(AgentToolCall {
            id: "read-a".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path":"a.rs"}),
        }),
        AgentMessage::tool("read-a", "verified historical evidence"),
        AgentMessage::user("older covered detail ".repeat(4_000)),
        AgentMessage::user("recent retained request"),
        AgentMessage::user("CURRENT-SECRET"),
    ];
    let compaction = compact_messages(&messages, 2_000);
    assert!(compaction.covered_message_count.unwrap() >= 3);
    let request = crate::ProviderRequest::new(
        "session",
        "model-a",
        messages,
        vec![ToolDefinition::new(
            "read_file",
            "Reads evidence",
            serde_json::json!({"type":"object"}),
        )],
    )
    .with_system_prompt("stable system constraints")
    .with_reasoning_effort(Some("low".into()));
    (config, request, compaction)
}

#[test]
fn summary_reuses_byte_identical_routed_wire_prefix_with_complete_tool_pairs() {
    for kind in [
        crate::ProviderKind::OpenAiCompatible,
        crate::ProviderKind::DeepSeek,
        crate::ProviderKind::OpenAiResponses,
    ] {
        let (config, routed, compaction) = summary_reuse_fixture(kind);
        let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
        let summary = prefix
            .summary_request(&config, &routed, &routed.messages, &compaction, 1_024, 256)
            .unwrap();
        let original_body = crate::request_body(&config, &routed).unwrap();
        let summary_body = crate::request_body(&config, &summary).unwrap();
        let field = if kind == crate::ProviderKind::OpenAiResponses {
            "input"
        } else {
            "messages"
        };
        let summary_rows = summary_body[field].as_array().unwrap();
        let prefix_rows = &summary_rows[..summary_rows.len() - 1];
        let original_rows = original_body[field].as_array().unwrap();
        assert_eq!(
            serde_json::to_vec(prefix_rows).unwrap(),
            serde_json::to_vec(&original_rows[..prefix_rows.len()]).unwrap()
        );
        assert_eq!(original_body.get("tools"), summary_body.get("tools"));
        assert_eq!(
            original_body.get("instructions"),
            summary_body.get("instructions")
        );
        assert_eq!(
            original_body.get("reasoning"),
            summary_body.get("reasoning")
        );
        assert!(summary
            .messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("read-a")));
        assert!(
            estimate_request_tokens(
                &summary.messages,
                summary.system_prompt.as_deref().unwrap(),
                &summary.tools
            ) <= 1_024
        );
        assert!(!serde_json::to_string(&summary_body)
            .unwrap()
            .contains("CURRENT-SECRET"));
        assert_eq!(summary.max_tokens, 256);
    }
}

#[test]
fn summary_prefix_invalidates_provider_model_profile_and_changed_history() {
    let (config, routed, compaction) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
    let build = |config: &crate::ProviderConfig,
                 profile: &crate::ProviderRequest,
                 source: &[AgentMessage]| {
        prefix.summary_request(config, profile, source, &compaction, 1_024, 256)
    };
    for changed in [
        crate::ProviderConfig {
            id: "other-provider".into(),
            ..config.clone()
        },
        crate::ProviderConfig {
            base_url: "https://other.test/v1".into(),
            ..config.clone()
        },
        crate::ProviderConfig {
            kind: crate::ProviderKind::OpenAiResponses,
            ..config.clone()
        },
    ] {
        assert!(build(&changed, &routed, &routed.messages).is_none());
    }
    let mut profiles = Vec::new();
    let mut changed = routed.clone();
    changed.model = "other-model".into();
    profiles.push(changed);
    let mut changed = routed.clone();
    changed.system_prompt = Some("new constraints".into());
    profiles.push(changed);
    let mut changed = routed.clone();
    changed.tools[0].input_schema = serde_json::json!({"type":"string"});
    profiles.push(changed);
    let mut changed = routed.clone();
    changed.reasoning_effort = Some("high".into());
    profiles.push(changed);
    let mut changed = routed.clone();
    changed.include_usage = !changed.include_usage;
    profiles.push(changed);
    for changed in profiles {
        assert!(build(&config, &changed, &routed.messages).is_none());
    }
    let mut changed_source = routed.messages.clone();
    changed_source[0].content = "changed goal".into();
    assert!(build(&config, &routed, &changed_source).is_none());
    let mut provenance_only = routed.messages.clone();
    provenance_only[0].source_message_id = Some("new-local-id".into());
    assert!(build(&config, &routed, &provenance_only).is_some());
    let mut changed_after_head = routed.messages.clone();
    changed_after_head[1].content = "changed tool commentary".into();
    let summary = build(&config, &routed, &changed_after_head).unwrap();
    assert_eq!(summary.messages[0], routed.messages[0]);
    assert!(summary.messages[..summary.messages.len() - 1]
        .iter()
        .all(|message| message.tool_calls.is_empty()));
}

#[test]
fn summary_prefix_never_replays_reasoning_images_or_incomplete_tool_groups() {
    let (config, mut routed, compaction) =
        summary_reuse_fixture(crate::ProviderKind::OpenAiResponses);
    routed.messages[1].reasoning_content = Some("PRIVATE-REASONING".into());
    routed.messages[1]
        .responses_reasoning_items
        .push(serde_json::json!({"encrypted_content":"PRIVATE-OPAQUE"}));
    let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
    let summary = prefix
        .summary_request(&config, &routed, &routed.messages, &compaction, 1_024, 256)
        .unwrap();
    let body = serde_json::to_string(&crate::request_body(&config, &summary).unwrap()).unwrap();
    assert!(!body.contains("PRIVATE-REASONING"));
    assert!(!body.contains("PRIVATE-OPAQUE"));
    assert_eq!(summary.messages.len(), 2);
    let (config, mut routed, _) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    routed.messages.remove(2);
    let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
    assert_eq!(prefix.request.messages.len(), 1);
    routed
        .image_attachments
        .push(crate::ProviderImageAttachment {
            name: "image".into(),
            media_type: "image/png".into(),
            data_url: "data:image/png;base64,AA==".into(),
            source_message_id: "image-user".into(),
        });
    assert!(ContextSummaryPrefix::capture(&config, &routed).is_none());
    let (config, routed, _) = summary_reuse_fixture(crate::ProviderKind::Anthropic);
    assert!(ContextSummaryPrefix::capture(&config, &routed).is_none());
}

#[test]
fn summary_prefix_is_bounded_and_rejects_large_fixed_overhead() {
    let (config, routed, compaction) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
    assert!(
        estimate_request_tokens(
            &prefix.request.messages,
            prefix.request.system_prompt.as_deref().unwrap(),
            &prefix.request.tools
        ) <= CONTEXT_SUMMARY_INPUT_LIMIT
    );
    let mut oversized = routed.clone();
    oversized.system_prompt = Some("x".repeat(40_000));
    assert!(ContextSummaryPrefix::capture(&config, &oversized).is_none());
    assert!(prefix
        .summary_request(&config, &routed, &routed.messages, &compaction, 64, 128)
        .is_none());
    let summary = prefix
        .summary_request(
            &config,
            &routed,
            &routed.messages,
            &compaction,
            1_000_000,
            256,
        )
        .unwrap();
    assert!(
        estimate_request_tokens(
            &summary.messages,
            summary.system_prompt.as_deref().unwrap(),
            &summary.tools
        ) <= CONTEXT_SUMMARY_INPUT_LIMIT
    );
}

#[test]
fn summary_large_input_capture_keeps_only_the_bounded_leading_request() {
    let (config, mut routed, _) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    routed.messages = vec![
        AgentMessage::user("stable historical head"),
        AgentMessage::user("x".repeat(4_000_000)),
    ];
    let prefix = ContextSummaryPrefix::capture(&config, &routed).unwrap();
    assert_eq!(prefix.request.messages.len(), 1);
    assert_eq!(prefix.request.messages[0], routed.messages[0]);
    assert!(serde_json::to_vec(&prefix.request).unwrap().len() < 65_536);
    assert!(serde_json::to_vec(&prefix.wire_messages).unwrap().len() < 65_536);
    assert!(
        estimate_request_tokens(
            &prefix.request.messages,
            prefix.request.system_prompt.as_deref().unwrap(),
            &prefix.request.tools
        ) <= CONTEXT_SUMMARY_INPUT_LIMIT
    );
    assert_eq!(routed.messages[1].content.len(), 4_000_000);
}

#[test]
fn summary_cache_commit_is_bounded_session_scoped_and_evicts_oldest() {
    let (config, mut routed, _) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    let mut cache = ContextSummaryPrefixCache::default();
    for index in 0..16 {
        routed.session_id = format!("session-{index}");
        cache.commit(
            &routed.session_id,
            ContextSummaryPrefix::capture(&config, &routed),
            true,
        );
    }
    assert_eq!(cache.entries.len(), 16);
    routed.session_id = "session-0".into();
    routed.model = "updated-model".into();
    cache.commit(
        &routed.session_id,
        ContextSummaryPrefix::capture(&config, &routed),
        true,
    );
    assert_eq!(cache.entries.len(), 16);
    routed.session_id = "session-16".into();
    cache.commit(
        &routed.session_id,
        ContextSummaryPrefix::capture(&config, &routed),
        true,
    );
    assert_eq!(cache.entries.len(), 16);
    assert!(cache.get("session-1").is_none());
    assert!(cache.get("session-16").is_some());
    assert_eq!(
        cache.get("session-0").unwrap().request.model,
        "updated-model"
    );
    assert!(cache.get("unrelated-session").is_none());
    let bytes: usize = cache
        .entries
        .iter()
        .map(|(_, _, prefix)| {
            serde_json::to_vec(&prefix.request).unwrap().len()
                + serde_json::to_vec(&prefix.wire_messages).unwrap().len()
        })
        .sum();
    assert!(bytes < 16 * 131_072);
}

#[test]
fn summary_failed_or_unaccepted_candidate_does_not_replace_a_warm_prefix() {
    let (config, mut routed, _) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    let mut cache = ContextSummaryPrefixCache::default();
    cache.commit(
        "session",
        ContextSummaryPrefix::capture(&config, &routed),
        true,
    );
    let original = serde_json::to_vec(&cache.get("session").unwrap().request).unwrap();
    routed.model = "failed-model".into();
    cache.commit(
        "session",
        ContextSummaryPrefix::capture(&config, &routed),
        false,
    );
    assert_eq!(
        serde_json::to_vec(&cache.get("session").unwrap().request).unwrap(),
        original
    );
    cache.commit("session", None, false);
    assert_eq!(
        serde_json::to_vec(&cache.get("session").unwrap().request).unwrap(),
        original
    );
    cache.commit(
        "session",
        ContextSummaryPrefix::capture(&config, &routed),
        true,
    );
    assert_eq!(cache.get("session").unwrap().request.model, "failed-model");
    // An accepted visual/unsupported request invalidates the old text prefix.
    cache.commit("session", None, true);
    assert!(cache.get("session").is_none());
}

#[test]
fn summary_cache_expires_old_prefixes_without_replaying_them() {
    let (config, routed, _) = summary_reuse_fixture(crate::ProviderKind::DeepSeek);
    let mut cache = ContextSummaryPrefixCache::default();
    cache.commit(
        "session",
        ContextSummaryPrefix::capture(&config, &routed),
        true,
    );
    cache.entries[0].1 = std::time::Instant::now() - std::time::Duration::from_secs(1_201);
    assert!(cache.get("session").is_none());
    let mut new_request = routed;
    new_request.session_id = "new-session".into();
    cache.commit(
        "new-session",
        ContextSummaryPrefix::capture(&config, &new_request),
        true,
    );
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.get("new-session").is_some());
}

#[test]
fn model_summary_never_replaces_constraints_evidence_or_retained_messages() {
    let (_, mut result) = summary_fixture();
    let original = result.clone();
    assert!(enrich_compaction_summary(
        &mut result,
        "已确认目标，旧工具结论仍需复核。"
    ));
    assert_eq!(
        &result.checkpoint_payload.as_ref().unwrap().entries
            [..original.checkpoint_payload.as_ref().unwrap().entries.len()],
        &original.checkpoint_payload.as_ref().unwrap().entries
    );
    let inserted = result
        .messages
        .iter()
        .find(|message| message.content.contains("模型生成的历史摘要"))
        .unwrap();
    assert_eq!(inserted.role, MessageRole::Assistant);
    assert!(inserted.tool_calls.is_empty());
    assert!(inserted.tool_call_id.is_none());
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|message| !message.content.contains("模型生成的历史摘要"))
            .cloned()
            .collect::<Vec<_>>(),
        original.messages
    );
    assert!(result.stats.compacted_estimated_tokens <= result.stats.budget_tokens);
    result.stats.budget_tokens = result.stats.compacted_estimated_tokens;
    let full = result.clone();
    assert!(!enrich_compaction_summary(&mut result, "more"));
    assert_eq!(result, full);
    assert!(!enrich_compaction_summary(&mut result, &"字".repeat(1_025)));
    assert!(!enrich_compaction_summary(&mut result, " "));
}

#[tokio::test]
async fn model_summary_progress_counts_received_characters_not_bytes_or_reasoning() {
    use crate::AgentEvent;
    use futures_util::stream;
    let events = vec![
        Ok(AgentEvent::ReasoningDelta {
            session_id: "s".into(),
            text: "not summary".into(),
        }),
        Ok(AgentEvent::TextDelta {
            session_id: "s".into(),
            text: "摘要".into(),
        }),
        Ok(AgentEvent::TextDelta {
            session_id: "s".into(),
            text: " A😀".into(),
        }),
        Ok(AgentEvent::Completed {
            session_id: "s".into(),
        }),
    ];
    let mut observed = Vec::new();
    let mut usage = None;
    let summary = collect_context_summary_with_progress(
        Box::pin(stream::iter(events)),
        &mut usage,
        |characters| observed.push(characters),
    )
    .await;
    assert_eq!(summary.as_deref(), Some("摘要 A😀"));
    assert_eq!(observed, vec![2, 5]);
}

#[tokio::test]
async fn model_summary_stream_requires_complete_plain_text_and_keeps_usage_on_failure() {
    use crate::AgentEvent;
    use futures_util::stream;
    let text = || {
        Ok(AgentEvent::TextDelta {
            session_id: "s".into(),
            text: "保留目标，证据需复核".into(),
        })
    };
    let completed = || {
        Ok(AgentEvent::Completed {
            session_id: "s".into(),
        })
    };
    let mut usage = None;
    let valid = vec![
        Ok(AgentEvent::ReasoningDelta {
            session_id: "s".into(),
            text: "discard".into(),
        }),
        text(),
        completed(),
    ];
    assert_eq!(
        collect_context_summary(Box::pin(stream::iter(valid)), &mut usage)
            .await
            .as_deref(),
        Some("保留目标，证据需复核")
    );
    for invalid in [
        vec![text()],
        vec![
            text(),
            Ok(AgentEvent::OutputTruncated {
                session_id: "s".into(),
            }),
            completed(),
        ],
        vec![
            text(),
            Ok(AgentEvent::ToolCallStarted {
                session_id: "s".into(),
                call_id: "c".into(),
                tool_index: None,
                tool_name: "shell".into(),
                input: Value::Null,
            }),
            completed(),
        ],
        vec![
            Ok(AgentEvent::TextDelta {
                session_id: "s".into(),
                text: "x".repeat(16_385),
            }),
            completed(),
        ],
        vec![text(), completed(), text()],
    ] {
        assert!(
            collect_context_summary(Box::pin(stream::iter(invalid)), &mut usage)
                .await
                .is_none()
        );
    }
    let invalid = vec![
        Ok(AgentEvent::Usage {
            session_id: "s".into(),
            input_tokens: Some(300),
            output_tokens: Some(20),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_tokens: Some(7),
        }),
        text(),
        Ok(AgentEvent::OutputTruncated {
            session_id: "s".into(),
        }),
    ];
    assert!(
        collect_context_summary(Box::pin(stream::iter(invalid)), &mut usage)
            .await
            .is_none()
    );
    let usage = usage.expect("usage");
    assert_eq!(usage.input_tokens, Some(300));
    assert_eq!(usage.reasoning_tokens, Some(7));
}

#[tokio::test]
async fn model_summary_timeout_keeps_usage_already_received() {
    use futures_util::stream::{self, StreamExt};
    let event = Ok(crate::AgentEvent::Usage {
        session_id: "s".into(),
        input_tokens: Some(42),
        output_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        reasoning_tokens: None,
    });
    let stream = stream::iter(vec![event]).chain(stream::pending());
    let mut usage = None;
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(10),
        collect_context_summary(Box::pin(stream), &mut usage)
    )
    .await
    .is_err());
    assert_eq!(usage.unwrap().input_tokens, Some(42));
}

#[test]
fn unknown_context_window_is_explicit_but_uses_a_conservative_runtime_budget() {
    let resolution = resolve_context_window(None);
    assert_eq!(resolution.status, ContextWindowStatus::Unknown);
    assert_eq!(resolution.reported_tokens, None);
    assert_eq!(resolution.effective_tokens, 64_000);
    assert!(resolution.conservative);
    assert_eq!(effective_context_window(None), 64_000);
    assert!(context_budget(None, "system", &[], false) < 64_000);
}

#[test]
fn invalid_declared_context_windows_are_treated_as_unknown_metadata() {
    for declared in [Some(0), Some(1), Some(1_023)] {
        let resolution = resolve_context_window(declared);
        assert_eq!(resolution.status, ContextWindowStatus::Unknown);
        assert_eq!(resolution.reported_tokens, None);
        assert_eq!(resolution.effective_tokens, DEFAULT_CONTEXT_WINDOW);
        assert!(resolution.conservative);
    }
}

#[test]
fn known_long_context_windows_keep_their_full_budget() {
    let one_million = resolve_context_window(Some(1_000_000));
    let two_million = resolve_context_window(Some(2_000_000));
    assert!(one_million.is_known());
    assert_eq!(one_million.reported_tokens, Some(1_000_000));
    assert_eq!(one_million.effective_tokens, 1_000_000);
    assert_eq!(two_million.effective_tokens, 2_000_000);
    assert_eq!(context_budget(Some(1_000_000), "", &[], false), 991_808);
    assert_eq!(context_budget(Some(2_000_000), "", &[], false), 1_991_808);
    assert_eq!(
        context_budget(Some(2_000_000), "", &[], false)
            - context_budget(Some(1_000_000), "", &[], false),
        1_000_000
    );
}

#[test]
fn keeps_current_request_after_compaction() {
    let history = vec![
        AgentMessage::user("old request ".repeat(500)),
        AgentMessage {
            role: MessageRole::Assistant,
            content: "old answer ".repeat(500),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        },
        AgentMessage::user("recent request"),
    ];
    let result =
        compact_messages_with_current(&history, AgentMessage::user("current request"), 160);
    assert!(result.compressed);
    assert_eq!(result.covered_message_count, Some(1));
    assert!(result.stats.budget_satisfied);
    assert!(estimate_messages_tokens(&result.messages) <= 160);
    assert!(result
        .messages
        .iter()
        .any(|message| message.content == "current request"));
}

#[test]
fn keeps_tool_sequence_together() {
    let history = vec![
        AgentMessage::user("old".repeat(200)),
        AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path":"a"}),
        }),
        AgentMessage::tool("call-1", "file content"),
    ];
    let result = compact_messages_with_current(&history, AgentMessage::user("next"), 120);
    assert_eq!(result.covered_message_count, Some(1));
    assert!(result.stats.budget_satisfied);
    let tool_index = result
        .messages
        .iter()
        .position(|message| message.role == MessageRole::Tool)
        .expect("tool result retained");
    assert!(tool_index > 0);
    assert_eq!(result.messages[tool_index - 1].role, MessageRole::Assistant);
}

#[test]
fn retained_recent_tokens_follow_the_ratio_but_stay_capped() {
    // 16% of what is left after the checkpoint budget.
    assert_eq!(retained_recent_tokens(3_000, 1_000), 320);
    assert_eq!(retained_recent_tokens(160, 53), 17);
    // A huge window must not push the verbatim tail past the ceiling; this
    // is what stops an 800K-token transcript from being re-sent whole.
    assert_eq!(
        retained_recent_tokens(1_000_000, CHECKPOINT_TOKEN_CEILING),
        RETAIN_TOKEN_CEILING
    );
    assert_eq!(
        retained_recent_tokens(800_000, CHECKPOINT_TOKEN_CEILING),
        RETAIN_TOKEN_CEILING
    );
    // A checkpoint that already eats the whole budget retains nothing extra.
    assert_eq!(retained_recent_tokens(20, 40), 0);
}

#[test]
fn verbatim_tail_grows_with_the_budget_instead_of_stopping_at_two_messages() {
    let mut history = Vec::new();
    for _ in 0..60 {
        history.push(AgentMessage::user("a".repeat(200)));
        history.push(AgentMessage {
            role: MessageRole::Assistant,
            content: "b".repeat(200),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        });
    }
    let small =
        compact_messages_with_current(&history, AgentMessage::user("current request"), 3_000);
    let large =
        compact_messages_with_current(&history, AgentMessage::user("current request"), 6_000);
    assert!(small.compressed && large.compressed);
    assert!(small.stats.budget_satisfied);
    assert!(large.stats.budget_satisfied);
    // The old fixed `history.len() - 2` tail kept exactly two messages
    // however much budget was available.
    assert!(
        small.stats.retained_recent_message_count > RETAIN_MIN_MESSAGES,
        "small budget retained only {} messages",
        small.stats.retained_recent_message_count
    );
    assert!(
        large.stats.retained_recent_message_count > small.stats.retained_recent_message_count,
        "tail did not grow with the budget: {} vs {}",
        large.stats.retained_recent_message_count,
        small.stats.retained_recent_message_count
    );
    assert!(large.stats.retained_recent_message_count >= 10);
    assert!(estimate_messages_tokens(&large.messages) <= 6_000);
}

// A long window must bound the rebuild of the checkpoint payload, not only
// the checkpoint text: before the typed entries were bounded, this fixture
// produced a 32_619-token summary whose 2_041 replayed messages were ~944K
// tokens, so the "compacted" request was still clamped to the full budget.
#[test]
fn a_million_token_window_keeps_a_real_transcript_tail_without_resending_it_all() {
    // ~1.5M tokens of transcript against a 1M-token window budget. The
    // request must collapse to the checkpoint plus the capped tail instead
    // of re-sending the whole transcript, and that tail must be a real
    // stretch of transcript rather than the old fixed two messages.
    let mut history = Vec::new();
    for _ in 0..1_500 {
        history.push(AgentMessage::user("x".repeat(2_000)));
        history.push(AgentMessage {
            role: MessageRole::Assistant,
            content: "y".repeat(2_000),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        });
    }
    let budget = context_budget(Some(1_000_000), "", &[], false);
    let result =
        compact_messages_with_current(&history, AgentMessage::user("current request"), budget);
    assert!(result.compressed);
    assert!(result.stats.budget_satisfied);
    assert!(result.stats.original_estimated_tokens > 1_000_000);
    // Checkpoint ceiling (16K) + tail ceiling (24K) + the current request.
    assert!(
        result.stats.compacted_estimated_tokens <= 48_000,
        "compacted request was {} tokens",
        result.stats.compacted_estimated_tokens
    );
    assert!(
        result.stats.retained_recent_message_count >= 40,
        "tail kept only {} messages",
        result.stats.retained_recent_message_count
    );
    assert!(result
        .messages
        .iter()
        .any(|message| message.content == "current request"));
    // The same ceiling has to hold on the restore path: the persisted payload
    // is replayed verbatim on the next round, so a bounded summary whose typed
    // entries were stored untruncated would put the whole transcript back.
    let payload = result
        .checkpoint_payload
        .as_ref()
        .expect("compaction should persist a checkpoint payload");
    // `checkpoint_entries_to_messages` is exactly what
    // `restore_context_checkpoint` replays out of the persisted payload.
    let replayed = checkpoint_entries_to_messages(&payload.entries);
    let replayed_tokens = estimate_messages_tokens(&replayed);
    assert!(
        replayed_tokens <= CHECKPOINT_TOKEN_CEILING,
        "restored checkpoint was {replayed_tokens} tokens"
    );
}

// The bug that started this was not a single oversized request but a whole
// session: round after round re-sent the accumulated transcript. Simulate the
// agent loop — the compaction result becomes the next round's history, exactly
// as `turn.rs` persists it — and require that no round injects more than the
// trigger, however long the session runs and whatever the window size.
#[test]
fn repeated_rounds_keep_every_injected_request_below_the_trigger() {
    for window in [200_000u32, 1_000_000] {
        let budget = context_budget(Some(window), "", &[], false);
        let trigger = compaction_trigger_tokens(budget);
        let mut history = Vec::new();
        let mut peak = 0usize;
        let mut pre_peak = 0usize;
        let mut compacted_rounds = 0usize;
        // 恒定 80K 上限下，压缩后的注入量天然远低于触发点，所以另记「压缩前」
        // 的历史规模，用来证明夹具确实走到了需要压缩的区间（否则下面的断言会
        // 因为夹具太小而永远成立）。
        for round in 0..20 {
            for step in 0..8 {
                history.push(AgentMessage::user(format!(
                    "round {round} step {step} {}",
                    "x".repeat(30_000)
                )));
                history.push(AgentMessage {
                    role: MessageRole::Assistant,
                    content: format!("answer {round} {step} {}", "y".repeat(30_000)),
                    reasoning_content: None,
                    responses_reasoning_items: Vec::new(),
                    anthropic_thinking_blocks: Vec::new(),
                    anthropic_content_blocks: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    source_message_id: None,
                });
            }
            let current = AgentMessage::user(format!("current request {round}"));
            pre_peak = pre_peak.max(estimate_messages_tokens(&history));
            let result = compact_messages_with_current(&history, current, budget);
            let injected = estimate_messages_tokens(&result.messages);
            peak = peak.max(injected);
            if result.compressed {
                compacted_rounds += 1;
            }
            assert!(
                injected <= trigger,
                "window {window} round {round} injected {injected} tokens (trigger {trigger})"
            );
            assert!(
                result
                    .messages
                    .iter()
                    .any(|message| message.content == format!("current request {round}")),
                "window {window} round {round} lost the current request"
            );
            history = result.messages;
        }
        assert!(
            compacted_rounds >= 3,
            "window {window} only compacted {compacted_rounds} rounds"
        );
        assert!(
                pre_peak > trigger / 2,
                "window {window} never grew into the trigger band (pre-compaction peak {pre_peak}, trigger {trigger})"
            );
    }
}

#[test]
fn compaction_starts_before_the_prompt_reaches_the_whole_window() {
    // 恒定 80K 上限：任何窗口都压到 80K（再受窗口 80% 的兜底约束），
    // 也就是「不超过八万就压」——大窗口不会被放到 400K 才压。
    assert_eq!(compaction_trigger_tokens(991_808), 80_000);
    assert_eq!(compaction_trigger_tokens(1_000_000), 80_000);
    assert_eq!(compaction_trigger_tokens(200_000), 80_000);
    assert_eq!(compaction_trigger_tokens(250_000), 80_000);
    assert_eq!(compaction_trigger_tokens(300_000), 80_000);
    assert_eq!(compaction_trigger_tokens(512_000), 80_000);
    // Windows whose proportional line stays under the ceiling are untouched.
    assert_eq!(compaction_trigger_tokens(64_000), 51_200);
    assert_eq!(compaction_trigger_tokens(124_000), 80_000);
    assert_eq!(compaction_trigger_tokens(128_000), 80_000);
    // Smaller budgets keep proportional headroom.
    assert_eq!(compaction_trigger_tokens(1_000), 800);
    assert_eq!(compaction_trigger_tokens(160), 128);
    assert_eq!(compaction_trigger_tokens(1), 1);

    // 16 messages of ~60 tokens: inside the 1_000-token budget but past the
    // 800-token trigger, which used to pass through untouched.
    let mut messages = vec![AgentMessage::system("system")];
    for index in 0..15 {
        messages.push(AgentMessage::user(format!(
            "turn {index} {}",
            "x".repeat(200)
        )));
    }
    let tokens = estimate_messages_tokens(&messages);
    assert!(
        tokens > compaction_trigger_tokens(1_000) && tokens <= 1_000,
        "fixture was {tokens} tokens"
    );
    let result = compact_messages(&messages, 1_000);
    assert!(result.compressed);
    assert!(result.stats.compacted_estimated_tokens <= 1_000);
    assert!(
        result.stats.compacted_estimated_tokens <= compaction_trigger_tokens(1_000),
        "compacted request was {} tokens",
        result.stats.compacted_estimated_tokens
    );

    let untouched = compact_messages(&[AgentMessage::user("short prompt")], 1_000);
    assert!(!untouched.compressed);
}

#[test]
fn the_trigger_ceiling_bounds_the_whole_request_not_just_its_messages() {
    // A million-token window with 23K of fixed overhead (a large tool set
    // plus the runtime context) must trim the message line by that overhead,
    // otherwise the billed request still peaks above the ceiling.
    assert_eq!(compaction_trigger_tokens_with_overhead(991_808, 0), 80_000);
    assert_eq!(
        compaction_trigger_tokens_with_overhead(991_808, 23_000),
        57_000
    );
    // 小窗口仍受自身 80% 兜底：64K 的相对线 51_200 比 80K 上限更低，
    // 扣掉开销后 51_200 − 23K = 28_200 胜出。
    assert_eq!(
        compaction_trigger_tokens_with_overhead(64_000, 23_000),
        28_200
    );
    // Without overhead the relative line survives untouched.
    assert_eq!(compaction_trigger_tokens_with_overhead(64_000, 0), 51_200);
    // A tool set larger than the ceiling cannot collapse the line to zero.
    assert_eq!(
        compaction_trigger_tokens_with_overhead(991_808, 500_000),
        COMPACTION_TRIGGER_MIN_TOKENS
    );
    // The drift correction composes with the overhead discount.
    assert_eq!(
        compaction_trigger_tokens_scaled_with_overhead(991_808, 23_000, Some(120)),
        47_500
    );
    assert_eq!(
        compaction_trigger_tokens_scaled_with_overhead(991_808, 23_000, None),
        57_000
    );
}

// A request can cross the trigger while the compactible base (history plus
// the current request) sits just under it: the in-flight tool suffix after
// the last user message cannot be summarized away. Measured on a 1,623
// message session that combination made every tool step announce a
// compaction that replaced an identical transcript (base ~139K, suffix
// ~21K, request ~174K against a 155K line). 现在的恒定上限是 80K，所以
// 这个现场按比例缩小到同一形态：base 略低于 80K，尾部工具后缀把它顶过去。
#[test]
fn a_base_just_under_the_trigger_still_compacts_when_the_suffix_crosses_it() {
    let trigger = compaction_trigger_tokens(1_000_000);
    assert_eq!(trigger, 80_000);
    let mut messages = Vec::new();
    for index in 0..7 {
        messages.push(AgentMessage::user(format!(
            "old note {index} {}",
            "x".repeat(20_000)
        )));
        messages.push(AgentMessage {
            role: MessageRole::Assistant,
            content: format!("old answer {index} {}", "y".repeat(20_000)),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: None,
        });
    }
    let base = estimate_messages_tokens(&messages);
    messages.push(AgentMessage::user("CURRENT"));
    for index in 0..4 {
        messages.push(AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: format!("call-{index}"),
            name: "read_file".to_string(),
            input: serde_json::json!({ "path": format!("file-{index}.rs") }),
        }));
        messages.push(AgentMessage::tool(
            format!("call-{index}"),
            "z".repeat(20_000),
        ));
    }
    let total = estimate_messages_tokens(&messages);
    assert!(
        base < trigger && total > trigger,
        "fixture base {base}, total {total}, trigger {trigger}"
    );
    let result = compact_messages_with_trigger(&messages, 1_000_000, trigger);
    assert!(
        result.compressed,
        "a request over the trigger must compact its base"
    );
    assert!(result.covered_message_count.unwrap_or_default() > 0);
    assert!(
        result.stats.compacted_estimated_tokens < result.stats.original_estimated_tokens,
        "compaction kept the transcript identical: {} → {}",
        result.stats.original_estimated_tokens,
        result.stats.compacted_estimated_tokens
    );
    assert!(result.stats.checkpoint_estimated_tokens > 0);
}

// The suffix alone can push a request over the trigger with nothing
// compactible above it: no compaction may be announced, and no summary
// round-trip paid, for a rewrite that leaves the transcript identical.
// P8-A raised the 1M-window trigger to 400K, so the fixture doubled.
#[test]
fn an_uncompactible_suffix_is_not_reported_as_a_compaction() {
    let trigger = compaction_trigger_tokens(1_000_000);
    let mut messages = vec![AgentMessage::user("CURRENT")];
    for index in 0..80 {
        messages.push(AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: format!("call-{index}"),
            name: "read_file".to_string(),
            input: serde_json::json!({ "path": format!("file-{index}.rs") }),
        }));
        messages.push(AgentMessage::tool(
            format!("call-{index}"),
            "z".repeat(20_000),
        ));
    }
    let total = estimate_messages_tokens(&messages);
    assert!(total > trigger, "fixture was {total} tokens");
    let result = compact_messages_with_trigger(&messages, 1_000_000, trigger);
    assert!(!result.compressed);
    assert_eq!(result.status, CompactionStatus::NotRequired);
    assert_eq!(result.covered_message_count, None);
    assert_eq!(
        result.stats.compacted_estimated_tokens,
        result.stats.original_estimated_tokens
    );
}

// The provider charges more than the local estimate on tool-heavy
// transcripts, so an estimate-only trigger lets the real prompt overshoot the
// intended line by that drift. Scaling the trigger by the measured drift
// moves the decision back to the same real cost.
#[test]
fn compaction_trigger_shrinks_by_the_provider_measured_drift() {
    assert_eq!(compaction_trigger_tokens_scaled(991_808, None), 80_000);
    // The provider charged 20% more than we estimated for the same request.
    assert_eq!(compaction_trigger_tokens_scaled(991_808, Some(120)), 66_666);
    assert_eq!(compaction_trigger_tokens_scaled(1_000, Some(125)), 640);
    // Charging less than the estimate never raises the line, and one absurd
    // report cannot collapse the trigger to nothing.
    assert_eq!(compaction_trigger_tokens_scaled(991_808, Some(90)), 80_000);
    assert_eq!(
        compaction_trigger_tokens_scaled(991_808, Some(10_000)),
        20_000
    );
    assert_eq!(compaction_trigger_tokens_scaled(1, Some(120)), 1);

    // 12 messages of ~61 tokens sit above the drift-corrected line (533) for a
    // 1_000-token budget but below the plain 800-token trigger.
    let mut messages = vec![AgentMessage::system("system")];
    for index in 0..12 {
        messages.push(AgentMessage::user(format!(
            "turn {index} {}",
            "x".repeat(200)
        )));
    }
    let tokens = estimate_messages_tokens(&messages);
    let scaled = compaction_trigger_tokens_scaled(1_000, Some(150));
    assert!(
        tokens > scaled && tokens <= compaction_trigger_tokens(1_000),
        "fixture was {tokens} tokens (scaled line {scaled})"
    );
    assert!(!compact_messages(&messages, 1_000).compressed);
    let drifted = compact_messages_with_trigger(&messages, 1_000, scaled);
    assert!(drifted.compressed && drifted.stats.budget_satisfied);
}

#[test]
fn semantic_checkpoint_prioritizes_constraints_pending_work_and_tool_sources() {
    let history = vec![
        AgentMessage::system(format!(
            "背景资料。{}。必须始终保留 workspace 边界，禁止访问边界外文件。",
            "无关说明".repeat(120)
        )),
        AgentMessage::user("先分析问题。未完成：修复队列竞态；不要在讨论确认前发布。"),
        AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "call-evidence".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path":"src/queue.rs"}),
        }),
        AgentMessage::tool(
            "call-evidence",
            serde_json::json!({
                "path": "src/queue.rs",
                "success": true,
                "message": "queue removes the item before acquiring the guard"
            })
            .to_string(),
        ),
        AgentMessage {
            role: MessageRole::Assistant,
            content: "结论：存在先出队后抢锁的竞态。".to_string(),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: Some("assistant-1".to_string()),
        },
    ];
    let checkpoint = build_semantic_checkpoint(&history, 900);
    assert!(checkpoint.text.contains("[系统约束]"));
    assert!(checkpoint.text.contains("workspace 边界"));
    assert!(checkpoint.text.contains("[用户未完成事项与约束]"));
    assert!(checkpoint.text.contains("修复队列竞态"));
    assert!(checkpoint.text.contains("[工具证据来源]"));
    assert!(checkpoint.text.contains("tool=read_file"));
    assert!(checkpoint.text.contains("src/queue.rs"));
    assert!(checkpoint.text.contains("call=call-evidence"));
    assert!(estimate_text_tokens(&checkpoint.text) <= 900);
    assert_eq!(checkpoint.system_constraint_count, 1);
    assert_eq!(checkpoint.pending_user_item_count, 1);
    assert_eq!(checkpoint.tool_evidence_count, 1);
}

#[test]
fn long_history_compaction_is_bounded_and_reports_checkpoint_stats() {
    let mut history = vec![AgentMessage::system(
        "必须保留用户选择的执行环境；禁止静默切换目标。".repeat(20),
    )];
    for turn in 0..24 {
        history.push(AgentMessage::user(format!(
            "任务 {turn}：检查队列并保留未完成事项。{}",
            "上下文".repeat(80)
        )));
        history.push(AgentMessage {
            role: MessageRole::Assistant,
            content: format!("第 {turn} 轮观察：{}", "历史结果".repeat(80)),
            reasoning_content: None,
            responses_reasoning_items: Vec::new(),
            anthropic_thinking_blocks: Vec::new(),
            anthropic_content_blocks: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            source_message_id: Some(format!("assistant-{turn}")),
        });
    }
    let result = compact_messages_with_current(
        &history,
        AgentMessage::user("当前请求：给出最终修复方案"),
        900,
    );
    assert!(result.compressed);
    assert!(result.stats.budget_satisfied);
    assert!(estimate_messages_tokens(&result.messages) <= 900);
    assert!(result.stats.covered_message_count > 0);
    assert!(result.stats.original_estimated_tokens > result.stats.compacted_estimated_tokens);
    assert_eq!(
        result.stats.compacted_estimated_tokens,
        estimate_messages_tokens(&result.messages)
    );
    assert!(result.summary.as_deref().is_some_and(|summary| {
        summary.contains("系统约束") && summary.contains("用户未完成事项")
    }));
    assert!(result
        .messages
        .last()
        .is_some_and(|message| message.content == "当前请求：给出最终修复方案"));
}

#[test]
fn recent_parallel_tool_sequence_stays_well_formed_under_pressure() {
    let recent_calls = AgentMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        reasoning_content: None,
        responses_reasoning_items: Vec::new(),
        anthropic_thinking_blocks: Vec::new(),
        anthropic_content_blocks: Vec::new(),
        tool_call_id: None,
        tool_calls: vec![
            crate::AgentToolCall {
                id: "call-a".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path":"a.rs"}),
            },
            crate::AgentToolCall {
                id: "call-b".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path":"b.rs"}),
            },
        ],
        source_message_id: None,
    };
    let history = vec![
        AgentMessage::user("old context ".repeat(600)),
        recent_calls,
        AgentMessage::tool("call-a", "A".repeat(1_200)),
        AgentMessage::tool("call-b", "B".repeat(1_200)),
    ];
    let result = compact_messages_with_current(
        &history,
        AgentMessage::user("continue with the evidence"),
        320,
    );
    assert!(result.stats.budget_satisfied);
    assert!(estimate_messages_tokens(&result.messages) <= 320);
    let call_index = result
        .messages
        .iter()
        .position(|message| !message.tool_calls.is_empty())
        .expect("recent tool call should remain");
    let tool_indices = result
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == MessageRole::Tool).then_some(index))
        .collect::<Vec<_>>();
    assert!(!tool_indices.is_empty());
    assert!(tool_indices.iter().all(|index| *index > call_index));
}

#[test]
fn a_single_oversized_current_message_is_explicitly_truncated_to_budget() {
    let result = compact_messages_with_current(
        &[],
        AgentMessage::user("current payload ".repeat(20_000)),
        128,
    );
    assert_eq!(result.status, CompactionStatus::TruncatedToBudget);
    assert!(result.summary.is_none());
    assert!(result.stats.budget_satisfied);
    assert!(estimate_messages_tokens(&result.messages) <= 128);
    assert!(result
        .messages
        .last()
        .is_some_and(|message| message.content.contains("消息内容已压缩")));
}

#[test]
fn impossible_budget_reports_failure_without_splitting_the_latest_tool_group() {
    let history = vec![
        AgentMessage::system("必须保留安全边界。".repeat(80)),
        AgentMessage::user("未完成：修复上下文压缩。".repeat(80)),
        AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "call-budget".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path":"context.rs"}),
        }),
        AgentMessage::tool("call-budget", "evidence ".repeat(400)),
    ];
    for budget in [32, 64, 128, 256, 512] {
        let result =
            compact_messages_with_current(&history, AgentMessage::user("current request"), budget);
        let estimated = estimate_messages_tokens(&result.messages);
        assert_eq!(result.stats.budget_satisfied, estimated <= budget);
        let call_index = result
            .messages
            .iter()
            .position(|message| !message.tool_calls.is_empty())
            .expect("latest tool call must be retained");
        let result_index = result
            .messages
            .iter()
            .position(|message| message.tool_call_id.as_deref() == Some("call-budget"))
            .expect("latest tool result must be retained");
        assert!(result_index > call_index);
        assert!(result
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User));
    }
}

#[test]
fn history_quarantines_only_assistant_planning_without_changing_the_source() {
    let content = "已核对设备状态。\nThe user wants me to continue. Based on the conversation, I need to:\n1. Use tools\n\n已完成检查。";
    let mut turn = AgentContextTurn {
        role: "assistant".to_string(),
        content: content.to_string(),
        source: None,
        message_id: Some("history-1".to_string()),
        turn_id: None,
        model_id: None,
        attachments: Vec::new(),
        artifacts: Vec::new(),
    };
    let message = context_turn_message(&turn).unwrap();
    assert_eq!(message.content, "已核对设备状态。\n已完成检查。");
    assert_eq!(message.source_message_id, turn.message_id);
    assert_eq!(turn.content, content);
    for role in ["user", "system"] {
        turn.role = role.to_string();
        assert_eq!(context_turn_message(&turn).unwrap().content, content);
    }
    turn.role = "assistant".to_string();
    turn.content =
        "The user wants me to continue. Based on the conversation, I need to:\n1. Use tools"
            .to_string();
    assert!(context_turn_message(&turn).is_none());
}

#[test]
fn current_prompt_already_present_as_last_history_turn_is_not_duplicated() {
    let context = vec![
        AgentContextTurn {
            role: "assistant".to_string(),
            content: "previous answer".to_string(),
            source: None,
            message_id: Some("assistant-1".to_string()),
            turn_id: None,
            model_id: None,
            attachments: Vec::new(),
            artifacts: Vec::new(),
        },
        AgentContextTurn {
            role: "user".to_string(),
            content: "run the Windows check".to_string(),
            source: None,
            message_id: Some("user-current".to_string()),
            turn_id: None,
            model_id: None,
            attachments: Vec::new(),
            artifacts: Vec::new(),
        },
    ];

    let result = build_agent_messages_detailed(&context, "run the Windows check", 4_096);
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::User && message.content == "run the Windows check"
            })
            .count(),
        1
    );
}

#[test]
fn unknown_window_retry_budget_subtracts_all_fixed_overhead() {
    let tools = vec![ToolDefinition::new(
        "read_file",
        "read a file",
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
    )];
    let budget = unknown_context_retry_budget(3, &"system ".repeat(200), &tools, 1_500, false)
        .expect("16K target should retain a message budget");
    assert_eq!(budget.target_context_window, 16_000);
    assert_eq!(
        budget.message_budget + budget.fixed_overhead_tokens,
        budget.target_context_window
    );
    assert!(budget.fixed_overhead_tokens > budget.reserved_output_tokens + 1_500);
    assert!(
        unknown_context_retry_budget(3, &"large system ".repeat(20_000), &tools, 8_000, false,)
            .is_none()
    );
}

#[test]
fn typed_checkpoint_restores_original_roles_and_rejects_model_mismatch() {
    let checkpoint = AgentContextCheckpoint {
        id: "checkpoint-1".to_string(),
        summary: "legacy summary".to_string(),
        evidence_policy_version: 2,
        covered_through_message_id: Some("tool-result".to_string()),
        payload: Some(AgentContextCheckpointPayload {
            format_version: CONTEXT_CHECKPOINT_FORMAT_VERSION,
            producer_model_id: Some("model-a".to_string()),
            entries: vec![
                AgentContextCheckpointEntry::SystemConstraint {
                    content: "system constraint".to_string(),
                    source_message_id: Some("system-1".to_string()),
                },
                AgentContextCheckpointEntry::UserConstraint {
                    content: "user constraint".to_string(),
                    source_message_id: Some("user-1".to_string()),
                },
                AgentContextCheckpointEntry::AssistantConclusion {
                    content: "assistant conclusion".to_string(),
                    source_message_id: Some("assistant-1".to_string()),
                },
                AgentContextCheckpointEntry::ToolEvidence {
                    call_id: "call-1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path":"a.rs"}),
                    output: "evidence".to_string(),
                    source_message_id: Some("tool-result".to_string()),
                },
            ],
        }),
    };

    let restored =
        restore_context_checkpoint(&checkpoint, Some("model-a")).expect("compatible checkpoint");
    assert_eq!(
        restored
            .iter()
            .map(|message| message.role.clone())
            .collect::<Vec<_>>(),
        vec![
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Assistant,
            MessageRole::Tool,
        ]
    );
    assert_eq!(
        restore_context_checkpoint(&checkpoint, Some("model-b")),
        Some(restored)
    );
    assert_eq!(
        legacy_context_checkpoint_message(&checkpoint)
            .expect("legacy fallback")
            .role,
        MessageRole::Assistant
    );
}

#[test]
fn million_token_history_compacts_without_dropping_the_current_request() {
    let mut history = Vec::with_capacity(8_000);
    for index in 0..8_000 {
        let content = format!("message-{index} {}", "context ".repeat(64));
        history.push(if index % 2 == 0 {
            AgentMessage::user(content)
        } else {
            AgentMessage {
                role: MessageRole::Assistant,
                content,
                reasoning_content: None,
                responses_reasoning_items: Vec::new(),
                anthropic_thinking_blocks: Vec::new(),
                anthropic_content_blocks: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                source_message_id: Some(format!("message-{index}")),
            }
        });
    }
    assert!(estimate_messages_tokens(&history) > 1_000_000);

    let result =
        compact_messages_with_current(&history, AgentMessage::user("current request"), 4_096);
    assert!(result.stats.budget_satisfied);
    assert!(result
        .messages
        .iter()
        .any(|message| message.content == "current request"));
}

#[test]
fn in_flight_compaction_preserves_the_current_user_request_before_tool_suffixes() {
    let messages = vec![
        AgentMessage::user("old history ".repeat(1_000)),
        AgentMessage::user("current user request must survive"),
        AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "call-live".to_string(),
            name: "run_command".to_string(),
            input: serde_json::json!({"command":"build"}),
        }),
        AgentMessage::tool("call-live", "live output ".repeat(2_000)),
    ];
    let result = compact_messages(&messages, 160);
    assert!(result.stats.budget_satisfied);
    assert!(estimate_messages_tokens(&result.messages) <= 160);
    assert!(result
        .messages
        .iter()
        .any(|message| message.content == "current user request must survive"));
}

// A request can cross the trigger because of an in-flight tool result that
// cannot be summarized. The base above it can still be compacted, and must
// be: the caller already decided the request is over the line, so leaving it
// at 100% while announcing a compaction is the bug this pins down.
#[test]
fn in_flight_suffix_alone_still_compacts_the_base_it_can_summarize() {
    let mut messages = vec![AgentMessage::system("system")];
    while estimate_messages_tokens(&messages) < 6_600 {
        messages.push(AgentMessage::user("x".repeat(1_340)));
    }
    messages.push(AgentMessage::user("current request"));
    let inner_tokens = estimate_messages_tokens(&messages);
    let suffix = vec![
        AgentMessage::assistant_tool_call(crate::AgentToolCall {
            id: "call-live".to_string(),
            name: "run_command".to_string(),
            input: serde_json::json!({"command":"build"}),
        }),
        AgentMessage::tool("call-live", "z".repeat(12_000)),
    ];
    let suffix_tokens = estimate_messages_tokens(&suffix);
    messages.extend(suffix);

    let budget: usize = 12_000;
    let inner_budget = budget
        .saturating_sub(suffix_tokens)
        .max(budget / 2)
        .min(budget);
    let total = estimate_messages_tokens(&messages);
    // The scenario the test is about: the whole request crosses the trigger
    // while the summarizable part does not, and the fit has no reason to
    // truncate — so only a compaction can bring the request back down.
    assert!(
        total > compaction_trigger_tokens(budget),
        "fixture was {total} tokens, inside the trigger"
    );
    assert!(
        inner_tokens <= compaction_trigger_tokens(inner_budget),
        "fixture left {inner_tokens} tokens to compact (inner line {})",
        compaction_trigger_tokens(inner_budget)
    );
    assert!(total <= budget, "fixture was {total} tokens, over budget");

    let result = compact_messages(&messages, budget);
    assert!(
        result.compressed,
        "a request over the trigger must compact the base it can summarize"
    );
    assert!(result.stats.covered_message_count > 0);
    let compacted = estimate_messages_tokens(&result.messages);
    assert!(
        compacted < total,
        "compaction kept the transcript identical at {total} tokens"
    );
    assert!(compacted <= budget, "compacted to {compacted} tokens");
}

#[test]
fn opaque_reasoning_items_are_included_in_budget_estimation() {
    let mut message = AgentMessage::user("request");
    let without_reasoning = estimate_message_tokens(&message);
    message.responses_reasoning_items = vec![serde_json::json!({
        "type": "reasoning",
        "payload": "x".repeat(4_000)
    })];
    assert!(estimate_message_tokens(&message) > without_reasoning + 900);
}

#[test]
fn claude_latest_tool_thinking_is_never_truncated_or_stripped_to_fit_a_window() {
    let signed =
        serde_json::json!({"type":"thinking", "thinking":"", "signature":"opaque".repeat(2000)});
    let mut assistant = AgentMessage::assistant_tool_call(crate::AgentToolCall {
        id: "call-live".into(),
        name: "read_file".into(),
        input: serde_json::json!({}),
    });
    assistant.anthropic_thinking_blocks = vec![signed.clone()];
    let history = vec![
        AgentMessage::user("current request"),
        assistant,
        AgentMessage::tool("call-live", "result"),
    ];
    let result = compact_messages(&history, 128);
    assert!(!result.stats.budget_satisfied);
    let retained = result
        .messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .unwrap();
    assert_eq!(retained.anthropic_thinking_blocks, vec![signed]);
}
