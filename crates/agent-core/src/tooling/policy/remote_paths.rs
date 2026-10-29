use serde_json::{json, Value};

use super::external_path_scope_key;
use super::{canonical_tool_name, remote_path_access_candidates};

pub(super) fn resolve_remote_policy_path(raw: &str, roots: &[String]) -> String {
    let mut value = raw.trim().replace('\\', "/");
    let root = roots
        .first()
        .map(|root| normalize_remote_policy_path(root))
        .unwrap_or_else(|| "/".to_string());
    if value == "~" {
        value = root.clone();
    } else if let Some(remainder) = value.strip_prefix("~/") {
        value = format!("{root}/{remainder}");
    } else if value == "$HOME" || value == "${HOME}" {
        value = root.clone();
    } else if let Some(remainder) = value.strip_prefix("$HOME/") {
        value = format!("{root}/{remainder}");
    } else if let Some(remainder) = value.strip_prefix("${HOME}/") {
        value = format!("{root}/{remainder}");
    } else if !value.starts_with('/') {
        value = format!("{root}/{value}");
    }
    normalize_remote_policy_path(&value)
}

pub(super) fn normalize_remote_policy_path(path: &str) -> String {
    let mut parts = Vec::new();
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = parts.pop();
        } else {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

pub(super) fn remote_policy_path_is_selected(path: &str, roots: &[String]) -> bool {
    let path = normalize_remote_policy_path(path);
    roots.iter().any(|root| {
        let root = normalize_remote_policy_path(root);
        root == "/" || path == root || path.starts_with(&format!("{root}/"))
    })
}

pub(super) fn remote_policy_candidates(
    tool_name: &str,
    input: &Value,
    roots: &[String],
) -> Vec<String> {
    let tool_name = canonical_tool_name(tool_name);
    let policy_input = if matches!(
        tool_name,
        "upload_file"
            | "download_file"
            | "upload_directory"
            | "download_directory"
            | "upload_paths"
            | "download_path"
    ) {
        let remote_path = input.get("remote_path").cloned().unwrap_or(Value::Null);
        json!({ "path": remote_path })
    } else {
        input.clone()
    };
    remote_path_access_candidates(tool_name, &policy_input, roots)
        .into_iter()
        .map(|path| resolve_remote_policy_path(&path.to_string_lossy(), roots))
        .collect()
}

pub fn external_path_scope_key_for_target(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
) -> Option<String> {
    if !is_remote_target {
        return external_path_scope_key(tool_name, input, roots);
    }
    remote_policy_candidates(tool_name, input, roots)
        .into_iter()
        .find(|candidate| !remote_policy_path_is_selected(candidate, roots))
        .map(|candidate| format!("remote-external-path:{candidate}"))
}
