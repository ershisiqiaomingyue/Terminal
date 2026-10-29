use serde_json::Value;
use std::path::Path;

use crate::ToolRegistry;

use super::canonical_tool_name;
use super::scope_keys::path_access_requires_confirmation_for_target;

pub fn is_read_only_command(input: &Value) -> bool {
    let Some(command) = input.get("command").and_then(Value::as_str) else {
        return false;
    };
    let name = Path::new(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    let args = input
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let first = args.first().map(String::as_str).unwrap_or_default();
    match name.as_str() {
        "where" => args.len() == 1 && !first.contains('\\') && !first.contains('/'),
        "rg" => !args
            .iter()
            .any(|arg| arg == "--pre" || arg.starts_with("--pre=")),
        "findstr" | "tree" => true,
        "git" => matches!(
            args.as_slice(),
            [command]
                if matches!(
                    command.as_str(),
                    "status" | "diff" | "log" | "show" | "branch" | "rev-parse" | "ls-files"
                )
        ),
        "adb" => {
            matches!(args.as_slice(), [command] if matches!(command.as_str(), "devices" | "get-state" | "version"))
        }
        "node"
        | "java"
        | "javac"
        | "python"
        | "python3"
        | "dotnet"
        | "cargo"
        | "gradle"
        | "cmake"
        | "ninja"
        | "go"
        | "rustc"
        | "trace_processor"
        | "trace_processor_shell" => {
            args.len() == 1 && matches!(first, "--version" | "-version" | "-v")
        }
        "android" => match args.as_slice() {
            [command] if matches!(command.as_str(), "--version" | "-V" | "help" | "info") => true,
            [command, ..] if matches!(command.as_str(), "docs" | "describe" | "layout") => true,
            [command, subcommand, ..] if command == "skills" && subcommand == "list" => true,
            [command, subcommand, ..] if command == "sdk" && subcommand == "list" => true,
            [command, subcommand, ..] if command == "emulator" && subcommand == "list" => true,
            _ => false,
        },
        "sdkmanager" => {
            matches!(args.as_slice(), [flag] if matches!(flag.as_str(), "--list" | "--version"))
        }
        "avdmanager" => args.first().map(String::as_str) == Some("list"),
        _ => false,
    }
}

pub fn is_read_only_tool(tool_name: &str, input: &Value) -> bool {
    let tool_name = canonical_tool_name(tool_name);
    match tool_name {
        "get_current_time"
        | "load_skill"
        | "get_working_directory"
        | "get_file_info"
        | "list_local_files"
        | "search_local_files"
        | "search_local_code"
        | "get_directory_tree"
        | "read_local_file"
        | "lsp_diagnostics"
        | "lsp_symbols"
        | "lsp_definition"
        | "lsp_references"
        | "get_git_status"
        | "get_git_diff"
        | "read_local_log_range"
        | "search_local_logs"
        | "capture_android_logcat"
        | "rebuild_log_timeline" => true,
        "run_local_command" => is_read_only_command(input),
        _ => false,
    }
}

pub fn is_dangerous_shell(input: &Value) -> bool {
    input
        .get("command")
        .and_then(Value::as_str)
        .is_none_or(|command| !command.trim().is_empty())
}

pub fn is_dangerous_command(input: &Value) -> bool {
    let Some(command) = input.get("command").and_then(Value::as_str) else {
        return false;
    };
    let name = Path::new(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    let args = input
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if matches!(
        name.as_str(),
        "node" | "python" | "python3" | "java" | "javac"
    ) && args
        .iter()
        .any(|arg| !matches!(arg.as_str(), "--version" | "-version" | "-v"))
    {
        return true;
    }
    match name.as_str() {
        "git" => args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "reset" | "clean" | "checkout" | "restore" | "push" | "rebase"
            )
        }),
        "npm" | "pnpm" | "yarn" => args
            .iter()
            .any(|arg| matches!(arg.as_str(), "publish" | "unpublish" | "prune")),
        "cargo" => args.iter().any(|arg| arg == "clean"),
        _ => false,
    }
}

pub fn is_parallel_safe_tool(
    tool_name: &str,
    input: &Value,
    permission_mode: &str,
    roots: &[String],
    registry: &ToolRegistry,
) -> bool {
    is_parallel_safe_tool_for_target(tool_name, input, permission_mode, roots, registry, false)
}

pub fn is_parallel_safe_tool_for_target(
    tool_name: &str,
    input: &Value,
    permission_mode: &str,
    roots: &[String],
    registry: &ToolRegistry,
    is_remote_target: bool,
) -> bool {
    let read_only_inspector = registry
        .get(tool_name)
        .map(|definition| definition.read_only || is_read_only_tool(tool_name, input))
        .unwrap_or(false);
    read_only_inspector
        && registry
            .get(tool_name)
            .map(|definition| !definition.requires_permission)
            .unwrap_or(false)
        && !path_access_requires_confirmation_for_target(
            permission_mode,
            tool_name,
            input,
            roots,
            true,
            is_remote_target,
        )
}
