use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{
    is_selected_path, normalized_policy_path, normalized_policy_path_from, policy_path_key,
    resolve_selected_path, selected_working_directory,
};

use super::canonical_tool_name;
use super::command_safety::{is_dangerous_command, is_dangerous_shell, is_read_only_tool};
use super::path_parsing::path_access_candidates;
use super::remote_paths::{
    external_path_scope_key_for_target, remote_policy_candidates, remote_policy_path_is_selected,
};

pub fn namespaced_permission_scope_key(namespace: &str, scope: &str) -> String {
    let namespace = namespace.trim();
    if namespace.is_empty() {
        return scope.to_string();
    }
    format!("{namespace}::{scope}")
}

/// P7-B：`{namespace}::{scope}` 里 namespace 的环境种类（`ssh:<id>` → `ssh`）。
/// 没有 namespace 或 namespace 不带种类前缀（旧 key）时返回空串。
pub fn permission_scope_namespace_kind(key: &str) -> &str {
    let Some((namespace, _)) = key.split_once("::") else {
        return "";
    };
    match namespace.split_once(':') {
        Some((kind, _)) => kind,
        None => "",
    }
}

/// P7-B：去掉同一环境种类下具体目标（连接/profile id）后的一层 key。
/// `ssh:connection-a::write_local_file:/remote/x` →
/// `ssh::write_local_file:/remote/x`。
///
/// 存在的理由：换一条到同一台机器的连接（或换了 WSL profile 名）后
/// `ssh:<id>` 变了，用户上一轮点过的「本次会话允许」就永远不命中，而在
/// 用户感知里那是**同一个远程环境**。命中顺序是精确 key → 环境种类 key，
/// 先按精确目标放行、没命中才退一层：既不跨环境（ssh ↔ wsl ↔ 本地）
/// 串权，也不必让用户重新点一遍。
pub fn permission_scope_kind_key(namespace: &str, scope: &str) -> Option<String> {
    let kind = match namespace.trim().split_once(':') {
        Some((kind, _)) if !kind.is_empty() => kind,
        _ => return None,
    };
    Some(format!("{kind}::{scope}"))
}

pub fn external_path_scope_key_for_environment(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
    namespace: &str,
) -> Option<String> {
    external_path_scope_key_for_target(tool_name, input, roots, is_remote_target)
        .map(|scope| namespaced_permission_scope_key(namespace, &scope))
}

pub fn path_access_requires_external_execution_for_target(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
) -> bool {
    if !is_remote_target {
        return path_access_requires_external_execution(tool_name, input, roots);
    }
    remote_policy_candidates(tool_name, input, roots)
        .iter()
        .any(|candidate| !remote_policy_path_is_selected(candidate, roots))
}

pub fn permission_scope_key_for_target(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
) -> String {
    let tool_name = canonical_tool_name(tool_name);
    if !is_remote_target {
        return permission_scope_key(tool_name, input, roots);
    }
    let candidates = remote_policy_candidates(tool_name, input, roots);
    let mut locations = candidates.clone();
    locations.sort();
    locations.dedup();
    let locations = locations.join("|");
    match tool_name {
        "run_local_shell" => format!(
            "remote-command:{tool_name}:{}:{locations}",
            input
                .get("shell")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
        ),
        "run_local_command" => format!(
            "remote-command:{tool_name}:{}:{locations}",
            input
                .get("command")
                .and_then(Value::as_str)
                .map(|value| {
                    Path::new(value)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or(value)
                        .to_ascii_lowercase()
                })
                .unwrap_or_default()
        ),
        _ if !candidates.is_empty() => {
            format!("remote-path:{tool_name}:{}", candidates[0])
        }
        _ => format!("remote-tool:{tool_name}"),
    }
}

pub fn permission_scope_key_for_environment(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    is_remote_target: bool,
    namespace: &str,
) -> String {
    namespaced_permission_scope_key(
        namespace,
        &permission_scope_key_for_target(tool_name, input, roots, is_remote_target),
    )
}

fn normalized_candidate(path: &Path, roots: &[String]) -> PathBuf {
    normalized_policy_path_from(&resolve_selected_path(path, roots), None)
}

fn external_scope_path(path: &Path, roots: &[String]) -> PathBuf {
    let mut path = normalized_candidate(path, roots);
    loop {
        if let Ok(metadata) = fs::metadata(&path) {
            if metadata.is_file() {
                return path.parent().map(normalized_policy_path).unwrap_or(path);
            }
            return path;
        }
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return path;
        };
        if parent == path {
            return path;
        }
        path = parent;
    }
}

pub fn external_path_scope_key(tool_name: &str, input: &Value, roots: &[String]) -> Option<String> {
    path_access_candidates(tool_name, input, roots)
        .into_iter()
        .find(|candidate| !is_selected_path(&normalized_candidate(candidate, roots), roots))
        .map(|candidate| {
            format!(
                "external-path:{}",
                policy_path_key(&external_scope_path(&candidate, roots))
            )
        })
}

