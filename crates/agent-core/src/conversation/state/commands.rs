use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Shared client-to-runtime vocabulary. Tauri, a CLI JSONL transport, and an
/// IDE bridge can serialize the same commands without owning session state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ConversationCommand {
    ThreadRead {
        thread_id: String,
        #[serde(default)]
        after_revision: Option<u64>,
    },
    TurnStart {
        thread_id: String,
        turn_id: String,
        request: Value,
    },
    TurnEnqueue {
        thread_id: String,
        request: Value,
    },
    TurnQueueGuide {
        thread_id: String,
        queue_id: String,
    },
    TurnQueueUpdate {
        thread_id: String,
        queue_id: String,
        content: String,
    },
    TurnQueueRemove {
        thread_id: String,
        queue_id: String,
    },
    TurnQueueReorder {
        thread_id: String,
        queue_ids: Vec<String>,
    },
    TurnResume {
        thread_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
    },
    TurnSteer {
        thread_id: String,
        turn_id: String,
        request: Value,
    },
    TurnInterrupt {
        thread_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    PermissionResolve {
        thread_id: String,
        request_id: String,
        action: String,
        instruction: Option<String>,
    },
}
