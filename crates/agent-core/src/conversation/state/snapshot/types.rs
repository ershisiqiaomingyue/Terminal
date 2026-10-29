use serde::{Deserialize, Serialize};

use super::super::models::{
    PermissionSnapshot, QueuedThreadItem, ThreadItem, ThreadRuntimeSnapshot, ThreadStatus,
    TurnSnapshot,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSnapshot {
    pub id: String,
    pub revision: u64,
    pub status: ThreadStatus,
    pub active_turn: Option<TurnSnapshot>,
    pub pending_permission: Option<PermissionSnapshot>,
    pub runtime: ThreadRuntimeSnapshot,
    pub items: Vec<ThreadItem>,
    pub queue: Vec<QueuedThreadItem>,
}
