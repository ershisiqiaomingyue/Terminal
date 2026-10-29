use super::super::*;

/// Save a transcript even when the turn ends with tools or a stop request.
/// An empty response is the one case that has no trustworthy assistant text.
pub(in crate::agent::turn) async fn save_transcript(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    termination_reason: &str,
    messages: &[AgentMessage],
    transcript_scope: String,
    runtime_context: String,
    image_attachments: &[ProviderImageAttachment],
    cache_fingerprint: Option<agent_core::PromptCacheFingerprint>,
) {
    if !matches!(termination_reason, "model_response_missing")
        && messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
    {
        // P9-B：转录保存不再要求「以无工具调用的 assistant 结尾」——工具调用
        // 结尾、被取消、重复输出的轮次同样保存，消除「下一轮必从对话历史重建、
        // 前缀必变」的缓存归零源。仅 model_response_missing（本轮没有可信的
        // 模型输出）仍跳过。
        cache_context::save(
            app,
            session_id,
            turn_id,
            transcript_scope,
            runtime_context,
            messages.to_vec(),
            image_attachments,
            cache_fingerprint,
        )
        .await;
    }
}
