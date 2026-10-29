//! Explicit file-browser actions. No trash, snapshots or backups are created.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserMutationRequest {
    pub root: String,
    pub path: String,
    pub action: String,
    pub new_name: Option<String>,
    pub recursive: bool,
    pub expected_version: Option<String>,
    pub acknowledged: bool,
    #[serde(default)]
    pub destination_directory: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserMutationPreview {
    pub command: String,
    pub version: String,
    pub destination: Option<String>,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub conflict: bool,
}

pub(crate) fn mutation_name(name: Option<&str>) -> Result<&str, String> {
    let name = name.ok_or("请输入新名称")?;
    if name.trim().is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err("新名称不能为空，也不能包含路径分隔符".into());
    }
    Ok(name)
}

fn local_target(request: &BrowserMutationRequest) -> Result<PathBuf, String> {
    let root = Path::new(&request.root);
    let path = Path::new(&request.path);
    // Windows listings/canonicalization may add \\?\ while the directory
    // picker keeps C:\. Compare equivalent lexical paths without resolving
    // directory links, which users may intentionally browse outside the root.
    let scope_root = local_scope_path(root);
    let scope_path = local_scope_path(path);
    if !root.is_absolute()
        || !path.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || !path_in_scope(&scope_path, &scope_root)
        || (!is_creation(request) && (scope_path == scope_root || path.file_name().is_none()))
    {
        return Err("只能操作当前浏览目录内的文件或子目录，禁止操作根目录".into());
    }
    if is_creation(request) {
        let parent = fs::canonicalize(path).map_err(|error| format!("无法访问目录：{error}"))?;
        if !parent.is_dir() {
            return Err("只能在文件夹内新建".into());
        }
        return Ok(parent);
    }
    // Resolve the parent only: removing or renaming a symlink must act on the
    // link itself, including a broken link, never its final target.
    let parent = fs::canonicalize(path.parent().ok_or("路径没有父目录")?)
        .map_err(|error| format!("无法访问父目录：{error}"))?;
    Ok(parent.join(path.file_name().ok_or("文件名为空")?))
}

fn path_in_scope(path: &Path, base: &Path) -> bool {
    let path = local_scope_path(path);
    let base = local_scope_path(base);
    path == base || path.starts_with(base)
}

fn local_scope_path(path: &Path) -> PathBuf {
    let path = crate::path_policy::strip_windows_extended_prefix(path);
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    path
}

pub(crate) fn is_creation(request: &BrowserMutationRequest) -> bool {
    matches!(request.action.as_str(), "create-file" | "create-directory")
}

pub(crate) fn is_move(request: &BrowserMutationRequest) -> bool {
    request.action == "move"
}

fn local_move_destination(
    request: &BrowserMutationRequest,
    path: &Path,
) -> Result<PathBuf, String> {
    let directory = Path::new(
        request
            .destination_directory
            .as_deref()
            .ok_or("请选择移动目标文件夹")?,
    );
    let root = local_scope_path(Path::new(&request.root));
    let scope_directory = local_scope_path(directory);
    if !directory.is_absolute()
        || !path_in_scope(&scope_directory, &root)
        || directory
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("移动目标必须位于当前浏览目录内".into());
    }
    let source_parent = local_scope_path(path.parent().ok_or("路径没有父目录")?);
    if source_parent == scope_directory {
        return Err("文件已位于该文件夹".into());
    }
    let directory =
        fs::canonicalize(directory).map_err(|error| format!("无法访问目标文件夹：{error}"))?;
    if !directory.is_dir() {
        return Err("移动目标不是文件夹".into());
    }
    Ok(directory.join(path.file_name().ok_or("文件名为空")?))
}

