use crate::AgentChatRequest;

#[derive(Debug, Clone)]
pub(crate) struct QueuedAgentRequest {
    pub(super) id: String,
    pub(super) request: AgentChatRequest,
}

/// A queue item remains recoverable while a dispatcher or steer operation is
/// trying to hand it to an Agent turn. The caller must explicitly acknowledge
/// or reject the lease; dropping this value never destroys the queued request.
#[derive(Debug, Clone)]
pub struct QueuedAgentClaim {
    pub id: String,
    pub lease_id: String,
    pub request: AgentChatRequest,
    pub(super) original_index: usize,
}
