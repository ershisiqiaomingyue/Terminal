//! Bounded model-response recovery without replaying side-effecting tools.
use crate::{AgentMessage, MessageRole};
use serde_json::json;

pub const MODEL_RESPONSE_RECOVERY_LIMIT: usize = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum ProgressRecovery {
    NotNeeded,
    Retry,
    Exhausted,
}

/// Runtime conditions shared by all provider protocols. A final-only or
/// truncated response must not restart execution, nor may a tool-disabled turn.
#[derive(Clone, Copy)]
pub struct ProgressRecoveryContext {
    pub tools_available: bool,
    pub final_only: bool,
    pub output_truncated: bool,
}

#[derive(Default)]
pub struct ModelResponseRecovery {
    attempts: usize,
    progress_attempts: usize,
}

impl ModelResponseRecovery {
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    pub fn progress_attempts(&self) -> usize {
        self.progress_attempts
    }

    /// Space out empty-answer continuations; never change the selected model
    /// or reasoning level, and never replay an already executed tool.
    pub fn retry_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.attempts.min(MODEL_RESPONSE_RECOVERY_LIMIT) as u64)
    }

    /// A short action announcement is not a finished task. This only sends a
    /// continuation back to the model; it never derives or executes a tool call.
    pub fn retry_progress_only(
        &mut self,
        messages: &mut Vec<AgentMessage>,
        response: &AgentMessage,
        context: ProgressRecoveryContext,
    ) -> ProgressRecovery {
        if !context.tools_available
            || context.final_only
            || context.output_truncated
            || response.role != MessageRole::Assistant
            || !response.tool_calls.is_empty()
            || !is_action_announcement(&response.content)
        {
            return ProgressRecovery::NotNeeded;
        }
        if self.progress_attempts >= MODEL_RESPONSE_RECOVERY_LIMIT {
            return ProgressRecovery::Exhausted;
        }
        self.progress_attempts += 1;
        messages.push(response.clone());
        messages.push(AgentMessage::user(
            "刚才只给出了准备执行的进度说明，没有发起工具调用。请继续完成当前用户要求：如果仍需查看或操作，直接发起真实的原生工具调用；若缺少必要信息，调用 ask_user；若无法继续，明确说明阻碍。已经完成的操作不要重复执行，不要再次只说准备做什么就结束。",
        ));
        ProgressRecovery::Retry
    }

    /// Preserve the provider's complete assistant response, including Claude
    /// thinking blocks, and pair every unexecuted call before requesting recovery.
    ///
    /// `thinking_only` marks the round that reported output tokens but produced no
    /// visible text: on protocols that count reasoning as output, the whole budget
    /// went into thinking. That needs its own wording — "do not call tools" alone
    /// still invites another round of reasoning and the answer never starts.
    pub fn retry(
        &mut self,
        messages: &mut Vec<AgentMessage>,
        response: AgentMessage,
        final_only: bool,
        thinking_only: bool,
    ) -> bool {
        debug_assert_eq!(response.role, MessageRole::Assistant);
        let rejected_calls = response.tool_calls.clone();
        if !response.content.trim().is_empty()
            || response.reasoning_content.is_some()
            || !response.responses_reasoning_items.is_empty()
            || !response.anthropic_thinking_blocks.is_empty()
            || !response.anthropic_content_blocks.is_empty()
            || !response.tool_calls.is_empty()
        {
            messages.push(response);
        }
        for call in rejected_calls {
            messages.push(AgentMessage::tool(
                &call.id,
                json!({
                    "skipped": true,
                    "error": {
                        "code": "final_phase_tool_call",
                        "message": "当前仅整理已执行结果，此工具未执行。请直接给出最终回答。",
                    },
                })
                .to_string(),
            ));
        }
        if self.attempts >= MODEL_RESPONSE_RECOVERY_LIMIT {
            return false;
        }
        self.attempts += 1;
        // Claude hoists system messages to the top-level prefix. A runtime
        // continuation must stay after the response/results, rather than leave
        // an empty or thinking-only assistant as the request's final message.
        messages.push(AgentMessage::user(if final_only {
            if thinking_only {
                "上一轮只有思考过程，没有输出任何正文，输出预算已被推理耗尽。请立即停止思考，不要再做任何分析或计划，直接开始写最终回答的正文：说明已完成的操作、失败原因和尚未完成的工作，不要再次调用工具。"
            } else {
                "上一轮未给出有效的最终回答。请依据已有工具结果输出文字，说明已完成的操作、失败原因和尚未完成的工作，不要再次调用工具。"
            }
        } else {
            "上一轮没有返回文字，也没有请求工具，任务可能尚未完成。请继续处理用户要求；需要操作时调用工具，完成后给出明确结论。不要把先前的进度说明当成最终回答。"
        }));
        true
    }
}

