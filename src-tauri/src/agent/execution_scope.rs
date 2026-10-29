use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentToolEnvironment {
    Windows,
    Ssh,
    Wsl,
}

impl AgentToolEnvironment {
    pub(super) fn prefix(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Ssh => "ssh",
            Self::Wsl => "wsl",
        }
    }

    pub(super) fn is_remote(self) -> bool {
        !matches!(self, Self::Windows)
    }
}

#[derive(Clone, Copy)]
pub(super) struct ToolExecutionScope<'a> {
    pub(super) environment: AgentToolEnvironment,
    pub(super) paths: &'a [String],
}

impl ToolExecutionScope<'_> {
    pub(super) fn is_remote(self) -> bool {
        self.environment.is_remote()
    }

    pub(super) fn permission_namespace<'a>(self, ssh: &'a str, wsl: &'a str) -> &'a str {
        match self.environment {
            AgentToolEnvironment::Windows => "windows",
            AgentToolEnvironment::Ssh => ssh,
            AgentToolEnvironment::Wsl => wsl,
        }
    }
}

pub(super) fn default_tool_environment(
    is_ssh_target: bool,
    is_wsl_target: bool,
) -> AgentToolEnvironment {
    if is_ssh_target {
        AgentToolEnvironment::Ssh
    } else if is_wsl_target {
        AgentToolEnvironment::Wsl
    } else {
        AgentToolEnvironment::Windows
    }
}

pub(super) fn tool_execution_scope<'a>(
    tool_name: &str,
    default_environment: AgentToolEnvironment,
    windows_paths: &'a [String],
    ssh_paths: &'a [String],
    wsl_paths: &'a [String],
) -> ToolExecutionScope<'a> {
    let environment = match agent_core::scoped_tool_name(tool_name).0 {
        Some("windows") => AgentToolEnvironment::Windows,
        Some("ssh") => AgentToolEnvironment::Ssh,
        Some("wsl") => AgentToolEnvironment::Wsl,
        _ => default_environment,
    };
    let paths = match environment {
        AgentToolEnvironment::Windows => windows_paths,
        AgentToolEnvironment::Ssh => ssh_paths,
        AgentToolEnvironment::Wsl => wsl_paths,
    };
    ToolExecutionScope { environment, paths }
}

pub(super) fn consume_one_time_path_approval(
    state: &AgentRuntimeState,
    session_id: &str,
    tool_name: &str,
    input: &Value,
    paths: &[String],
    is_remote_target: bool,
    scope_namespace: &str,
) {
    let exact = agent_core::permission_scope_key_for_environment(
        tool_name,
        input,
        paths,
        is_remote_target,
        scope_namespace,
    );
    let external = agent_core::external_path_scope_key_for_environment(
        tool_name,
        input,
        paths,
        is_remote_target,
        scope_namespace,
    );
    if let Ok(mut approvals) = state.one_time_approvals.lock() {
        approvals.retain(|(session, scope)| {
            session != session_id
                || (scope != &exact && external.as_deref() != Some(scope.as_str()))
        });
    }
}
