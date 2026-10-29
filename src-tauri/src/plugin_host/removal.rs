use std::{fs, io, path::Path};

fn already_removed(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Remove app-owned copies without following npm symlinks or directory junctions.
/// A previous partial uninstall or a concurrently removed entry is harmless.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let linked = metadata.file_type().is_symlink();
    if linked {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x10 != 0 {
                return already_removed(fs::remove_dir(path));
            }
        }
        return already_removed(fs::remove_file(path));
    }
    if metadata.is_dir() {
        match fs::read_dir(path) {
            Ok(entries) => {
                let mut entries = entries.collect::<io::Result<Vec<_>>>()?;
                // Keep the registry until all installation files are removed so a
                // failed cleanup remains visible and can be retried in the UI.
                entries.sort_by_key(|entry| entry.file_name() == "plugin.json");
                for entry in entries {
                    remove_tree(&entry.path())?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        already_removed(fs::remove_dir(path))
    } else {
        already_removed(fs::remove_file(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn locked_files_keep_the_plugin_visible_until_cleanup_can_finish() {
        use std::os::windows::fs::OpenOptionsExt;
        let base =
            std::env::temp_dir().join(format!("plugin-removal-locked-{}", std::process::id()));
        fs::create_dir_all(base.join("versions")).unwrap();
        fs::write(base.join("plugin.json"), "registry").unwrap();
        fs::write(base.join("versions/locked.js"), "copy").unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(base.join("versions/locked.js"))
            .unwrap();
        assert!(remove_tree(&base).is_err());
        assert!(base.join("plugin.json").is_file());
        drop(locked);
        remove_tree(&base).unwrap();
        assert!(!base.exists());
    }
    #[test]
    fn partially_removed_plugin_and_dangling_links_are_safe_to_retry() {
        let base = std::env::temp_dir().join(format!("plugin-removal-{}", std::process::id()));
        let plugin = base.join("plugin");
        let original = base.join("original");
        fs::create_dir_all(plugin.join("versions/old")).unwrap();
        fs::create_dir_all(&original).unwrap();
        fs::write(original.join("keep.txt"), "original").unwrap();
        fs::write(plugin.join("versions/old/index.js"), "copy").unwrap();
        #[cfg(windows)]
        {
            // Junctions need no developer-mode privilege, unlike Windows symlinks.
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(plugin.join("original-link"))
                .arg(&original)
                .output()
                .unwrap();
            assert!(status.status.success());
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(plugin.join("missing-link"))
                .arg(base.join("missing"))
                .output()
                .unwrap();
            assert!(status.status.success());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&original, plugin.join("original-link")).unwrap();
            std::os::unix::fs::symlink(base.join("missing"), plugin.join("missing-link")).unwrap();
        }
        remove_tree(&plugin).unwrap();
        remove_tree(&plugin).unwrap();
        assert_eq!(
            fs::read_to_string(original.join("keep.txt")).unwrap(),
            "original"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
