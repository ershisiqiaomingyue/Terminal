mod turn_entry;
mod body;

use super::*;

mod finalization;
mod budget;
mod strategy;
mod setup;

pub(super) async fn run_agent_turn<'a>(
    app: AppHandle,
    state: State<'a, AgentRuntimeState>,
    mcp_state: State<'a, McpRuntimeState>,
    request: AgentChatRequest,
    session_id: String,
    turn_id: String,
    request_guard: AgentRequestGuard<'a>,
) -> Result<AgentChatResponse, String> {
    turn_entry::prepare_and_run(
        app,
        state,
        mcp_state,
        request,
        session_id,
        turn_id,
        request_guard,
    )
    .await
}
