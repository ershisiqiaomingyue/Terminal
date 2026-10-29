use crate::{
    command_path_tokens, is_allowed_agent_path, normalized_policy_path, policy_path_key,
    resolve_selected_path, selected_working_directory,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) fn cancelled(cancellation: &Arc<AtomicBool>) -> Result<(), String> {
    if cancellation.load(Ordering::Relaxed) {
        Err("请求已取消".to_string())
    } else {
        Ok(())
    }
}

pub(super) fn selected_tool_path(
    input: &Value,
    roots: &[String],
    allow_external_paths: bool,
) -> Result<PathBuf, String> {
    input
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .map(|path| resolve_selected_path(&path, roots))
        .or_else(|| selected_working_directory(roots))
        .ok_or_else(|| {
            if allow_external_paths {
                "请提供要访问的路径".to_string()
            } else {
                "请先选择工作目录或文件".to_string()
            }
        })
}

pub(super) fn is_excluded_path(path: &Path, excluded_paths: &[String]) -> bool {
    if excluded_paths.is_empty() {
        return false;
    }
    let target = policy_path_key(&normalized_policy_path(path));
    excluded_paths.iter().any(|excluded| {
        let excluded = policy_path_key(Path::new(excluded));
        target == excluded || target.starts_with(&format!("{excluded}\\"))
    })
}

pub(super) fn is_allowed_tool_path(
    path: &Path,
    roots: &[String],
    excluded_paths: &[String],
    allow_external_paths: bool,
) -> bool {
    is_allowed_agent_path(path, roots, allow_external_paths)
        && !is_excluded_path(path, excluded_paths)
}

pub(super) fn command_uses_excluded_path(
    command: &str,
    args: &[String],
    cwd: &Path,
    roots: &[String],
    excluded_paths: &[String],
) -> bool {
    let mut values = command_path_tokens(command);
    values.extend(args.iter().cloned());
    values.into_iter().any(|value| {
        if !(value.contains('/') || value.contains('\\') || Path::new(&value).is_absolute()) {
            return false;
        }
        let raw_path = Path::new(&value);
        let path = if raw_path.is_absolute() {
            resolve_selected_path(raw_path, roots)
        } else {
            cwd.join(raw_path)
        };
        is_excluded_path(&path, excluded_paths)
    })
}

pub(super) fn skip_agent_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                ".git"
                    | ".cache"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | "node_modules"
                    | "target"
                    | "dist"
                    | "build"
                    | ".gradle"
            )
        })
        .unwrap_or(false)
}
