use crate::{browser_mutations::rename_no_replace, transfer_archive::safe_transfer_name};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Keep the connection label, appending the host only when it is not already
/// present as a complete address (rather than a substring of another IP).
pub fn browser_download_server_directory(name: &str, host: &str, port: u16) -> String {
    let contains_host = name.match_indices(host).any(|(index, _)| {
        let before = name[..index].chars().next_back();
        let after = name[index + host.len()..].chars().next();
        let address_char = |ch: char| ch.is_alphanumeric() || ch == '.' || ch == ':';
        !before.is_some_and(address_char) && !after.is_some_and(address_char)
    });
    let label = if contains_host {
        format!("{name}-{port}")
    } else {
        format!("{name}-{host}-{port}")
    };
    let label: String = label
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .take(120)
        .collect();
    format!("server-{}", label.trim_end_matches([' ', '.']))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownloadStatus {
    pub path: String,
    pub status: &'static str,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub error: Option<String>,
}

pub fn inspect_browser_download(path: &str) -> BrowserDownloadStatus {
    let metadata = if Path::new(path).is_absolute() {
        fs::symlink_metadata(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "必须使用本地绝对路径",
        ))
    };
    match metadata {
        Ok(metadata) => BrowserDownloadStatus {
            path: path.into(),
            status: "present",
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            error: None,
        },
        Err(error) => BrowserDownloadStatus {
            path: path.into(),
            status: if error.kind() == std::io::ErrorKind::NotFound {
                "missing"
            } else {
                "unavailable"
            },
            is_directory: false,
            is_symlink: false,
            error: Some(error.to_string()),
        },
    }
}

/// Publish one extracted item directly into a chosen directory. Never merge
/// directories or replace a same-named item, even if another job finishes first.
pub fn publish_browser_download(
    staging: &Path,
    directory: &Path,
    name: &str,
) -> Result<PathBuf, String> {
    if !safe_transfer_name(name) {
        return Err("下载文件名无效".into());
    }
    let source = staging.join(name);
    let is_directory = fs::metadata(&source)
        .map_err(|error| error.to_string())?
        .is_dir();
    let name_path = Path::new(name);
    let (stem, extension) = if is_directory {
        (name, None)
    } else {
        (
            name_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(name),
            name_path.extension().and_then(|value| value.to_str()),
        )
    };
    for index in 0..10_000 {
        let candidate = if index == 0 {
            name.into()
        } else {
            format!(
                "{stem} ({index}){}",
                extension.map(|ext| format!(".{ext}")).unwrap_or_default()
            )
        };
        let destination = directory.join(candidate);
        match rename_no_replace(&source, &destination) {
            Ok(()) => return Ok(destination),
            Err(_) if fs::symlink_metadata(&destination).is_ok() => continue,
            Err(error) => return Err(format!("无法保存下载文件：{error}")),
        }
    }
    Err("下载目录同名文件过多，请清理或更换目录".into())
}

pub fn validate_browser_download_path(
    path: &Path,
    roots: &[PathBuf],
    recorded: &[String],
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("下载文件路径必须是本地绝对路径".into());
    }
    let resolved =
        fs::canonicalize(path).map_err(|error| format!("下载文件不存在或无法访问：{error}"))?;
    let allowed = roots
        .iter()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| resolved.starts_with(root))
        || recorded.iter().any(|saved| Path::new(saved) == resolved);
    if !allowed {
        return Err("该路径不属于下载目录或已完成的下载记录".into());
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_names_do_not_duplicate_addresses_and_missing_is_distinct_from_invalid() {
        assert_eq!(
            browser_download_server_directory("app1whu@10.179.85.15", "10.179.85.15", 2202),
            "server-app1whu@10.179.85.15-2202"
        );
        assert_eq!(
            browser_download_server_directory("N60", "10.179.85.15", 2202),
            "server-N60-10.179.85.15-2202"
        );
        assert_eq!(
            browser_download_server_directory("app@10.179.85.150", "10.179.85.15", 2202),
            "server-app@10.179.85.150-10.179.85.15-2202"
        );
        assert_eq!(
            inspect_browser_download("relative.txt").status,
            "unavailable"
        );
        let root = std::env::temp_dir().join(format!(
            "android-tool-download-status-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), "data").unwrap();
        assert!(inspect_browser_download(root.to_str().unwrap()).is_directory);
        assert_eq!(
            inspect_browser_download(root.join("file").to_str().unwrap()).status,
            "present"
        );
        fs::remove_file(root.join("file")).unwrap();
        assert_eq!(
            inspect_browser_download(root.join("file").to_str().unwrap()).status,
            "missing"
        );
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn downloads_publish_without_timestamp_or_overwriting_and_retain_file_extensions() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-download-publish-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".staging/source")).unwrap();
        fs::write(root.join(".staging/log.txt"), "new").unwrap();
        fs::write(root.join("log.txt"), "old").unwrap();
        fs::write(root.join(".staging/source/file"), "nested").unwrap();
        fs::write(root.join("outside.txt"), "outside").unwrap();
        let result = publish_browser_download(&root.join(".staging"), &root, "log.txt").unwrap();
        assert_eq!(result, root.join("log (1).txt"));
        assert_eq!(fs::read_to_string(root.join("log.txt")).unwrap(), "old");
        assert_eq!(fs::read_to_string(&result).unwrap(), "new");
        let directory = publish_browser_download(&root.join(".staging"), &root, "source").unwrap();
        assert_eq!(directory, root.join("source"));
        assert_eq!(
            fs::read_to_string(directory.join("file")).unwrap(),
            "nested"
        );
        assert!(publish_browser_download(&root.join(".staging"), &root, "../outside.txt").is_err());
        assert_eq!(
            validate_browser_download_path(&result, &[root.clone()], &[]).unwrap(),
            fs::canonicalize(&result).unwrap()
        );
        let recorded = fs::canonicalize(&result)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(validate_browser_download_path(&result, &[], &[recorded]).is_ok());
        assert!(validate_browser_download_path(&root.join("outside.txt"), &[], &[]).is_err());
        assert!(
            validate_browser_download_path(&root.join("missing.txt"), &[root.clone()], &[])
                .is_err()
        );
        assert!(
            validate_browser_download_path(Path::new("relative.txt"), &[root.clone()], &[])
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
