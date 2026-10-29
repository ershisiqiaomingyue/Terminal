mod projection;

use crate::{ThreadEvent, ThreadSnapshot};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct ThreadUpdateJournal {
    pub(super) events: VecDeque<(u64, usize, ThreadEvent)>,
    pub(super) bytes: usize,
    pub(super) published_revision: Option<u64>,
}

pub(super) trait ThreadProjectionState {
    fn threads_slot(&self) -> &Mutex<HashMap<String, ThreadSnapshot>>;
    fn updates_slot(&self) -> &Mutex<HashMap<String, ThreadUpdateJournal>>;
}

#[cfg(test)]
mod tests;
