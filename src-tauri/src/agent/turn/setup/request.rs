use super::super::*;
use super::TurnRequest;

pub(in crate::agent::turn) fn prepare_input(
    mut request: AgentChatRequest,
) -> Result<TurnRequest, String> {
    agent_core::normalize_local_project_target(&mut request, None);
    let is_ssh_target = request
        .execution_target
        .as_ref()
        .is_some_and(|target| target.kind.eq_ignore_ascii_case("ssh"));
    let is_wsl_target = request
        .execution_target
        .as_ref()
        .is_some_and(|target| target.kind.eq_ignore_ascii_case("wsl"));
    bind_workspace_artifact(&mut request)?;
    Ok(TurnRequest {
        request,
        is_ssh_target,
        is_wsl_target,
        default_environment: super::default_tool_environment(is_ssh_target, is_wsl_target),
    })
}

