use std::fs;
use std::path::{Component, Path, PathBuf, Prefix};

pub fn strip_windows_extended_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix("\\\\?\\UNC\\") {
            return PathBuf::from(format!("\\\\{unc}"));
        }
        if let Some(normal) = value.strip_prefix("\\\\?\\") {
            return PathBuf::from(normal);
        }
    }
    path.to_path_buf()
}

pub fn normalized_policy_path(path: &Path) -> PathBuf {
    normalized_policy_path_from(path, None)
}

pub fn normalized_policy_path_from(path: &Path, base: Option<&Path>) -> PathBuf {
    let path = resolve_path_aliases(&strip_windows_extended_prefix(path));
    let absolute = if let Some((drive, suffix)) = windows_drive_relative_parts(&path) {
        let current_directory = if base.is_none() {
            std::env::current_dir()
                .ok()
                .filter(|directory| windows_drive_letter(directory) == Some(drive))
        } else {
            None
        };
        let drive_base = base.or(current_directory.as_deref());
        drive_relative_path(drive, &suffix, drive_base).unwrap_or(path)
    } else if path.is_absolute() || is_windows_absolute(&path) {
        path
    } else {
        base.map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(path)
    };
    let absolute = strip_windows_extended_prefix(&absolute);
    let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
    let absolute = strip_windows_extended_prefix(&absolute);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Do not pop a drive or UNC root. Popping the root from
                // `C:\..\file` would turn the result into drive-relative
                // `C:file`, which has different Windows semantics.
                if normalized
                    .components()
                    .next_back()
                    .is_some_and(|last| matches!(last, Component::Normal(_)))
                {
                    let _ = normalized.pop();
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR_STR),
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

pub fn policy_path_key(path: &Path) -> String {
    let mut value = normalized_policy_path(path)
        .to_string_lossy()
        .replace('/', "\\");
    #[cfg(windows)]
    {
        value = value.to_ascii_lowercase();
    }
    while value.len() > 3 && value.ends_with('\\') {
        value.pop();
    }
    value
}

pub fn is_selected_path(path: &Path, roots: &[String]) -> bool {
    let target = policy_path_key(&normalized_policy_path(&resolve_selected_path(path, roots)));
    roots.iter().any(|root| {
        let root_path = normalized_policy_path(Path::new(root));
        let root_key = policy_path_key(&root_path);
        if target == root_key {
            return true;
        }
        target.starts_with(&format!("{root_key}\\"))
            && fs::metadata(&root_path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(true)
    })
}

pub fn is_explicitly_selected_root(path: &Path, roots: &[String]) -> bool {
    let target = policy_path_key(&normalized_policy_path(&resolve_selected_path(path, roots)));
    roots
        .iter()
        .any(|root| policy_path_key(Path::new(root)) == target)
}

pub fn is_allowed_agent_path(path: &Path, roots: &[String], allow_external_paths: bool) -> bool {
    allow_external_paths || is_selected_path(path, roots)
}

pub fn selected_working_directory(roots: &[String]) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let path = normalized_policy_path(Path::new(root));
        match fs::metadata(&path).ok() {
            Some(metadata) if metadata.is_dir() => Some(strip_windows_extended_prefix(&path)),
            Some(_) => path
                .parent()
                .map(Path::to_path_buf)
                .map(|parent| strip_windows_extended_prefix(&parent)),
            None => None,
        }
    })
}

pub fn resolve_user_directory_alias(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy().replace('\\', "/");
    let value = raw.trim_start_matches('/').trim_start_matches("./");
    let (name, remainder) = value.split_once('/').unwrap_or((value, ""));
    let directory = match name.to_ascii_lowercase().as_str() {
        "desktop" | "桌面" => "Desktop",
        "documents" | "文档" => "Documents",
        "downloads" | "下载" => "Downloads",
        _ => return None,
    };
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let home_path = PathBuf::from(home);
    let mut resolved = home_path.join(directory);
    if directory == "Desktop" {
        for variable in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
            let Some(one_drive) = std::env::var_os(variable) else {
                continue;
            };
            let candidate = PathBuf::from(one_drive).join(directory);
            if candidate.is_dir() {
                resolved = candidate;
                break;
            }
        }
    }
    if !remainder.is_empty() {
        resolved.push(remainder);
    }
    Some(resolved)
}

