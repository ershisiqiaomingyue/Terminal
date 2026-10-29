use super::QueuedThreadSnapshot;
use crate::{AgentRuntimeState, QueuedThreadItem, ThreadSnapshot};

impl AgentRuntimeState {
    /// Read-only inbox projection; does not populate the legacy scheduler.
    pub fn project_external_queue(
        &self,
        session_id: &str,
        queue: Vec<QueuedThreadItem>,
    ) -> ThreadSnapshot {
        self.replace_thread_queue(session_id, queue)
    }

    pub(crate) fn replace_thread_queue(
        &self,
        session_id: &str,
        queue: Vec<QueuedThreadItem>,
    ) -> ThreadSnapshot {
        let mut threads = self
            .threads_slot()
            .lock()
            .expect("thread snapshot lock should be available");
        let snapshot = threads
            .entry(session_id.to_string())
            .or_insert_with(|| ThreadSnapshot::new(session_id));
        snapshot.replace_queue(queue);
        snapshot.clone()
    }
}
