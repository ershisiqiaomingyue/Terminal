use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::{resolve_environment_path, resolve_user_directory_alias};

use super::{canonical_tool_name, path_access_tool};
use crate::selected_working_directory;

pub fn command_path_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in command.chars() {
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            ';' | '|' | '&' | '>' | '<' if current.is_empty() => {}
            ';' | '|' | '&' | '>' | '<' => tokens.push(std::mem::take(&mut current)),
            character if character.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn collect_declared_path_values(value: &Value, candidates: &mut Vec<PathBuf>) {
    let Value::Object(object) = value else {
        if let Value::Array(values) = value {
            for value in values {
                collect_declared_path_values(value, candidates);
            }
        }
        return;
    };
    const PATH_KEYS: &[&str] = &[
        "path",
        "cwd",
        "source",
        "destination",
        "from",
        "to",
        "target",
        "file",
        "directory",
        "working_directory",
        "workingDirectory",
        "inputPath",
        "outputPath",
        "paths",
        "files",
    ];
    for (key, value) in object {
        if PATH_KEYS
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
        {
            match value {
                Value::String(path) if !path.trim().is_empty() => {
                    candidates.push(PathBuf::from(path.trim().trim_matches(['"', '\''])));
                }
                Value::Array(values) => {
                    for value in values {
                        if let Some(path) = value.as_str().filter(|path| !path.trim().is_empty()) {
                            candidates.push(PathBuf::from(path.trim().trim_matches(['"', '\''])));
                        }
                    }
                }
                _ => {}
            }
        }
        if value.is_object() || value.is_array() {
            collect_declared_path_values(value, candidates);
        }
    }
}

pub fn command_contains_path(command: &str) -> bool {
    let value = command.trim().trim_matches(['"', '\'']);
    if let Some((_, assigned_value)) = value.rsplit_once('=') {
        if command_contains_path(assigned_value) {
            return true;
        }
    }
    let bytes = value.as_bytes();
    let drive_path = bytes.len() >= 2 && bytes[1] == b':';
    let environment_path = value.starts_with('$')
        || (value.starts_with('%') && value[1..].contains('%'))
        || resolve_environment_path(value).is_some();
    value.contains('\\')
        || value.contains('/')
        || drive_path
        || environment_path
        || matches!(value, "." | "..")
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with("~\\")
        || value.starts_with("~/")
        || resolve_user_directory_alias(Path::new(value)).is_some()
}

fn path_access_candidates_impl(
    tool_name: &str,
    input: &Value,
    roots: &[String],
    include_local_fallback: bool,
) -> Vec<PathBuf> {
    let tool_name = canonical_tool_name(tool_name);
    let known_path_tool = path_access_tool(tool_name);
    let mut candidates = Vec::new();
    let mut add_value = |value: Option<&str>| {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            let value = value.trim().trim_matches(['"', '\'']);
            let value = value
                .rsplit_once('=')
                .map(|(_, path)| path)
                .unwrap_or(value)
                .trim_matches(['"', '\'']);
            candidates.push(PathBuf::from(value));
        }
    };
    match tool_name {
        "list_local_files" | "search_local_files" | "search_local_code" | "get_directory_tree"
        | "read_local_file" | "write_local_file" | "apply_patch" | "lsp_diagnostics"
        | "lsp_symbols" | "lsp_definition" | "lsp_references" | "get_git_diff"
        | "get_git_status" | "run_project_check" => {
            add_value(input.get("path").and_then(Value::as_str));
            add_value(input.get("cwd").and_then(Value::as_str));
        }
        "run_local_command" | "run_local_shell" => {
            add_value(input.get("cwd").and_then(Value::as_str));
            if let Some(command) = input.get("command").and_then(Value::as_str) {
                for token in command_path_tokens(command)
                    .iter()
                    .filter(|value| command_contains_path(value))
                {
                    add_value(Some(token));
                }
            }
            if let Some(args) = input.get("args").and_then(Value::as_array) {
                for value in args
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|value| command_contains_path(value))
                {
                    add_value(Some(value));
                }
            }
        }
        _ => {}
    }
    collect_declared_path_values(input, &mut candidates);
    if include_local_fallback && candidates.is_empty() && known_path_tool {
        if let Some(working_directory) = selected_working_directory(roots) {
            candidates.push(working_directory);
        }
    }
    candidates
}

pub fn path_access_candidates(tool_name: &str, input: &Value, roots: &[String]) -> Vec<PathBuf> {
    path_access_candidates_impl(tool_name, input, roots, true)
}

/// Collect path arguments without falling back to the desktop process' current
/// directory. SSH paths are POSIX paths and must never be inferred from the
/// Windows filesystem when the model omitted an optional path.
pub fn remote_path_access_candidates(
    tool_name: &str,
    input: &Value,
    roots: &[String],
) -> Vec<PathBuf> {
    path_access_candidates_impl(tool_name, input, roots, false)
}
