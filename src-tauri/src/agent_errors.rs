use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl AgentCommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn provider(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let retryable = matches!(
            code.as_str(),
            "network" | "timeout" | "rate_limited" | "server_failure" | "context_compaction"
        );
        Self::new(code, message, retryable)
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        let message = message.into();
        let normalized = message.to_ascii_lowercase();
        let explicit_code = message
            .strip_prefix('[')
            .and_then(|value| value.split_once(']'))
            .map(|(code, _)| code.trim())
            .filter(|code| !code.is_empty());
        let code = if let Some(code) = explicit_code {
            code.to_string()
        } else if normalized.contains("请求已取消") {
            "cancelled".to_string()
        } else if normalized.contains("权限") {
            "permission".to_string()
        } else if normalized.contains("context")
            || normalized.contains("token")
            || message.contains("上下文")
        {
            "context_compaction".to_string()
        } else if normalized.contains("命令") || normalized.contains("工具") {
            "tool_execution".to_string()
        } else {
            "agent_runtime".to_string()
        };
        let retryable = matches!(
            code.as_str(),
            "cancelled"
                | "context_compaction"
                | "network"
                | "timeout"
                | "turn_idle_timeout"
                | "rate_limited"
                | "server_failure"
        );
        Self::new(code, message, retryable)
    }
}
