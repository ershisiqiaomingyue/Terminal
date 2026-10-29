use super::events::ThreadEvent;
use super::models::{ThreadRuntimeSnapshot, ThreadStatus, ThreadUsageSnapshot};

mod apply;
mod queue;
mod tool;
mod types;

#[cfg(test)]
pub(crate) use tool::MAX_SNAPSHOT_TOOL_OUTPUT_BYTES;
pub use types::ThreadSnapshot;

impl ThreadSnapshot {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            revision: 0,
            status: ThreadStatus::Idle,
            active_turn: None,
            pending_permission: None,
            runtime: ThreadRuntimeSnapshot {
                activity: String::new(),
                tool_call_count: 0,
                usage: ThreadUsageSnapshot::default(),
                execution_target: None,
            },
            items: Vec::new(),
            queue: Vec::new(),
        }
    }

    pub fn apply(&mut self, event: ThreadEvent) -> bool {
        self.reduce(event)
    }
}