pub fn path_access_requires_confirmation(
    mode: &str,
    tool_name: &str,
    input: &Value,
    roots: &[String],
) -> bool {
    path_access_requires_confirmation_with_read_only(
        mode,
        tool_name,
        input,
        roots,
        is_read_only_tool(tool_name, input),
    )
}

pub fn path_access_requires_confirmation_with_read_only(
    mode: &str,
    tool_name: &str,
    input: &Value,
    roots: &[String],
    read_only: bool,
) -> bool {
    if mode == "full" {
        return false;
    }
    let candidates = path_access_candidates(tool_name, input, roots);
    if candidates.is_empty() {
        return false;
    }
    let external = candidates
        .iter()
        .any(|candidate| !is_selected_path(&normalized_candidate(candidate, roots), roots));
    mode == "ask" || external || !read_only
}

pub fn path_access_requires_external_execution(
    tool_name: &str,
    input: &Value,
    roots: &[String],
) -> bool {
    path_access_candidates(tool_name, input, roots)
        .iter()
        .any(|candidate| !is_selected_path(&normalized_candidate(candidate, roots), roots))
}

pub fn path_access_requires_confirmation_for_target(
    mode: &str,
    tool_name: &str,
    input: &Value,
    roots: &[String],
    read_only: bool,
    is_remote_target: bool,
) -> bool {
    if !is_remote_target {
        return path_access_requires_confirmation_with_read_only(
            mode, tool_name, input, roots, read_only,
        );
    }
    if mode == "full" {
        return false;
    }
    let candidates = remote_policy_candidates(tool_name, input, roots);
    let external = candidates
        .iter()
        .any(|candidate| !remote_policy_path_is_selected(candidate, roots));
    mode == "ask" || external || !read_only
}

fn permission_scope_path(tool_name: &str, input: &Value, roots: &[String]) -> String {
    let candidate = path_access_candidates(tool_name, input, roots)
        .into_iter()
        .find(|candidate| !is_selected_path(&normalized_candidate(candidate, roots), roots))
        .or_else(|| {
            path_access_candidates(tool_name, input, roots)
                .into_iter()
                .next()
        })
        .map(|path| resolve_selected_path(&path, roots))
        .or_else(|| selected_working_directory(roots));
    let Some(path) = candidate else {
        return String::new();
    };
    let path = normalized_policy_path(&path);
    if fs::metadata(&path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
    {
        path.parent()
            .map(policy_path_key)
            .unwrap_or_else(|| policy_path_key(&path))
    } else {
        policy_path_key(&path)
    }
}

fn permission_scope_paths(tool_name: &str, input: &Value, roots: &[String]) -> String {
    let mut locations = path_access_candidates(tool_name, input, roots)
        .into_iter()
        .map(|candidate| {
            let path = normalized_candidate(&candidate, roots);
            if fs::metadata(&path)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                path.parent()
                    .map(policy_path_key)
                    .unwrap_or_else(|| policy_path_key(&path))
            } else {
                policy_path_key(&path)
            }
        })
        .collect::<Vec<_>>();
    locations.sort();
    locations.dedup();
    locations.join("|")
}

pub fn permission_scope_key(tool_name: &str, input: &Value, roots: &[String]) -> String {
    let tool_name = canonical_tool_name(tool_name);
    match tool_name {
        "__agent_path_access_probe" => external_path_scope_key(tool_name, input, roots)
            .unwrap_or_else(|| {
                format!(
                    "path:{tool_name}:{}",
                    permission_scope_path(tool_name, input, roots)
                )
            }),
        "run_local_shell" => {
            if is_dangerous_shell(input) {
                format!(
                    "exact:{tool_name}:{}",
                    serde_json::to_string(input).unwrap_or_default()
                )
            } else {
                format!(
                    "command:{tool_name}:{}:{}",
                    input
                        .get("shell")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    permission_scope_paths(tool_name, input, roots),
                )
            }
        }
        "run_local_command" => {
            let command = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if is_dangerous_command(input) {
                format!(
                    "exact:{tool_name}:{}",
                    serde_json::to_string(input).unwrap_or_default()
                )
            } else {
                format!(
                    "command:{tool_name}:{}:{}",
                    Path::new(command)
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or(command)
                        .to_ascii_lowercase(),
                    permission_scope_paths(tool_name, input, roots),
                )
            }
        }
        _ if !path_access_candidates(tool_name, input, roots).is_empty() => {
            format!(
                "path:{tool_name}:{}",
                permission_scope_path(tool_name, input, roots)
            )
        }
        _ => format!("tool:{tool_name}"),
    }
}
