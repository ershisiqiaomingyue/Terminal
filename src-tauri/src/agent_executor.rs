use agent_core::CommandOutputSink as CoreCommandOutputSink;
use serde_json::Value;
use std::sync::Arc;
use tauri::AppHandle;

#[derive(Clone)]
pub struct CommandOutputSink {
    inner: CoreCommandOutputSink,
}

impl CommandOutputSink {
    pub fn for_tool(
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        tool_name: &'static str,
    ) -> Self {
        let app = app.clone();
        let turn_id = turn_id.to_string();
        let listener_turn_id = turn_id.clone();
        let listener = Arc::new(move |payload: Value| {
            crate::agent::emit_agent_event_payload_for_turn(&app, payload, Some(&listener_turn_id));
        });
        Self {
            inner: CoreCommandOutputSink::new(listener, session_id, turn_id, tool_name),
        }
    }

    pub fn inner(&self) -> &CoreCommandOutputSink {
        &self.inner
    }
}
