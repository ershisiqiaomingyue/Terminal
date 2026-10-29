use serde_json::Value;

fn project_lifecycle(payload: &Value) -> Option<Value> {
    let kind = payload["type"].as_str().unwrap_or_default();
    if !matches!(
        kind,
        "started"
            | "completed"
            | "cancelled"
            | "error"
            | "tool_call_started"
            | "tool_call_finished"
            | "permission_requested"
            | "permission_decision"
    ) {
        return None;
    }
    let mut data = serde_json::Map::new();
    for key in [
        "session_id",
        "turn_id",
        "call_id",
        "tool_name",
        "status",
        "is_error",
        "started_at_ms",
        "finished_at_ms",
        "termination_reason",
        "terminal_persistence_failed",
        "durable_terminal",
        "code",
        "retryable",
    ] {
        if let Some(value) = payload.get(key) {
            if value.is_string() || value.is_number() || value.is_boolean() {
                data.insert(key.into(), value.clone());
            }
        }
    }
    // Attribute failures to the actual handler family rather than assuming all
    // filesystem/browser work used the native implementation or a third-party plugin.
    if let Some(name) = payload["tool_name"].as_str() {
        let (environment, name) = if let Some(name) = name.strip_prefix("windows__") {
            ("windows", name)
        } else if let Some(name) = name.strip_prefix("ssh__") {
            ("ssh", name)
        } else if let Some(name) = name.strip_prefix("wsl__") {
            ("wsl", name)
        } else {
            ("turn_default", name)
        };
        let origin = if name.starts_with("plugin__") {
            "plugin"
        } else if name.starts_with("mcp__") {
            "mcp"
        } else {
            "native"
        };
        data.insert("tool_origin".into(), origin.into());
        data.insert("tool_environment".into(), environment.into());
    }
    if kind == "tool_call_finished" {
        // A duplicate call may be skipped without is_error=true. Keep that
        // distinction without copying tool arguments, output, or error text.
        for key in ["skipped", "interrupted"] {
            if let Some(value) = payload["output"][key].as_bool() {
                data.insert(key.into(), Value::Bool(value));
            }
        }
    }
    Some(Value::Object(data))
}

pub fn record_lifecycle(payload: &Value) {
    if let Some(data) = project_lifecycle(payload) {
        let kind = payload["type"].as_str().unwrap_or_default();
        let failed = kind == "error" || payload["is_error"] == true;
        super::record(
            if failed { "error" } else { "info" },
            &format!("agent.{kind}"),
            "native.agent",
            data,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_filter_ignores_high_frequency_output() {
        // Streaming text and command output must never be written to diagnostics.
        assert!(
            project_lifecycle(&serde_json::json!({"type":"text_delta","text":"private"})).is_none()
        );
        assert!(project_lifecycle(
            &serde_json::json!({"type":"command_output","output":"private"})
        )
        .is_none());
        let data = project_lifecycle(&serde_json::json!({"type":"tool_call_finished","session_id":"thread-a","turn_id":"turn-b","tool_name":"run_local_shell","is_error":true,"output":"private","input":{"password":"private"}})).unwrap();
        assert_eq!(data["session_id"], "thread-a");
        assert_eq!(data["turn_id"], "turn-b");
        assert_eq!(data["is_error"], true);
        assert!(!data.to_string().contains("private"));
        let terminal = project_lifecycle(&serde_json::json!({"type":"completed", "termination_reason":"output_limit_reached", "durable_terminal":true, "text":"private reply"})).unwrap();
        assert_eq!(terminal["termination_reason"], "output_limit_reached");
        assert_eq!(terminal["durable_terminal"], true);
        assert!(!terminal.to_string().contains("private"));
        let skipped = project_lifecycle(&serde_json::json!({"type":"tool_call_finished", "is_error":false, "output":{"skipped":true,"reason":"private output"}})).unwrap();
        assert_eq!(skipped["skipped"], true);
        assert!(!skipped.to_string().contains("private"));
        for (name, origin, environment) in [
            ("windows__plugin__abc__search", "plugin", "windows"),
            ("mcp__browser__snapshot", "mcp", "turn_default"),
            ("ssh__search_local_code", "native", "ssh"),
            ("wsl__read_local_file", "native", "wsl"),
        ] {
            let projected = project_lifecycle(
                &serde_json::json!({"type":"tool_call_finished","tool_name":name}),
            )
            .unwrap();
            assert_eq!(projected["tool_origin"], origin);
            assert_eq!(projected["tool_environment"], environment);
        }
    }
}
