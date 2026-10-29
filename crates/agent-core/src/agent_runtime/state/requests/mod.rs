mod control;
mod guard;
mod permissions;

pub(crate) use control::RequestState;
pub use control::{cancel_request_token_if_matching, AgentRequestControl, UnitKind};
pub use guard::AgentRequestGuard;
pub use permissions::{
    is_cancelled, register_pending_permission, request_cancellation, wait_for_agent_cancellation,
    wait_for_permission, PendingPermission, PermissionAction, PermissionResponse,
    PermissionWaitResult,
};

#[cfg(test)]
mod tests;
