//! Local distributable packages. Extraction never runs package scripts or npm.
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

pub(super) struct Package {
    _temporary: tempfile::TempDir,
    pub root: PathBuf,
}

fn safe_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.to_string_lossy().contains(['\\', ':'])
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err("插件压缩包包含越界路径".into());
    }
    Ok(())
}

fn budget(total: &mut u64, size: u64, index: usize) -> Result<(), String> {
    *total = total.checked_add(size).ok_or("插件压缩包过大")?;
    if *total > MAX_BYTES || index >= MAX_ENTRIES {
        return Err("插件压缩包超过解压大小或文件数量限制".into());
    }
    Ok(())
}

fn copy_file(reader: &mut impl io::Read, path: &Path, size: u64) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("插件文件路径无效")?).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let written =
        io::copy(&mut io::Read::take(reader, size + 1), &mut file).map_err(|e| e.to_string())?;
    if written != size {
        return Err("插件压缩包文件长度无效".into());
    }
    Ok(())
}

fn package_root(path: &Path) -> Result<PathBuf, String> {
    let mut root = path.to_path_buf();
    for _ in 0..5 {
        if root.join("package.json").is_file() {
            return Ok(root);
        }
        let entries = fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if entries.len() != 1 || !entries[0].path().is_dir() {
            break;
        }
        root = entries[0].path();
    }
    Err("插件压缩包中找不到 package.json，请选择插件发布包".into())
}

impl Package {
    pub fn open(path: &Path) -> Result<Self, String> {
        let temporary = tempfile::Builder::new()
            .prefix("android-tool-plugin-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let name = path.to_string_lossy().to_ascii_lowercase();
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut total = 0;
        if name.ends_with(".tgz") || name.ends_with(".tar.gz") {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            for (index, entry) in archive.entries().map_err(|e| e.to_string())?.enumerate() {
                let mut entry = entry.map_err(|e| e.to_string())?;
                let relative = entry.path().map_err(|e| e.to_string())?.into_owned();
                safe_path(&relative)?;
                budget(&mut total, entry.size(), index)?;
                let output = temporary.path().join(relative);
                if entry.header().entry_type().is_dir() {
                    fs::create_dir_all(output).map_err(|e| e.to_string())?;
                } else if entry.header().entry_type().is_file() {
                    let size = entry.size();
                    copy_file(&mut entry, &output, size)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(
                            &output,
                            fs::Permissions::from_mode(
                                entry.header().mode().map_err(|e| e.to_string())? & 0o777,
                            ),
                        )
                        .map_err(|e| e.to_string())?;
                    }
                } else {
                    return Err("插件压缩包不支持链接或特殊文件".into());
                }
            }
        } else if name.ends_with(".zip") {
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
                let relative = Path::new(entry.name());
                safe_path(relative)?;
                budget(&mut total, entry.size(), index)?;
                let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
                if mode != 0 && mode != 0o100000 && mode != 0o040000 {
                    return Err("插件压缩包不支持链接或特殊文件".into());
                }
                let output = temporary.path().join(relative);
                if entry.is_dir() {
                    fs::create_dir_all(output).map_err(|e| e.to_string())?;
                } else {
                    let size = entry.size();
                    copy_file(&mut entry, &output, size)?;
                    #[cfg(unix)]
                    if let Some(mode) = entry.unix_mode() {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&output, fs::Permissions::from_mode(mode & 0o777))
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        } else {
            return Err("请选择 .tgz、.tar.gz 或 .zip 插件压缩包".into());
        }
        let root = package_root(temporary.path())?;
        let pkg: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("package.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if pkg.get("dsh").is_none() && pkg.get("androidTool").is_none() {
            return Err("该包未声明插件入口（dsh / androidTool）".into());
        }
        Ok(Self {
            _temporary: temporary,
            root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const MANIFEST: &[u8] = br#"{"name":"example-plugin","version":"1.0.0","androidTool":{}}"#;

    fn zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn npm_tgz_extracts_built_files_and_cleans_up() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("plugin.tgz");
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        for (name, bytes) in [
            ("package/package.json", MANIFEST),
            ("package/dist/client.js", b"built client".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        let package = Package::open(&path).unwrap();
        let extracted = package.root.clone();
        assert_eq!(
            fs::read(extracted.join("dist/client.js")).unwrap(),
            b"built client"
        );
        drop(package);
        assert!(!extracted.exists());
        assert!(path.exists());
    }

    #[test]
    fn zip_accepts_root_or_one_wrapping_directory() {
        let fixture = tempfile::tempdir().unwrap();
        for prefix in ["", "example-plugin/"] {
            let path = fixture.path().join("plugin.zip");
            zip(&path, &[(&format!("{prefix}package.json"), MANIFEST)]);
            assert_eq!(
                fs::read(Package::open(&path).unwrap().root.join("package.json")).unwrap(),
                MANIFEST
            );
        }
    }

    #[test]
    fn zip_rejects_paths_outside_destination_and_non_plugins() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("plugin.zip");
        for name in [
            "../outside.json",
            "/outside.json",
            "C:/outside.json",
            "package/..\\outside.json",
        ] {
            zip(&path, &[(name, MANIFEST)]);
            assert!(Package::open(&path).err().unwrap().contains("越界路径"));
        }
        zip(
            &path,
            &[("package.json", b"{\"name\":\"ordinary-package\"}")],
        );
        assert!(Package::open(&path)
            .err()
            .unwrap()
            .contains("未声明插件入口"));
        zip(&path, &[("README.md", b"no manifest")]);
        assert!(Package::open(&path)
            .err()
            .unwrap()
            .contains("找不到 package.json"));
    }

    #[test]
    fn tar_links_and_extraction_limits_are_rejected() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("plugin.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder
            .append_link(&mut header, "package/link", "../../outside")
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(Package::open(&path).err().unwrap().contains("不支持链接"));
        assert!(budget(&mut 0, MAX_BYTES + 1, 0).is_err());
        assert!(budget(&mut 0, 0, MAX_ENTRIES).is_err());
    }
}