/// Deliberately narrow: ordinary answers, proposals, questions, failure reports,
/// and mixed progress/results must not trigger another model request.
fn is_action_announcement(content: &str) -> bool {
    let content = content
        .trim()
        .strip_prefix("[历史助手陈述，非工具证据]")
        .unwrap_or(content.trim())
        .trim();
    if content.is_empty()
        || content.chars().take(401).count() > 400
        || content.contains(['?', '？', ':', '：'])
        || content.contains("```")
        || [
            "建议",
            "如果",
            "可以",
            "请你",
            "请提供",
            "需要你",
            "无法",
            "不能",
            "失败",
            "已完成",
            "已读取",
            "已找到",
            "已确认",
            "已经",
            "发现",
            "查到",
            "找到",
            "读取到",
            "完成了",
            "结果",
            "结论",
        ]
        .iter()
        .any(|word| content.contains(word))
    {
        return false;
    }
    let paragraphs: Vec<_> = content
        .split_inclusive(['。', '！', ';', '；', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if paragraphs.len() > 4 {
        return false;
    }
    paragraphs.iter().all(|line| {
        let action = [
            "我先",
            "我会先",
            "我将先",
            "让我先",
            "接下来我会",
            "接下来我将",
            "接下来我要",
            "接下来",
            "现在我会",
            "现在我要",
            "现在我来",
            "现在",
            "接着我会",
            "接着",
            "我来",
            "我将",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .or_else(|| {
            line.contains("再决定")
                .then(|| line.strip_prefix("先"))
                .flatten()
        });
        action.is_some_and(|action| {
            [
                "查看", "看", "检查", "读取", "列出", "列", "查找", "搜索", "运行", "执行", "修改",
                "打开", "测试", "确认", "写入", "写", "创建", "新建", "补充", "调整", "实现",
            ]
            .iter()
            .find_map(|verb| action.strip_prefix(verb))
            .is_some_and(|rest| !rest.starts_with(['了', '好', '完', '过', '的']))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentToolCall, ProviderConfig, ProviderKind, ProviderRequest};

    fn execution_context() -> ProgressRecoveryContext {
        ProgressRecoveryContext {
            tools_available: true,
            final_only: false,
            output_truncated: false,
        }
    }

    fn assistant(content: &str) -> AgentMessage {
        let mut message = AgentMessage::user(content);
        message.role = MessageRole::Assistant;
        message
    }

    #[test]
    fn empty_tool_boundary_can_continue_work_instead_of_forcing_final_only() {
        let mut messages = vec![AgentMessage::tool("edit", "file edited")];
        let mut recovery = ModelResponseRecovery::default();
        assert!(recovery.retry(&mut messages, assistant(""), false, false));
        assert!(messages
            .last()
            .unwrap()
            .content
            .contains("需要操作时调用工具"));
        assert_eq!(messages[0].content, "file edited");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, MessageRole::User);
    }

    #[test]
    fn final_tool_calls_are_paired_and_thinking_is_preserved_on_every_retry() {
        let mut recovery = ModelResponseRecovery::default();
        let mut messages = Vec::new();
        for attempt in 0..3 {
            let mut response = assistant("");
            response.anthropic_thinking_blocks = vec![json!({
                "type": "thinking", "thinking": "prepare result", "signature": "signature"
            })];
            response.tool_calls = vec![AgentToolCall {
                id: format!("call-{attempt}"),
                name: "write_local_file".into(),
                input: json!({"path":"source.cpp", "content":"new"}),
            }];
            response.anthropic_content_blocks = vec![
                response.anthropic_thinking_blocks[0].clone(),
                json!({"type":"tool_use", "id":format!("call-{attempt}"),
                    "name":"write_local_file", "input":{"path":"source.cpp", "content":"new"}}),
            ];
            let original_blocks = response.anthropic_content_blocks.clone();
            let start = messages.len();
            assert_eq!(
                recovery.retry(&mut messages, response, true, false),
                attempt < 2
            );
            assert_eq!(messages[start].anthropic_thinking_blocks.len(), 1);
            assert_eq!(messages[start].anthropic_content_blocks, original_blocks);
            assert_eq!(
                messages[start + 1].tool_call_id.as_deref(),
                Some(format!("call-{attempt}").as_str())
            );
            let result: serde_json::Value =
                serde_json::from_str(&messages[start + 1].content).unwrap();
            assert_eq!(result["error"]["code"], "final_phase_tool_call");
        }
    }

    #[test]
    fn final_empty_responses_get_two_retries_even_when_final_phase_is_already_active() {
        let mut recovery = ModelResponseRecovery::default();
        let mut messages = Vec::new();
        assert_eq!(recovery.retry_delay(), std::time::Duration::ZERO);
        assert!(recovery.retry(&mut messages, assistant(""), true, false));
        assert_eq!(recovery.attempts(), 1);
        assert_eq!(recovery.retry_delay(), std::time::Duration::from_secs(1));
        assert!(recovery.retry(&mut messages, assistant(""), true, false));
        assert_eq!(recovery.attempts(), 2);
        assert_eq!(recovery.retry_delay(), std::time::Duration::from_secs(2));
        assert!(!recovery.retry(&mut messages, assistant(""), true, false));
        assert_eq!(recovery.attempts(), MODEL_RESPONSE_RECOVERY_LIMIT);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn thinking_only_rounds_ask_the_model_to_stop_reasoning_and_write() {
        let mut recovery = ModelResponseRecovery::default();
        let mut messages = Vec::new();
        let mut response = assistant("");
        response.reasoning_content = Some("反复推敲了很久".into());
        assert!(recovery.retry(&mut messages, response, true, true));
        let prompt = &messages.last().unwrap().content;
        assert!(prompt.contains("停止思考"), "{prompt}");
        assert!(prompt.contains("直接开始写最终回答的正文"), "{prompt}");
        assert!(prompt.contains("不要再次调用工具"), "{prompt}");
        // 真正空回复的措辞不变：不能把「没写」误诊成「想太多」。
        let mut plain = Vec::new();
        let mut plain_recovery = ModelResponseRecovery::default();
        assert!(plain_recovery.retry(&mut plain, assistant(""), true, false));
        assert!(!plain.last().unwrap().content.contains("停止思考"));
    }

    #[test]
    fn action_announcements_request_real_calls_with_a_bounded_budget() {
        let mut recovery = ModelResponseRecovery::default();
        let mut messages = vec![AgentMessage::tool("previous-edit", "file edited")];
        for attempt in 0..3 {
            let response = assistant("我先看本地 `CarAdapter` 的结构，确认 server 和 host 的判定逻辑位置。\n\n我先列目录确认结构。");
            assert_eq!(
                recovery.retry_progress_only(&mut messages, &response, execution_context()),
                if attempt < 2 {
                    ProgressRecovery::Retry
                } else {
                    ProgressRecovery::Exhausted
                }
            );
        }
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].tool_call_id.as_deref(), Some("previous-edit"));
        assert!(messages[1..].iter().all(|m| m.tool_calls.is_empty()));
        assert!(messages
            .last()
            .unwrap()
            .content
            .contains("已经完成的操作不要重复执行"));
        assert!(is_action_announcement("[历史助手陈述，非工具证据]\n先看清当前 Windows 本地 `CarAdapter` 的结构，再决定怎么修改。"));
    }

    #[test]
    fn next_edit_announcements_after_successful_tools_continue_without_replaying_writes() {
        for text in [
            "现在写布局和文案。",
            "现在我来创建布局文件。",
            "接下来调整页面，再执行测试。",
            "我将补充单元测试。",
        ] {
            let mut messages = vec![AgentMessage::tool("write-activity", "file written")];
            let mut recovery = ModelResponseRecovery::default();
            assert_eq!(
                recovery.retry_progress_only(&mut messages, &assistant(text), execution_context()),
                ProgressRecovery::Retry,
                "{text}"
            );
            assert_eq!(recovery.progress_attempts(), 1);
            assert_eq!(messages.len(), 3);
            assert_eq!(messages[0].tool_call_id.as_deref(), Some("write-activity"));
            assert!(messages.iter().all(|message| message.tool_calls.is_empty()));
        }
    }

    #[test]
    fn answers_questions_proposals_and_failures_are_not_retried_as_progress() {
        for content in [
            "你好",
            "可以先看目录，再调整架构。",
            "建议我先检查目录。",
            "我先查看哪个目录？",
            "我先查看目录，但权限被拒绝，无法继续。",
            "我先检查了目录，已经找到配置文件。",
            "我先检查目录。\n结果是配置缺失。",
            "我先检查了目录，发现缺少配置。",
            "我先读取了文件，发现接口定义不一致。",
            "我先列出了目录。",
            "先检查配置文件。",
            "我先检查目录。目录包含三个项目。",
            "先打开设置，再点击主题，就可以切换。",
            "我先检查目录：请提供项目路径。",
            "现在写好了布局和文案。",
            "现在创建的文件是 MainActivity.java。",
            "现在无法写入布局。",
            "接下来你可以运行测试。",
            "现在写布局和文案。\n\n已完成全部修改。",
        ] {
            assert!(!is_action_announcement(content), "{content}");
        }
        let mut recovery = ModelResponseRecovery::default();
        let mut response = assistant("我先检查目录。");
        response.tool_calls.push(AgentToolCall {
            id: "read".into(),
            name: "list_local_files".into(),
            input: json!({}),
        });
        let mut messages = Vec::new();
        assert_eq!(
            recovery.retry_progress_only(&mut messages, &response, execution_context()),
            ProgressRecovery::NotNeeded
        );
        assert!(messages.is_empty());
    }

    #[test]
    fn final_only_truncated_and_tool_disabled_turns_never_restart_execution() {
        let mut recovery = ModelResponseRecovery::default();
        let mut messages = vec![AgentMessage::user("检查配置")];
        for tools_available in [false, true] {
            for final_only in [false, true] {
                for output_truncated in [false, true] {
                    if tools_available && !final_only && !output_truncated {
                        continue;
                    }
                    assert_eq!(
                        recovery.retry_progress_only(
                            &mut messages,
                            &assistant("我先检查目录。"),
                            ProgressRecoveryContext {
                                tools_available,
                                final_only,
                                output_truncated
                            }
                        ),
                        ProgressRecovery::NotNeeded
                    );
                    assert_eq!(messages.len(), 1);
                }
            }
        }
        // Ineligible rounds must not consume the recovery budget.
        for _ in 0..2 {
            assert_eq!(
                recovery.retry_progress_only(
                    &mut messages,
                    &assistant("我先检查目录。"),
                    execution_context()
                ),
                ProgressRecovery::Retry
            );
        }
    }

    #[test]
    fn continuation_preserves_protocol_replay_and_completed_tools() {
        for kind in [
            ProviderKind::OpenAi,
            ProviderKind::OpenAiCompatible,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
            ProviderKind::Glm,
            ProviderKind::MiMo,
            ProviderKind::OpenAiResponses,
            ProviderKind::Anthropic,
        ] {
            let mut previous = AgentMessage::assistant_tool_call(AgentToolCall {
                id: "previous-read".into(),
                name: "read_file".into(),
                input: json!({"path":"a.txt"}),
            });
            previous.responses_reasoning_items = vec![
                json!({"type":"reasoning", "id":"rs_previous", "encrypted_content":"opaque-previous", "summary":[]}),
            ];
            let mut response = assistant("我先检查目录。");
            response.reasoning_content = Some("reasoning".into());
            response.responses_reasoning_items = vec![
                json!({"type":"reasoning", "id":"rs_current", "encrypted_content":"opaque-current", "summary":[]}),
            ];
            response.anthropic_thinking_blocks =
                vec![json!({"type":"thinking", "thinking":"reasoning", "signature":"signed"})];
            response.anthropic_content_blocks = vec![
                response.anthropic_thinking_blocks[0].clone(),
                json!({"type":"text", "text":response.content}),
            ];
            let original_response = serde_json::to_value(&response).unwrap();
            let mut messages = vec![
                previous,
                AgentMessage::tool("previous-read", "original file contents"),
            ];
            let original_history = serde_json::to_value(&messages).unwrap();
            assert_eq!(
                ModelResponseRecovery::default().retry_progress_only(
                    &mut messages,
                    &response,
                    execution_context()
                ),
                ProgressRecovery::Retry
            );
            assert_eq!(
                serde_json::to_value(&messages[..2]).unwrap(),
                original_history
            );
            assert_eq!(
                serde_json::to_value(&messages[2]).unwrap(),
                original_response
            );
            let config = ProviderConfig::new(
                "provider",
                "Provider",
                kind,
                "https://example.test/v1",
                "credential",
            )
            .unwrap();
            let body = crate::request_body(
                &config,
                &ProviderRequest::new("session", "model", messages, vec![]),
            )
            .unwrap();
            let wire = body[if kind == ProviderKind::OpenAiResponses {
                "input"
            } else {
                "messages"
            }]
            .as_array()
            .unwrap();
            assert_eq!(wire.last().unwrap()["role"], "user");
            match kind {
                ProviderKind::Anthropic => {
                    assert_eq!(wire[0]["content"][0]["id"], "previous-read");
                    assert_eq!(wire[1]["content"][0]["tool_use_id"], "previous-read");
                    assert_eq!(wire[2]["content"][0]["signature"], "signed");
                    assert_eq!(wire[2]["content"][1]["text"], "我先检查目录。");
                }
                ProviderKind::OpenAiResponses => {
                    assert_eq!(wire[0]["encrypted_content"], "opaque-previous");
                    assert_eq!(wire[1]["type"], "function_call");
                    assert_eq!(wire[2]["type"], "function_call_output");
                    assert_eq!(wire[2]["call_id"], "previous-read");
                    assert_eq!(wire[3]["content"][0]["text"], "我先检查目录。");
                }
                _ => {
                    assert_eq!(wire[0]["tool_calls"][0]["id"], "previous-read");
                    assert_eq!(wire[1]["tool_call_id"], "previous-read");
                    assert_eq!(wire[2]["content"], "我先检查目录。");
                }
            }
        }
    }
}
