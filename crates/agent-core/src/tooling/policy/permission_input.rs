use serde_json::{json, Value};

use super::command_safety::{is_dangerous_command, is_dangerous_shell, is_read_only_tool};
use super::names::canonical_tool_name;
use super::path_parsing::path_access_candidates;
use super::remote_paths::{
    normalize_remote_policy_path, remote_policy_candidates, remote_policy_path_is_selected,
    resolve_remote_policy_path,
};
use super::scope_keys::{
    path_access_requires_external_execution, permission_scope_key,
    permission_scope_key_for_environment,
};
use crate::{
    is_selected_path, normalized_policy_path_from, resolve_selected_path,
    selected_working_directory,
};

pub fn enrich_permission_input(
    tool_name: &str,
    input: &Value,
    mode: &str,
    roots: &[String],
) -> Value {
    let tool_name = canonical_tool_name(tool_name);
    let mut object = input.as_object().cloned().unwrap_or_default();
    let candidates = path_access_candidates(tool_name, input, roots);
    let target = candidates
        .iter()
        .find(|candidate| !is_selected_path(&normalized_policy_path_from(candidate, None), roots))
        .or_else(|| candidates.first())
        .map(|candidate| {
            resolve_selected_path(candidate, roots)
                .to_string_lossy()
                .to_string()
        });
    let working_directory = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            selected_working_directory(roots).map(|path| path.to_string_lossy().to_string())
        });
    let external = path_access_requires_external_execution(tool_name, input, roots);
    let dangerous = (tool_name == "run_local_shell" && is_dangerous_shell(input))
        || (tool_name == "run_local_command" && is_dangerous_command(input));
    object.insert(
        "permission".to_string(),
        json!({
            "target": target,
            "workingDirectory": working_directory,
            "risk": if dangerous { "dangerous_operation" } else if external { "external_path" } else if !is_read_only_tool(tool_name, input) { "write_or_command" } else { "read_only" },
            "scope": permission_scope_key(tool_name, input, roots),
            "mode": mode,
        }),
    );
    Value::Object(object)
}

pub fn enrich_permission_input_for_target(
    tool_name: &str,
    input: &Value,
    mode: &str,
    roots: &[String],
    is_remote_target: bool,
    namespace: &str,
) -> Value {
    let tool_name = canonical_tool_name(tool_name);
    if !is_remote_target {
        let mut enriched = enrich_permission_input(tool_name, input, mode, roots);
        if let Some(permission) = enriched
            .get_mut("permission")
            .and_then(Value::as_object_mut)
        {
            permission.insert(
                "scope".to_string(),
                Value::String(permission_scope_key_for_environment(
                    tool_name, input, roots, false, namespace,
                )),
            );
            permission.insert(
                "environment".to_string(),
                Value::String(namespace.to_string()),
            );
        }
        return enriched;
    }
    let mut object = input.as_object().cloned().unwrap_or_default();
    let candidates = remote_policy_candidates(tool_name, input, roots);
    let target = candidates
        .iter()
        .find(|candidate| !remote_policy_path_is_selected(candidate, roots))
        .or_else(|| candidates.first())
        .cloned();
    let working_directory = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(|value| resolve_remote_policy_path(value, roots))
        .or_else(|| roots.first().map(|root| normalize_remote_policy_path(root)));
    let external = candidates
        .iter()
        .any(|candidate| !remote_policy_path_is_selected(candidate, roots));
    let dangerous = (tool_name == "run_local_shell" && is_dangerous_shell(input))
        || (tool_name == "run_local_command" && is_dangerous_command(input));
    object.insert(
        "permission".to_string(),
        json!({
            "target": target,
            "workingDirectory": working_directory,
            "risk": if dangerous { "dangerous_operation" } else if external { "external_path" } else if !is_read_only_tool(tool_name, input) { "write_or_command" } else { "read_only" },
            "scope": permission_scope_key_for_environment(tool_name, input, roots, true, namespace),
            "environment": namespace,
            "mode": mode,
        }),
    );
    Value::Object(object)
}
