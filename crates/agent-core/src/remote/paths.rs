use super::*;

pub(super) fn cancelled(cancellation: &Arc<AtomicBool>) -> Result<(), String> {
    if cancellation.load(Ordering::Relaxed) {
        Err("请求已取消".to_string())
    } else {
        Ok(())
    }
}

pub(super) fn normalize_remote_path(path: &str) -> String {
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
    format!("/{}", parts.join("/"))
}

pub(super) fn remote_directory_entry_path(parent: &str, entry: &Path) -> String {
    let name = entry
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

pub(super) fn is_symbolic_remote_path(path: &str) -> bool {
    let value = path.trim();
    value == "~" || value.starts_with("~/") || value.starts_with('$')
}

pub(super) fn remote_shell_path_expression(path: &str) -> Result<String, String> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("远程工作目录不能为空".to_string());
    }
    if normalized.chars().any(|character| character.is_control()) {
        return Err("远程工作目录包含不可用字符".to_string());
    }
    let absolute = normalized.starts_with('/');
    let mut expression = if absolute {
        "/".to_string()
    } else {
        String::new()
    };
    let mut first = true;
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        let value = if first && segment == "~" {
            "$HOME".to_string()
        } else if let Some(variable) = segment
            .strip_prefix("${")
            .and_then(|value| value.strip_suffix('}'))
        {
            if is_remote_env_name(variable) {
                format!("${{{variable}}}")
            } else {
                return Err("远程路径中的环境变量名称无效".to_string());
            }
        } else if let Some(variable) = segment.strip_prefix('$') {
            if is_remote_env_name(variable) {
                format!("${variable}")
            } else {
                return Err("远程路径中的环境变量名称无效".to_string());
            }
        } else if segment == "~" {
            return Err("~ 只能出现在远程路径开头".to_string());
        } else {
            shell_escape(segment)
        };
        if !expression.is_empty() && !expression.ends_with('/') {
            expression.push('/');
        }
        expression.push_str(&value);
        first = false;
    }
    if expression.is_empty() {
        return Ok("/".to_string());
    }
    Ok(expression)
}

pub(super) fn is_remote_env_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().enumerate().all(|(index, character)| {
            if index == 0 {
                character == '_' || character.is_ascii_alphabetic()
            } else {
                character == '_' || character.is_ascii_alphanumeric()
            }
        })
}

pub(super) fn remote_path_is_within_root(path: &str, root: &str) -> bool {
    root == "/" || path == root || path.starts_with(&format!("{root}/"))
}

pub(super) fn remote_path_is_same_or_descendant(path: &str, root: &str) -> bool {
    path == root
        || if root == "/" {
            path.starts_with('/')
        } else {
            path.starts_with(&format!("{root}/"))
        }
}

pub(super) fn remote_path(context: &SshToolContext, input: &Value) -> Result<String, String> {
    let raw = input
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&context.root);
    let raw = raw.trim().replace('\\', "/");
    let root = normalize_remote_path(&context.root);
    let target = if raw.starts_with('/') {
        normalize_remote_path(&raw)
    } else {
        normalize_remote_path(&format!("{root}/{raw}"))
    };
    if !context.allow_external_paths && !remote_path_is_within_root(&target, &root) {
        return Err("远程路径必须位于当前 SSH 工作目录内".to_string());
    }
    Ok(target)
}

pub(super) fn ensure_remote_path_safe(
    sftp: &ssh2::Sftp,
    path: &str,
    root: &str,
    cancellation: &Arc<AtomicBool>,
    operation: &str,
) -> Result<(), String> {
    let root_real = retry_nonblocking_with_cancel(
        "解析远程工作目录真实路径",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.realpath(Path::new(root)),
    )?;
    let target_stat = remote_sftp_lstat_if_exists(sftp, path, cancellation, operation)?;
    if target_stat
        .as_ref()
        .is_some_and(|stat| matches!(stat.file_type(), ssh2::FileType::Symlink))
    {
        return Err(format!("{operation} 不允许通过符号链接访问目标：{path}"));
    }
    let real_path = match retry_nonblocking_with_cancel(
        "解析远程目标真实路径",
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        cancellation,
        || sftp.realpath(Path::new(path)),
    ) {
        Ok(path) => path,
        Err(_) => {
            let parent = remote_parent_path(path);
            let parent_real = retry_nonblocking_with_cancel(
                "解析远程目标父目录真实路径",
                Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
                cancellation,
                || sftp.realpath(Path::new(parent)),
            )?;
            parent_real.join(
                Path::new(path)
                    .file_name()
                    .ok_or_else(|| format!("{operation} 目标路径无效：{path}"))?,
            )
        }
    };
    let root = normalize_remote_path(&root_real.to_string_lossy());
    let target = normalize_remote_path(&real_path.to_string_lossy());
    if !remote_path_is_within_root(&target, &root) {
        return Err(format!(
            "{operation} 目标通过符号链接越出 SSH 工作目录：{path}"
        ));
    }
    Ok(())
}

pub(super) fn search_match_kind(
    query: &str,
    name: &str,
    path: &str,
    content: Option<&str>,
) -> Option<&'static str> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    let name = name.to_lowercase();
    let path = path.to_lowercase();
    if name.contains(&query) || path.contains(&query) {
        return Some("name");
    }
    if let Some(content) = content {
        if content.to_lowercase().contains(&query) {
            return Some("content");
        }
    }

    // Let one search cover a natural-language query after the model has
    // expanded it into multiple useful terms. This is still deterministic:
    // the model decides the terms, while the tool only matches evidence.
    let terms = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.len() > 1
        && terms
            .iter()
            .all(|term| name.contains(term) || path.contains(term))
    {
        return Some("terms");
    }
    if let Some(content) = content {
        let content = content.to_lowercase();
        if terms.len() > 1 && terms.iter().all(|term| content.contains(term)) {
            return Some("terms");
        }
    }
    None
}

pub(super) fn remote_output_sink_for_input(
    context: &SshToolContext,
    input: &Value,
) -> Option<CommandOutputSink> {
    let call_id = input
        .get("__agent_call_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    context.output_sink.as_ref().map(|sink| {
        call_id
            .map(|value| sink.with_call_id(value))
            .unwrap_or_else(|| sink.clone())
    })
}

pub(super) fn ssh_error(error: impl std::fmt::Display) -> String {
    format!("SSH 操作失败: {error}")
}
