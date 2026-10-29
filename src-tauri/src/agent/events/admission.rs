#[derive(Debug, Clone)]
pub(crate) struct AgentQueueAdmission {
    pub(crate) queue_id: String,
    pub(crate) lease_id: String,
    pub(crate) lease_owner: String,
}
