use serde_json::Value;
use std::path::Path;

use super::path_parsing::command_path_tokens;

pub const FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE: &str =
    "出于安全策略，禁止执行可能全局删除文件的命令。请改用受控文件工具，并明确指定需要操作的具体路径。";

fn is_recursive_delete_flag(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value == "--recursive"
        || value == "-r"
        || value == "-R"
        || (value.starts_with('-') && !value.starts_with("--") && value.contains('r'))
        || value == "/s"
        || value == "-recurse"
        || value == "-recursive"
}

fn is_broad_delete_target(value: &str) -> bool {
    let value = value
        .trim()
        .trim_matches(['"', '\''])
        .replace('\\', "/")
        .to_ascii_lowercase();
    value == "*"
        || value == "?"
        || value == "."
        || value == ".."
        || value == "/"
        || value == "/*"
        || value == "~"
        || value == "~/*"
        || value == "$home"
        || value == "${home}"
        || value == "%userprofile%"
        || value == "%userprofile%/*"
        || value.ends_with("/*")
        || value.ends_with("/?")
        || (value.ends_with(":/") && value.len() == 3)
}

fn has_recursive_delete(tokens: &[String], start: usize) -> bool {
    let mut recursive = false;
    let mut after_separator = false;
    let mut targets = Vec::new();
    for token in tokens.iter().skip(start + 1) {
        if token == "--" {
            after_separator = true;
            continue;
        }
        if !after_separator && (token.starts_with('-') || matches!(token.as_str(), "/s" | "/q")) {
            recursive |= is_recursive_delete_flag(token);
            continue;
        }
        targets.push(token.as_str());
    }
    recursive && targets.iter().any(|target| is_broad_delete_target(target))
}

/// Hard-block commands that can erase an entire working tree or filesystem
/// scope. Permission prompts are not enough because full-access mode bypasses
/// prompts; targeted file operations remain available through controlled tools.
pub fn is_forbidden_destructive_command_line(command: &str) -> bool {
    let tokens = command_path_tokens(command)
        .into_iter()
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return false;
    }
    for (index, token) in tokens.iter().enumerate() {
        if matches!(
            token.as_str(),
            "rm" | "remove-item" | "del" | "erase" | "rmdir" | "rd"
        ) && has_recursive_delete(&tokens, index)
        {
            return true;
        }
        if token == "find"
            && tokens
                .iter()
                .skip(index + 1)
                .any(|value| value == "-delete")
        {
            return true;
        }
        if token == "find"
            && tokens.iter().skip(index + 1).any(|value| value == "-exec")
            && tokens
                .iter()
                .skip(index + 1)
                .any(|value| value == "rm" || value == "remove-item")
        {
            return true;
        }
        if token == "git"
            && tokens
                .iter()
                .skip(index + 1)
                .enumerate()
                .any(|(offset, value)| {
                    offset == 0
                        && value == "clean"
                        && tokens.iter().skip(index + 2).any(|flag| {
                            flag == "-f"
                                || flag == "--force"
                                || (flag.starts_with('-') && flag.contains('f'))
                        })
                })
        {
            return true;
        }
    }
    false
}

pub fn is_forbidden_destructive_command(input: &Value) -> bool {
    let Some(command) = input.get("command").and_then(Value::as_str) else {
        return false;
    };
    let args = input
        .get("args")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if args.is_empty() {
        is_forbidden_destructive_command_line(command)
    } else {
        is_forbidden_destructive_command_line(&format!("{command} {}", args.join(" ")))
    }
}

pub fn is_allowed_windows_shell(shell: &str) -> bool {
    matches!(shell, "powershell" | "pwsh" | "cmd" | "bash" | "wsl")
}

pub fn shell_process_args(shell: &str, command: &str) -> Result<Vec<String>, String> {
    match shell {
        "powershell" | "pwsh" => Ok(vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            format!("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {command}"),
        ]),
        "cmd" => Ok(vec![
            "/d".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            format!("chcp 65001 >NUL & {command}"),
        ]),
        "bash" => Ok(vec![
            "--noprofile".to_string(),
            "--norc".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ]),
        "wsl" => Ok(vec![
            "--exec".to_string(),
            "/bin/sh".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ]),
        _ => Err(format!("Windows shell 不受支持: {shell}")),
    }
}

pub fn controlled_tool_for_command(command: &str) -> Option<&'static str> {
    let name = Path::new(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    match name.as_str() {
        "dir" | "ls" | "gci" | "get-childitem" => Some("list_local_files"),
        "cat" | "type" | "gc" | "get-content" => Some("read_local_file"),
        "pwd" | "get-location" => Some("get_working_directory"),
        "grep" => Some("run_local_command with rg"),
        _ => None,
    }
}

pub fn is_windows_shell_command(command: &str) -> bool {
    let name = Path::new(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "powershell" | "pwsh" | "cmd" | "bash" | "wsl"
    )
}

pub fn is_allowed_command(command: &str) -> bool {
    let name = Path::new(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "git"
            | "rg"
            | "findstr"
            | "where"
            | "tree"
            | "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "node"
            | "adb"
            | "java"
            | "javac"
            | "python"
            | "python3"
            | "dotnet"
            | "mvn"
            | "gradle"
            | "gradlew"
            | "cmake"
            | "ninja"
            | "go"
            | "rustc"
            | "android"
            | "trace_processor"
            | "trace_processor_shell"
            | "sdkmanager"
            | "avdmanager"
    )
}
