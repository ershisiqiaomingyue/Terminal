use super::super::models::QueuedThreadItem;
use super::ThreadSnapshot;

impl ThreadSnapshot {
    pub(crate) fn replace_queue(&mut self, queue: Vec<QueuedThreadItem>) -> bool {
        if self.queue == queue {
            return false;
        }
        self.queue = queue;
        self.revision += 1;
        true
    }
}