pub fn resolve_environment_path(raw: &str) -> Option<PathBuf> {
    let (variable, remainder) = environment_reference(raw)?;
    let value = std::env::var_os(variable)?;
    Some(PathBuf::from(value).join(remainder.trim_start_matches(['/', '\\'])))
}

pub fn resolve_selected_path(path: &Path, roots: &[String]) -> PathBuf {
    let path = resolve_path_aliases(&strip_windows_extended_prefix(path));
    if let Some((drive, suffix)) = windows_drive_relative_parts(&path) {
        if let Some(directory) = selected_working_directory(roots)
            .filter(|directory| windows_drive_letter(directory) == Some(drive))
        {
            return drive_relative_path(drive, &suffix, Some(&directory))
                .unwrap_or_else(|| path.clone());
        }
        if let Some(directory) = std::env::current_dir()
            .ok()
            .filter(|directory| windows_drive_letter(directory) == Some(drive))
        {
            return drive_relative_path(drive, &suffix, Some(&directory))
                .unwrap_or_else(|| path.clone());
        }
        // A drive-relative path has no portable meaning without that
        // drive's current directory. Keep it unresolved instead of turning
        // `D:foo` into a path on the process' current drive.
        return path;
    }
    if path.is_absolute() || is_windows_absolute(&path) {
        return path;
    }
    selected_working_directory(roots)
        .map(|directory| directory.join(&path))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|directory| directory.join(&path))
        })
        .unwrap_or(path)
}

pub fn resolve_command_working_directory(
    cwd: Option<&str>,
    roots: &[String],
    allow_external_paths: bool,
) -> Result<PathBuf, String> {
    let selected = selected_working_directory(roots);
    let requested_cwd = cwd.filter(|value| !value.trim().is_empty());
    if selected.is_none() && requested_cwd.is_none() {
        return Err(if allow_external_paths {
            "独立对话执行命令需要显式提供工作目录 cwd，避免使用应用进程目录".to_string()
        } else {
            "请先选择工作目录或文件".to_string()
        });
    }
    let candidate = requested_cwd
        .map(PathBuf::from)
        .map(|path| resolve_selected_path(&path, roots))
        .or(selected)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "请先选择工作目录或文件".to_string())?;
    let normalized = normalized_policy_path_from(&candidate, None);
    if !is_allowed_agent_path(&normalized, roots, allow_external_paths)
        || !fs::metadata(&normalized)
            .map(|value| value.is_dir())
            .unwrap_or(false)
    {
        return Err("命令工作目录不在用户选择范围内".to_string());
    }
    Ok(normalized)
}

fn resolve_path_aliases(path: &Path) -> PathBuf {
    if let Some(alias) = resolve_user_directory_alias(path) {
        return alias;
    }
    let raw = path.to_string_lossy();
    if let Some(expanded) = resolve_environment_path(&raw) {
        return expanded;
    }
    if let Some(remainder) = raw
        .strip_prefix('~')
        .filter(|remainder| remainder.is_empty() || remainder.starts_with(['/', '\\']))
    {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            return PathBuf::from(home).join(remainder.trim_start_matches(['/', '\\']));
        }
    }
    path.to_path_buf()
}

fn environment_reference(raw: &str) -> Option<(&str, &str)> {
    if raw
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("$env:"))
    {
        let value = &raw[5..];
        let end = value
            .find(|character: char| matches!(character, '/' | '\\'))
            .unwrap_or(value.len());
        return Some((&value[..end], &value[end..]));
    }
    if let Some(value) = raw.strip_prefix('%') {
        let end = value.find('%')?;
        return Some((&value[..end], &value[end + 1..]));
    }
    if let Some(value) = raw.strip_prefix("${") {
        let end = value.find('}')?;
        return Some((&value[..end], &value[end + 1..]));
    }
    if let Some(value) = raw.strip_prefix('$') {
        let end = value
            .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .unwrap_or(value.len());
        return Some((&value[..end], &value[end..]));
    }
    None
}

fn windows_drive_relative_parts(path: &Path) -> Option<(u8, String)> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() < 2
        || bytes[1] != b':'
        || !bytes[0].is_ascii_alphabetic()
        || bytes
            .get(2)
            .is_some_and(|value| matches!(value, b'/' | b'\\'))
    {
        return None;
    }
    Some((bytes[0].to_ascii_lowercase(), raw[2..].to_string()))
}

fn windows_drive_letter(path: &Path) -> Option<u8> {
    path.components().find_map(|component| {
        let Component::Prefix(prefix) = component else {
            return None;
        };
        match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(letter.to_ascii_lowercase())
            }
            _ => None,
        }
    })
}

