use super::*;

mod request;
pub(in crate::agent::turn) use request::prepare_input;
mod environment;
pub(in crate::agent::turn) use environment::prepare_turn_environments;
mod preparation;
pub(in crate::agent::turn) use preparation::{prepare_turn_input, PreparedInput};

/// Request normalization and target classification belong to the turn setup
/// boundary. The execution environment itself is prepared separately.
pub(in crate::agent::turn) struct TurnRequest {
    pub request: AgentChatRequest,
    pub is_ssh_target: bool,
    pub is_wsl_target: bool,
    pub default_environment: AgentToolEnvironment,
}

pub(in crate::agent::turn) struct PreparedTurn<'a> {
    pub app: &'a AppHandle,
    pub request: &'a AgentChatRequest,
    pub ssh_context: Option<&'a agent_core::SshToolContext>,
    pub bound_ssh_hosts: &'a [BoundSshHost],
    pub wsl_root: Option<&'a str>,
    pub wsl_context: Option<&'a LocalToolContext>,
    pub default_environment: AgentToolEnvironment,
    pub mcp_state: &'a McpRuntimeState,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub cancellation_reason: &'a Arc<Mutex<Option<String>>>,
    pub cancellation_token: &'a tokio_util::sync::CancellationToken,
    pub is_ssh_target: bool,
    pub is_wsl_target: bool,
}