fn local_move_version(request: &BrowserMutationRequest, path: &Path) -> Result<String, String> {
    let directory = Path::new(
        request
            .destination_directory
            .as_deref()
            .ok_or("请选择移动目标文件夹")?,
    );
    let destination = directory.join(path.file_name().ok_or("文件名为空")?);
    let target = match fs::symlink_metadata(&destination) {
        Ok(metadata) => format!(
            "present:{}:{}:{:?}:{:?}",
            metadata.len(),
            metadata.file_type().is_dir(),
            metadata.modified().ok(),
            metadata.created().ok()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing".into(),
        Err(error) => return Err(format!("无法检查移动目标：{error}")),
    };
    Ok(format!(
        "{}|{}|{}",
        local_version(path)?,
        local_version(directory)?,
        target
    ))
}

fn remove_local_entry(path: &Path) -> Result<(), String> {
    let file_type = fs::symlink_metadata(path)
        .map_err(|error| format!("无法检查同名目标：{error}"))?
        .file_type();
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if file_type.is_symlink_dir() {
            return fs::remove_dir(path).map_err(|error| format!("无法移除同名目标：{error}"));
        }
    }
    if file_type.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("无法移除同名目标：{error}"))
}

fn local_name(request: &BrowserMutationRequest) -> Result<&str, String> {
    let name = mutation_name(request.new_name.as_deref())?;
    #[cfg(windows)]
    if name.ends_with(['.', ' '])
        || name.contains(['<', '>', ':', '"', '|', '?', '*'])
        || name.chars().any(|ch| ch.is_control())
        || matches!(
            name.split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
    {
        return Err("名称包含 Windows 不允许的字符或保留名称".into());
    }
    Ok(name)
}

fn local_version(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| format!("无法检查目标：{error}"))?;
    Ok(format!(
        "{:?}|{}|{:?}|{:?}|{:?}",
        path,
        metadata.len(),
        metadata.modified(),
        metadata.created(),
        metadata.file_type()
    ))
}

pub(crate) fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // Zero flags atomically reject an existing destination, including a
        // target created after the preview. std::fs::rename can overwrite it.
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn renameat2(
                old_dir: i32,
                old: *const std::ffi::c_char,
                new_dir: i32,
                new: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        if unsafe { renameat2(-100, source.as_ptr(), -100, destination.as_ptr(), 1) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "此平台尚不支持保证不覆盖的重命名",
        ))
    }
}

