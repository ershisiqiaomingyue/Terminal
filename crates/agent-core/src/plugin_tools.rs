//! Protocol-safe plugin tool names and lossless result projection.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// Keep enough room for the application's Windows environment prefix (9 bytes).
pub fn plugin_tool_name(plugin: &str, name: &str) -> String {
    let suffix = if name.len() <= 29
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        name.to_string()
    } else {
        format!("tool_{:x}", Sha256::digest(name.as_bytes()))[..29].into()
    };
    format!("plugin__{plugin}__{suffix}")
}

pub fn plugin_tool_output(result: Value) -> Value {
    let mut value = result
        .get("value")
        .cloned()
        .unwrap_or_else(|| result.clone());
    let failed = result["isError"] == true
        || value.get("error").is_some_and(|error| !error.is_null())
        || matches!(
            value["status"].as_str(),
            Some("error" | "rejected" | "failed")
        )
        || value
            .get("exitCode")
            .or_else(|| value.get("exit_code"))
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0);
    // A structured value may accompany rich MCP/plugin content. Keep binary
    // blocks for the runtime image projector without duplicating text results.
    if result.get("value").is_some() {
        let rich = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block["type"] != "text")
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !rich.is_empty() {
            value = json!({"structuredContent":value,"content":rich});
        }
    }
    if failed {
        if let Some(object) = value.as_object_mut() {
            object.insert("success".into(), Value::Bool(false));
        } else {
            return json!({"success":false,"value":value});
        }
    }
    value
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_tool_names_remain_unique_and_fit_all_protocols_with_environment_aliases() {
        let id = "1234567890abcdef";
        let a = plugin_tool_name(id, "a tool with a long name and arbitrary unicode 字符");
        let b = plugin_tool_name(id, "a tool with a long name and arbitrary unicode 文字");
        assert_ne!(a, b);
        for name in [a, b, plugin_tool_name(id, "ops_inspect_local_file")] {
            assert!(format!("windows__{name}").len() <= 64);
            assert!(name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')));
        }
    }
    #[test]
    fn structured_tool_results_are_not_duplicated_and_failures_reach_the_chat_status() {
        assert_eq!(
            plugin_tool_output(
                json!({"isError":false,"value":{"items":[1]},"content":[{"type":"text","text":"duplicate"}]})
            ),
            json!({"items":[1]})
        );
        for result in [
            json!({"isError":true,"content":[{"type":"text","text":"failed"}]}),
            json!({"value":{"status":"rejected","error":{"code":"POLICY_DENIED"}}}),
            json!({"value":{"exitCode":2}}),
        ] {
            assert_eq!(plugin_tool_output(result)["success"], false);
        }
        let rich_failure = plugin_tool_output(json!({"value":{"exitCode":2},"content":[
            {"type":"text","text":"duplicate"},
            {"type":"image","mimeType":"image/png","data":"YWJj"}
        ]}));
        assert_eq!(rich_failure["success"], false);
        assert_eq!(rich_failure["structuredContent"]["exitCode"], 2);
        assert_eq!(rich_failure["content"].as_array().unwrap().len(), 1);
        assert_eq!(rich_failure["content"][0]["type"], "image");
    }
}