fn drive_relative_path(drive: u8, suffix: &str, base: Option<&Path>) -> Option<PathBuf> {
    let base = base?;
    if windows_drive_letter(base) != Some(drive) {
        return None;
    }
    let suffix = suffix.trim_start_matches(['/', '\\']);
    if suffix.is_empty() {
        Some(base.to_path_buf())
    } else {
        Some(base.join(suffix))
    }
}

fn is_windows_absolute(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw.starts_with("\\\\")
        || (raw.len() >= 3
            && raw.as_bytes()[0].is_ascii_alphabetic()
            && raw.as_bytes()[1] == b':'
            && matches!(raw.as_bytes()[2], b'/' | b'\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-core-path-policy-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ))
    }

    fn available_environment_variable() -> (&'static str, std::ffi::OsString) {
        ["USERPROFILE", "HOME", "SystemRoot"]
            .into_iter()
            .find_map(|name| std::env::var_os(name).map(|value| (name, value)))
            .expect("a user or system home environment variable should exist")
    }

    #[test]
    fn expands_supported_environment_syntax_without_dropping_suffixes() {
        let (name, value) = available_environment_variable();
        let expected = PathBuf::from(value).join("agent-core-path-policy-child");
        for raw in [
            format!("%{name}%/agent-core-path-policy-child"),
            format!("${{{name}}}/agent-core-path-policy-child"),
            format!("${name}/agent-core-path-policy-child"),
            format!("$env:{name}/agent-core-path-policy-child"),
        ] {
            assert_eq!(
                resolve_environment_path(&raw),
                Some(expected.clone()),
                "{raw}"
            );
        }
    }

    #[test]
    fn resolves_home_and_user_directory_aliases() {
        let (_, home) = available_environment_variable();
        assert_eq!(
            resolve_selected_path(Path::new("~/agent-core-path-policy-child"), &[]),
            PathBuf::from(&home).join("agent-core-path-policy-child")
        );
        assert_eq!(
            resolve_user_directory_alias(Path::new("Documents\\nested")),
            Some(PathBuf::from(home).join("Documents").join("nested"))
        );
    }

    #[test]
    fn parent_segments_escape_the_selected_root_boundary() {
        let root = test_directory("boundary");
        std::fs::create_dir_all(&root).expect("test root should be created");
        let escaped = root.join("..").join("agent-core-path-policy-escaped");
        assert!(!is_selected_path(
            &escaped,
            &[root.to_string_lossy().into_owned()]
        ));
        assert!(resolve_command_working_directory(
            Some(escaped.to_string_lossy().as_ref()),
            &[root.to_string_lossy().into_owned()],
            false,
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn drive_relative_paths_use_the_selected_drive_and_do_not_switch_drives() {
        let root = test_directory("drive");
        let child = root.join("child");
        let outside = root
            .parent()
            .expect("test directory should have a parent")
            .join("agent-core-path-policy-outside");
        std::fs::create_dir_all(&child).expect("test child should be created");
        std::fs::create_dir_all(&outside).expect("test outside directory should be created");
        let drive = windows_drive_letter(&root).expect("temporary directory should have a drive");
        let drive_name = drive as char;
        let relative = PathBuf::from(format!("{drive_name}:child"));
        let roots = vec![root.to_string_lossy().into_owned()];

        assert_eq!(
            normalized_policy_path(&resolve_selected_path(&relative, &roots)),
            normalized_policy_path(&child)
        );
        assert!(resolve_command_working_directory(
            Some(relative.to_string_lossy().as_ref()),
            &roots,
            false,
        )
        .is_ok());

        let escape = PathBuf::from(format!("{drive_name}:..\\agent-core-path-policy-outside"));
        assert!(resolve_command_working_directory(
            Some(escape.to_string_lossy().as_ref()),
            &roots,
            false,
        )
        .is_err());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_extended_and_unc_paths_without_popping_the_root() {
        assert_eq!(
            strip_windows_extended_prefix(Path::new(r"\\?\C:\workspace\file")),
            PathBuf::from(r"C:\workspace\file")
        );
        assert_eq!(
            normalized_policy_path(Path::new(r"\\server\share\folder\..\file")),
            PathBuf::from(r"\\server\share\file")
        );
        assert_eq!(
            normalized_policy_path(Path::new(r"C:\..\..\file")),
            PathBuf::from(r"C:\file")
        );
    }
}