pub fn preview_local_browser_mutation(
    request: &BrowserMutationRequest,
) -> Result<BrowserMutationPreview, String> {
    let path = local_target(request)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| format!("无法检查目标：{error}"))?;
    let is_symlink = metadata.file_type().is_symlink();
    let is_directory = if is_creation(request) {
        request.action == "create-directory"
    } else {
        metadata.is_dir() && !is_symlink
    };
    let mut conflict = false;
    let (command, destination) = match request.action.as_str() {
        "create-file" | "create-directory" => {
            let name = local_name(request)?;
            let destination = path.join(name);
            if fs::symlink_metadata(&destination).is_ok() {
                return Err("同名目标已存在，新建不会覆盖".into());
            }
            let operation = if request.action == "create-file" {
                "OpenOptions::create_new(true)"
            } else {
                "std::fs::create_dir"
            };
            (
                format!("{operation}({destination:?})"),
                Some(
                    Path::new(&request.path)
                        .join(name)
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
        }
        "rename" => {
            let name = local_name(request)?;
            let destination = path.with_file_name(name);
            if fs::symlink_metadata(&destination).is_ok() {
                return Err("同名目标已存在，重命名不会覆盖".into());
            }
            let operation = if cfg!(windows) {
                "MoveFileExW（flags=0，不覆盖）"
            } else {
                "renameat2（RENAME_NOREPLACE）"
            };
            (
                format!("{operation}({:?}, {:?})", path, destination),
                Some(
                    Path::new(&request.path)
                        .with_file_name(name)
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
        }
        "move" => {
            let destination = local_move_destination(request, &path)?;
            if is_directory && path_in_scope(&destination, &path) {
                return Err("不能将文件夹移动到自身的子目录".into());
            }
            let destination_scope = request
                .destination_directory
                .as_deref()
                .ok_or("请选择移动目标文件夹")?;
            let visible_destination = Path::new(destination_scope)
                .join(path.file_name().ok_or("文件名为空")?)
                .to_string_lossy()
                .into_owned();
            if fs::symlink_metadata(&destination).is_ok() {
                conflict = true;
                if !request.overwrite {
                    (
                        format!(
                            "发现同名目标；需明确选择覆盖后才能移动（{path:?} -> {destination:?}）"
                        ),
                        Some(visible_destination),
                    )
                } else {
                    (
                        format!("覆盖并移动（永久替换同名目标）({path:?} -> {destination:?})"),
                        Some(visible_destination),
                    )
                }
            } else {
                let operation = if cfg!(windows) {
                    "MoveFileExW（flags=0，不覆盖）"
                } else {
                    "renameat2（RENAME_NOREPLACE）"
                };
                (
                    format!("{operation}({path:?}, {destination:?})"),
                    Some(visible_destination),
                )
            }
        }
        "delete" => {
            #[cfg(windows)]
            let directory_link = {
                use std::os::windows::fs::FileTypeExt;
                metadata.file_type().is_symlink_dir()
            };
            #[cfg(not(windows))]
            let directory_link = false;
            let operation = if is_directory {
                if request.recursive {
                    "remove_dir_all"
                } else {
                    "remove_dir"
                }
            } else if directory_link {
                "remove_dir"
            } else {
                "remove_file"
            };
            (format!("std::fs::{operation}({path:?})"), None)
        }
        _ => return Err("未知文件操作".into()),
    };
    let version = if is_move(request) {
        local_move_version(request, &path)?
    } else {
        local_version(&path)?
    };
    Ok(BrowserMutationPreview {
        command,
        version,
        destination,
        is_directory,
        is_symlink,
        conflict,
    })
}

pub fn execute_local_browser_mutation(
    request: &BrowserMutationRequest,
) -> Result<BrowserMutationPreview, String> {
    if !request.acknowledged {
        return Err("需要先完成二次确认".into());
    }
    let preview = preview_local_browser_mutation(request)?;
    if request.expected_version.as_deref() != Some(preview.version.as_str()) {
        return Err("目标或父目录已变化，请重新确认操作".into());
    }
    let path = local_target(request)?;
    if is_move(request) {
        if local_move_version(request, &path)? != preview.version {
            return Err("目标、父目录或同名项已变化，请重新确认操作".into());
        }
    } else if local_version(&path)? != preview.version {
        return Err("目标已变化，请重新确认操作".into());
    }
    match request.action.as_str() {
        "create-file" => fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(local_name(request)?))
            .map(|_| ()),
        "create-directory" => fs::create_dir(path.join(local_name(request)?)),
        "rename" => rename_no_replace(
            &path,
            &path.with_file_name(mutation_name(request.new_name.as_deref())?),
        ),
        "move" => {
            let destination = local_move_destination(request, &path)?;
            if fs::symlink_metadata(&destination).is_ok() {
                if !request.overwrite {
                    return Err("目标文件夹存在同名项，请明确确认覆盖".into());
                }
                remove_local_entry(&destination)?;
            }
            rename_no_replace(&path, &destination)
        }
        "delete" if preview.is_directory && request.recursive => fs::remove_dir_all(&path),
        "delete" if preview.is_directory => fs::remove_dir(&path),
        "delete" => {
            // Windows directory symlinks require RemoveDirectory; both variants
            // unlink the reparse point without traversing its contents.
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileTypeExt;
                let file_type = fs::symlink_metadata(&path)
                    .map_err(|error| error.to_string())?
                    .file_type();
                if file_type.is_symlink_dir() {
                    fs::remove_dir(&path)
                } else {
                    fs::remove_file(&path)
                }
            }
            #[cfg(not(windows))]
            fs::remove_file(&path)
        }
        _ => return Err("未知文件操作".into()),
    }
    .map_err(|error| format!("文件操作失败，未创建备份：{error}"))?;
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_browser_paths_accept_mixed_prefixes_and_case() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-paths-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("nested")).unwrap();
        let canonical = fs::canonicalize(&root).unwrap();
        let ordinary = crate::path_policy::strip_windows_extended_prefix(&canonical);
        for (scope, folder) in [
            (ordinary.clone(), canonical.clone()),
            (canonical.clone(), ordinary.clone()),
            (
                PathBuf::from(ordinary.to_string_lossy().to_uppercase()),
                canonical.clone(),
            ),
        ] {
            for folder in [folder.clone(), folder.join("nested")] {
                for action in ["create-file", "create-directory"] {
                    let name = format!(
                        "item-{}-{action}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    );
                    let mut request = BrowserMutationRequest {
                        root: scope.to_string_lossy().into(),
                        path: folder.to_string_lossy().into(),
                        action: action.into(),
                        new_name: Some(name.clone()),
                        recursive: false,
                        expected_version: None,
                        acknowledged: true,
                        destination_directory: None,
                        overwrite: false,
                    };
                    request.expected_version =
                        Some(preview_local_browser_mutation(&request).unwrap().version);
                    execute_local_browser_mutation(&request).unwrap();
                    assert!(folder.join(&name).exists());
                    request.path = folder.join(name).to_string_lossy().into();
                    request.action = "rename".into();
                    request.new_name = Some(format!("renamed-{action}"));
                    request.expected_version =
                        Some(preview_local_browser_mutation(&request).unwrap().version);
                    execute_local_browser_mutation(&request).unwrap();
                    request.path = folder
                        .join(format!("renamed-{action}"))
                        .to_string_lossy()
                        .into();
                    request.action = "delete".into();
                    request.expected_version =
                        Some(preview_local_browser_mutation(&request).unwrap().version);
                    execute_local_browser_mutation(&request).unwrap();
                }
            }
            let mut request = BrowserMutationRequest {
                root: scope.to_string_lossy().into(),
                path: canonical.to_string_lossy().into(),
                action: "delete".into(),
                new_name: None,
                recursive: true,
                expected_version: None,
                acknowledged: true,
                destination_directory: None,
                overwrite: false,
            };
            assert!(
                preview_local_browser_mutation(&request).is_err(),
                "root deletion stays forbidden across path forms"
            );
            request.action = "create-file".into();
            request.new_name = Some("escape.txt".into());
            request.path = ordinary
                .with_file_name(format!(
                    "{}-sibling",
                    ordinary.file_name().unwrap().to_string_lossy()
                ))
                .to_string_lossy()
                .into();
            assert!(local_target(&request).unwrap_err().contains("当前浏览目录"));
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn creation_is_confirmed_scoped_and_never_overwrites_even_after_preview() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-create-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let mut request = BrowserMutationRequest {
            root: root.to_string_lossy().into(),
            path: root.to_string_lossy().into(),
            action: "create-file".into(),
            new_name: Some("new.txt".into()),
            recursive: false,
            expected_version: None,
            acknowledged: false,
            destination_directory: None,
            overwrite: false,
        };
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        assert!(execute_local_browser_mutation(&request).is_err());
        request.acknowledged = true;
        execute_local_browser_mutation(&request).unwrap();
        assert_eq!(fs::read(root.join("new.txt")).unwrap(), b"");
        fs::write(root.join("new.txt"), "keep").unwrap();
        assert!(execute_local_browser_mutation(&request).is_err());
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "keep");
        request.new_name = Some("../escape".into());
        assert!(preview_local_browser_mutation(&request).is_err());
        request.new_name = Some("folder".into());
        request.action = "create-directory".into();
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        execute_local_browser_mutation(&request).unwrap();
        assert!(root.join("folder").is_dir());
        assert!(execute_local_browser_mutation(&request).is_err());
        request.path = root.join("new.txt").to_string_lossy().into();
        assert!(preview_local_browser_mutation(&request).is_err());
        request.path = root.parent().unwrap().to_string_lossy().into();
        assert!(preview_local_browser_mutation(&request).is_err());
        request.path = root.to_string_lossy().into();
        request.action = "create-file".into();
        request.new_name = Some("race.txt".into());
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        fs::write(root.join("race.txt"), "concurrent").unwrap();
        assert!(execute_local_browser_mutation(&request).is_err());
        assert_eq!(
            fs::read_to_string(root.join("race.txt")).unwrap(),
            "concurrent"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mutations_require_confirmation_and_never_overwrite_or_implicitly_recurse() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-mutations-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "original").unwrap();
        fs::write(root.join("b.txt"), "existing").unwrap();
        assert!(rename_no_replace(&root.join("a.txt"), &root.join("b.txt")).is_err());
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "existing");
        let mut request = BrowserMutationRequest {
            root: root.to_string_lossy().into(),
            path: root.join("a.txt").to_string_lossy().into(),
            action: "rename".into(),
            new_name: Some("b.txt".into()),
            recursive: false,
            expected_version: None,
            acknowledged: false,
            destination_directory: None,
            overwrite: false,
        };
        assert!(preview_local_browser_mutation(&request).is_err());
        request.new_name = Some("../escape".into());
        assert!(preview_local_browser_mutation(&request).is_err());
        request.new_name = Some("c.txt".into());
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        assert!(execute_local_browser_mutation(&request).is_err());
        request.acknowledged = true;
        execute_local_browser_mutation(&request).unwrap();
        assert_eq!(fs::read_to_string(root.join("c.txt")).unwrap(), "original");
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/child"), "data").unwrap();
        request.path = root.join("dir").to_string_lossy().into();
        request.action = "delete".into();
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        assert!(execute_local_browser_mutation(&request).is_err());
        assert!(root.join("dir/child").exists());
        request.action = "rename".into();
        request.new_name = Some("renamed-dir".into());
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        execute_local_browser_mutation(&request).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("renamed-dir/child")).unwrap(),
            "data"
        );
        request.action = "delete".into();
        request.path = root.join("renamed-dir").to_string_lossy().into();
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        request.recursive = true;
        execute_local_browser_mutation(&request).unwrap();
        assert!(!root.join("renamed-dir").exists());
        request.path = root.to_string_lossy().into();
        assert!(preview_local_browser_mutation(&request).is_err());
        request.path = root.join("c.txt").to_string_lossy().into();
        request.expected_version = Some("stale".into());
        assert!(execute_local_browser_mutation(&request).is_err());
        assert!(root.join("c.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_moves_invalidate_stale_previews_and_require_explicit_overwrite() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-move-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("source/move.txt"), "source").unwrap();
        let mut request = BrowserMutationRequest {
            root: root.to_string_lossy().into(),
            path: root.join("source/move.txt").to_string_lossy().into(),
            action: "move".into(),
            new_name: None,
            recursive: false,
            expected_version: None,
            acknowledged: true,
            destination_directory: Some(root.join("destination").to_string_lossy().into()),
            overwrite: false,
        };
        let preview = preview_local_browser_mutation(&request).unwrap();
        assert!(!preview.conflict);
        assert!(preview.command.contains("MoveFileExW") || preview.command.contains("renameat2"));
        request.expected_version = Some(preview.version);
        fs::write(root.join("destination/move.txt"), "keep").unwrap();
        assert!(
            execute_local_browser_mutation(&request).is_err(),
            "target appearing after preview must require a fresh confirmation"
        );
        assert_eq!(
            fs::read_to_string(root.join("destination/move.txt")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(root.join("source/move.txt")).unwrap(),
            "source"
        );
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        assert!(
            execute_local_browser_mutation(&request).is_err(),
            "existing target is never overwritten without consent"
        );
        assert_eq!(
            fs::read_to_string(root.join("destination/move.txt")).unwrap(),
            "keep"
        );
        request.overwrite = true;
        request.expected_version = Some(preview_local_browser_mutation(&request).unwrap().version);
        execute_local_browser_mutation(&request).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("destination/move.txt")).unwrap(),
            "source"
        );
        assert!(!root.join("source/move.txt").exists());
        fs::create_dir(root.join("nest")).unwrap();
        request.path = root.join("nest").to_string_lossy().into();
        request.action = "move".into();
        request.destination_directory = Some(root.join("nest/child").to_string_lossy().into());
        assert!(preview_local_browser_mutation(&request).is_err());
        let sibling = root.with_file_name(format!(
            "{}-root2",
            root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir(&sibling).unwrap();
        request.path = root.join("nest").to_string_lossy().into();
        request.destination_directory = Some(sibling.to_string_lossy().into());
        assert!(
            preview_local_browser_mutation(&request).is_err(),
            "a sibling prefix must not count as a destination inside the root"
        );
        fs::remove_dir(&sibling).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
