mod claim;
mod manager;
mod preview;
mod projection;

pub use claim::QueuedAgentClaim;
pub(crate) use claim::QueuedAgentRequest;

pub(super) trait QueuedThreadSnapshot {
    fn threads_slot(
        &self,
    ) -> &std::sync::Mutex<std::collections::HashMap<String, crate::ThreadSnapshot>>;
}

#[cfg(test)]
mod tests;
