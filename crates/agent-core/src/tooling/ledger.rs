//! Tool-call deduplication, bounded retries and host-independent result normalization.
use crate::{prune_tool_result_for_model, AgentEvent, AgentMessage};
use serde_json::{json, Value};
use std::collections::HashMap;

const MAX_RETRYABLE_READ_ONLY_TOOL_ATTEMPTS: usize = 2;

/// End a tool loop only after repeated rounds make no executable progress.
/// One redundant read among useful calls must not terminate the entire task.
#[derive(Default)]
pub struct ToolLoopProgress {
    stalled_rounds: usize,
}

impl ToolLoopProgress {
    pub fn observe_round(&mut self, executed: bool, skipped_duplicate: bool) -> bool {
        if executed || !skipped_duplicate {
            self.stalled_rounds = 0;
        } else {
            self.stalled_rounds += 1;
        }
        self.stalled_rounds >= 3
    }
}

/// A write (including a partially failed write) makes previous file reads stale.
/// Keep mutation deduplication and failed-call retry bounds intact.
pub fn invalidate_successful_file_reads(ledger: &mut HashMap<String, ToolCallLedgerEntry>) {
    ledger.retain(|signature, entry| {
        if entry.outcome != ToolCallOutcome::Succeeded {
            return true;
        }
        !serde_json::from_str::<Value>(signature)
            .ok()
            .is_some_and(|value| {
                value["name"]
                    .as_str()
                    .is_some_and(|name| crate::canonical_tool_name(name) == "read_local_file")
            })
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallOutcome {
    Running,
    Succeeded,
    Failed { retryable: bool },
}

#[derive(Debug, Clone, Copy)]
pub struct ToolCallLedgerEntry {
    pub attempts: usize,
    pub outcome: ToolCallOutcome,
}

pub fn tool_call_can_run(
    ledger: &HashMap<String, ToolCallLedgerEntry>,
    signature: &str,
    read_only: bool,
) -> bool {
    let Some(entry) = ledger.get(signature) else {
        return true;
    };
    matches!(
        entry,
        ToolCallLedgerEntry {
            attempts,
            outcome: ToolCallOutcome::Failed { retryable: true },
        } if read_only && *attempts < MAX_RETRYABLE_READ_ONLY_TOOL_ATTEMPTS
    )
}

pub fn tool_call_skip_reason(
    ledger: &HashMap<String, ToolCallLedgerEntry>,
    signature: &str,
) -> String {
    match ledger.get(signature) {
        Some(ToolCallLedgerEntry {
            outcome: ToolCallOutcome::Succeeded,
            ..
        }) => "检测到相同工具和参数已经成功执行，请直接使用已有结果。".to_string(),
        Some(ToolCallLedgerEntry {
            outcome: ToolCallOutcome::Running,
            ..
        }) => "检测到相同工具和参数仍在执行，已跳过重复调用。".to_string(),
        Some(ToolCallLedgerEntry {
            outcome: ToolCallOutcome::Failed { retryable: true },
            ..
        }) => "相同只读工具已达到可重试次数上限，请根据此前错误调整目标或参数。".to_string(),
        Some(ToolCallLedgerEntry {
            outcome: ToolCallOutcome::Failed { retryable: false },
            ..
        }) => "相同工具此前已失败且不适合自动重试，请根据错误调整目标或参数。".to_string(),
        None => "检测到相同工具和参数的重复调用，已跳过重复执行。".to_string(),
    }
}

pub fn begin_tool_call(ledger: &mut HashMap<String, ToolCallLedgerEntry>, signature: String) {
    ledger
        .entry(signature)
        .and_modify(|entry| {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.outcome = ToolCallOutcome::Running;
        })
        .or_insert(ToolCallLedgerEntry {
            attempts: 1,
            outcome: ToolCallOutcome::Running,
        });
}

pub fn complete_tool_call(
    ledger: &mut HashMap<String, ToolCallLedgerEntry>,
    signature: &str,
    events: &[AgentEvent],
) {
    let outcome = events.iter().rev().find_map(|event| {
        let AgentEvent::ToolCallFinished {
            output, is_error, ..
        } = event
        else {
            return None;
        };
        if !is_error {
            return Some(ToolCallOutcome::Succeeded);
        }
        let retryable = output
            .get("retryable")
            .and_then(Value::as_bool)
            .or_else(|| {
                output
                    .get("error")
                    .and_then(|error| error.get("retryable"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false);
        Some(ToolCallOutcome::Failed { retryable })
    });
    if let (Some(entry), Some(outcome)) = (ledger.get_mut(signature), outcome) {
        entry.outcome = outcome;
    }
}

pub fn tool_execution_failure_retryable(read_only: bool, error: &str) -> bool {
    if !read_only {
        return false;
    }
    let error = error.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "network",
        "connection",
        "temporar",
        "unavailable",
        "网络",
        "连接",
        "超时",
        "暂时",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

pub fn tool_execution_failure_output(error: &str, retryable: bool) -> Value {
    json!({
        "error": {
            "code": "tool_execution",
            "message": error,
            "retryable": retryable,
        },
        "code": "tool_execution",
        "retryable": retryable,
    })
}

pub fn normalize_tool_execution_result(
    session_id: &str,
    call_id: &str,
    tool_name: &str,
    input: &Value,
    read_only: bool,
    result: Result<crate::ToolCallResult, String>,
) -> crate::ToolCallResult {
    match result {
        Ok(mut result) => {
            for event in &mut result.events {
                if let AgentEvent::ToolCallFinished {
                    input: event_input, ..
                } = event
                {
                    *event_input = input.clone();
                }
            }
            result
        }
        Err(error) => {
            let retryable = tool_execution_failure_retryable(read_only, &error);
            let output = tool_execution_failure_output(&error, retryable);
            crate::ToolCallResult {
                images: Vec::new(),
                message: AgentMessage::tool(
                    call_id,
                    prune_tool_result_for_model(&output.to_string()),
                ),
                events: vec![AgentEvent::ToolCallFinished {
                    session_id: session_id.to_string(),
                    call_id: call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    input: input.clone(),
                    output,
                    is_error: true,
                }],
                permission_request: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_call_signature;

    #[test]
    fn read_edit_verify_allows_fresh_reads_without_replaying_the_edit() {
        for tool_name in [
            "read_local_file",
            "ssh__read_local_file",
            "wsl__read_local_file",
        ] {
            let input = json!({"path":"KeyGestureTracker.cpp", "offset":0, "limit":100});
            let read = tool_call_signature(tool_name, &input);
            let edit = tool_call_signature(
                "write_local_file",
                &json!({"path":"KeyGestureTracker.cpp", "content":"changed"}),
            );
            let mut ledger = HashMap::new();
            begin_tool_call(&mut ledger, read.clone());
            complete_tool_call(
                &mut ledger,
                &read,
                &[AgentEvent::ToolCallFinished {
                    session_id: "session".into(),
                    call_id: "read".into(),
                    tool_name: tool_name.into(),
                    input: input.clone(),
                    output: json!({"content":"old"}),
                    is_error: false,
                }],
            );
            assert!(!tool_call_can_run(&ledger, &read, true));
            let next_page = tool_call_signature(
                tool_name,
                &json!({"path":"KeyGestureTracker.cpp", "offset":100, "limit":100}),
            );
            assert!(tool_call_can_run(&ledger, &next_page, true));
            invalidate_successful_file_reads(&mut ledger);
            begin_tool_call(&mut ledger, edit.clone());
            complete_tool_call(
                &mut ledger,
                &edit,
                &[AgentEvent::ToolCallFinished {
                    session_id: "session".into(),
                    call_id: "edit".into(),
                    tool_name: "write_local_file".into(),
                    input: json!({}),
                    output: json!({"written":true}),
                    is_error: false,
                }],
            );
            assert!(tool_call_can_run(&ledger, &read, true));
            invalidate_successful_file_reads(&mut ledger);
            assert!(!tool_call_can_run(&ledger, &edit, false));
        }
    }

    #[test]
    fn redundant_call_does_not_end_a_round_with_new_work_but_a_loop_is_bounded() {
        let mut progress = ToolLoopProgress::default();
        assert!(!progress.observe_round(false, true));
        assert!(!progress.observe_round(true, true));
        assert!(!progress.observe_round(false, true));
        assert!(!progress.observe_round(false, true));
        assert!(progress.observe_round(false, true));
    }

    #[test]
    fn read_cache_invalidation_preserves_failed_call_retry_bounds() {
        let read = tool_call_signature("read_local_file", &json!({"path":"source.cpp"}));
        let mut ledger = HashMap::from([(
            read.clone(),
            ToolCallLedgerEntry {
                attempts: 2,
                outcome: ToolCallOutcome::Failed { retryable: true },
            },
        )]);
        invalidate_successful_file_reads(&mut ledger);
        assert!(!tool_call_can_run(&ledger, &read, true));
    }

    #[test]
    fn duplicate_shell_call_is_blocked_while_running_and_after_success() {
        let input = json!({"shell": "powershell", "command": "adb shell pm list packages"});
        let signature = tool_call_signature("run_local_shell", &input);
        let mut ledger = HashMap::new();
        assert!(tool_call_can_run(&ledger, &signature, false));
        begin_tool_call(&mut ledger, signature.clone());
        assert!(!tool_call_can_run(&ledger, &signature, false));
        complete_tool_call(
            &mut ledger,
            &signature,
            &[AgentEvent::ToolCallFinished {
                session_id: "session".to_string(),
                call_id: "call-1".to_string(),
                tool_name: "run_local_shell".to_string(),
                input,
                output: json!({"stdout": "package:example.app"}),
                is_error: false,
            }],
        );
        assert!(!tool_call_can_run(&ledger, &signature, false));
        assert!(!tool_call_can_run(&ledger, &signature, true));
    }

    #[test]
    fn failed_side_effecting_call_is_not_retried_and_read_only_retries_are_bounded() {
        let signature = "test-call".to_string();
        let mut ledger = HashMap::from([(
            signature.clone(),
            ToolCallLedgerEntry {
                attempts: 1,
                outcome: ToolCallOutcome::Failed { retryable: true },
            },
        )]);
        assert!(!tool_call_can_run(&ledger, &signature, false));
        assert!(tool_call_can_run(&ledger, &signature, true));
        begin_tool_call(&mut ledger, signature.clone());
        assert!(!tool_call_can_run(&ledger, &signature, true));
        ledger.get_mut(&signature).unwrap().outcome = ToolCallOutcome::Failed { retryable: true };
        assert!(!tool_call_can_run(&ledger, &signature, true));
    }

    #[test]
    fn failed_result_preserves_tool_identity_input_and_retry_policy() {
        for read_only in [false, true] {
            let input = json!({"path": "source.rs"});
            let result = normalize_tool_execution_result(
                "session",
                "call",
                "read_local_file",
                &input,
                read_only,
                Err("connection timed out".into()),
            );
            assert_eq!(result.message.tool_call_id.as_deref(), Some("call"));
            let [AgentEvent::ToolCallFinished {
                session_id,
                call_id,
                tool_name,
                input: event_input,
                output,
                is_error,
            }] = result.events.as_slice()
            else {
                panic!("a tool failure must produce one terminal event");
            };
            assert_eq!(
                (session_id.as_str(), call_id.as_str(), tool_name.as_str()),
                ("session", "call", "read_local_file")
            );
            assert_eq!(event_input, &input);
            assert!(*is_error);
            assert_eq!(output["retryable"], read_only);
            assert!(!tool_execution_failure_retryable(
                read_only,
                "permission denied"
            ));
        }
    }
}
