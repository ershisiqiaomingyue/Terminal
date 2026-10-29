use serde::{Deserialize, Serialize};

use super::events::ThreadEvent;
use super::snapshot::ThreadSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadUpdate {
    Snapshot {
        snapshot: ThreadSnapshot,
    },
    Delta {
        id: String,
        base_revision: u64,
        revision: u64,
        metadata: ThreadSnapshot,
        events: Vec<ThreadEvent>,
    },
}
