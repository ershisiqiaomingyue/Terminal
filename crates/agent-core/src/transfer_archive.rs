//! Bounded-memory archive packaging shared by desktop SSH transfers.
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::{
    fs,
    io::{self, Read},
    path::{Component, Path},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub type ArchiveProgress = Arc<dyn Fn(u64) + Send + Sync>;
pub fn safe_transfer_name(name: &str) -> bool {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !(stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

pub fn local_transfer_size(paths: &[String], cancel: &Arc<AtomicBool>) -> Result<u64, String> {
    let mut total = 0u64;
    for path in paths {
        visit(Path::new(path), cancel, &mut |path, metadata| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("文件名不是 UTF-8")?;
            if !safe_transfer_name(name) {
                return Err(format!("文件名不能用于跨平台传输：{name}"));
            }
            if metadata.is_file() {
                total = total.checked_add(metadata.len()).ok_or("文件总大小溢出")?;
            }
            Ok(())
        })?;
    }
    Ok(total)
}

fn visit(
    path: &Path,
    cancel: &Arc<AtomicBool>,
    action: &mut impl FnMut(&Path, &fs::Metadata) -> Result<(), String>,
) -> Result<(), String> {
    visit_depth(path, cancel, action, 0)
}
fn visit_depth(
    path: &Path,
    cancel: &Arc<AtomicBool>,
    action: &mut impl FnMut(&Path, &fs::Metadata) -> Result<(), String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 128 {
        return Err("目录层级超过 128 层，无法传输".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("传输已取消".into());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法访问 {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(format!(
            "请上传实际文件或目录，不支持特殊文件或目录链接：{}",
            path.display()
        ));
    }
    action(path, &metadata)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            visit_depth(
                &entry.map_err(|error| error.to_string())?.path(),
                cancel,
                action,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

struct ProgressReader<R> {
    reader: R,
    cancel: Arc<AtomicBool>,
    progress: ArchiveProgress,
    completed: u64,
    reported: Instant,
}
impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("传输已取消"));
        }
        let count = self.reader.read(buffer)?;
        self.completed += count as u64;
        if self.reported.elapsed() >= Duration::from_millis(200) {
            (self.progress)(self.completed);
            self.reported = Instant::now();
        }
        Ok(count)
    }
}

pub fn pack_transfer_archive(
    paths: &[String],
    archive: &Path,
    cancel: Arc<AtomicBool>,
    progress: ArchiveProgress,
) -> Result<(), String> {
    let output = fs::File::create(archive).map_err(|error| error.to_string())?;
    let mut builder = tar::Builder::new(GzEncoder::new(output, Compression::fast()));
    let mut total = 0;
    for source in paths {
        let source = Path::new(source);
        let parent = source.parent().ok_or("上传路径没有父目录")?;
        visit(source, &cancel, &mut |path, metadata| {
            let relative = path
                .strip_prefix(parent)
                .map_err(|error| error.to_string())?;
            if metadata.is_dir() {
                builder
                    .append_dir(relative, path)
                    .map_err(|error| error.to_string())?;
            } else {
                let reader = fs::File::open(path).map_err(|error| error.to_string())?;
                let mut monitored = ProgressReader {
                    reader,
                    cancel: cancel.clone(),
                    progress: progress.clone(),
                    completed: total,
                    reported: Instant::now(),
                };
                let mut header = tar::Header::new_gnu();
                header.set_metadata(metadata);
                header.set_size(metadata.len());
                header.set_cksum();
                builder
                    .append_data(&mut header, relative, &mut monitored)
                    .map_err(|error| error.to_string())?;
                total = monitored.completed;
                progress(total);
            }
            Ok(())
        })?;
    }
    builder
        .into_inner()
        .map_err(|error| error.to_string())?
        .finish()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn unpack_transfer_archive(
    archive: &Path,
    destination: &Path,
    cancel: Arc<AtomicBool>,
    progress: ArchiveProgress,
) -> Result<(), String> {
    let input = fs::File::open(archive).map_err(|error| error.to_string())?;
    let reader = ProgressReader {
        reader: GzDecoder::new(input),
        cancel: cancel.clone(),
        progress,
        completed: 0,
        reported: Instant::now(),
    };
    let mut archive = tar::Archive::new(reader);
    let mut names = std::collections::HashSet::new();
    for entry in archive.entries().map_err(|error| error.to_string())? {
        if cancel.load(Ordering::Relaxed) {
            return Err("传输已取消".into());
        }
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path().map_err(|error| error.to_string())?;
        if path.components().any(|part| !matches!(part, Component::Normal(name) if name.to_str().is_some_and(safe_transfer_name))) { return Err(format!("压缩包路径不能安全解压到 Windows：{}", path.display())); }
        if !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir()) {
            return Err("压缩包包含特殊文件，未解压".into());
        }
        if !names.insert(
            path.to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_lowercase(),
        ) {
            return Err(format!(
                "压缩包包含 Windows 下同名的路径：{}",
                path.display()
            ));
        }
        if !entry
            .unpack_in(destination)
            .map_err(|error| error.to_string())?
        {
            return Err("压缩包包含越界路径".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_names_reject_escape_and_device_names() {
        for name in ["..", "a/b", "a\\b", "C:x", "NUL.txt", "COM1", "a.", "a "] {
            assert!(!safe_transfer_name(name), "{name}");
        }
        for name in ["源码", "main.ts", "a b", "COM10"] {
            assert!(safe_transfer_name(name));
        }
    }
    #[test]
    fn archives_round_trip_nested_text_and_binary_without_buffering_whole_files() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-archive-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source/nested")).unwrap();
        fs::write(root.join("source/nested/你好.txt"), "text\n").unwrap();
        fs::write(root.join("source/data.bin"), [0, 255, 2]).unwrap();
        let paths = vec![root.join("source").to_string_lossy().into_owned()];
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(local_transfer_size(&paths, &cancel).unwrap(), 8);
        pack_transfer_archive(
            &paths,
            &root.join("archive.tgz"),
            cancel.clone(),
            Arc::new(|_| {}),
        )
        .unwrap();
        fs::create_dir(root.join("out")).unwrap();
        unpack_transfer_archive(
            &root.join("archive.tgz"),
            &root.join("out"),
            cancel,
            Arc::new(|_| {}),
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("out/source/data.bin")).unwrap(),
            [0, 255, 2]
        );
        assert_eq!(
            fs::read_to_string(root.join("out/source/nested/你好.txt")).unwrap(),
            "text\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn extraction_rejects_links_case_collisions_and_cancellation() {
        let root = std::env::temp_dir().join(format!(
            "android-tool-archive-safety-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("out")).unwrap();
        let file = root.join("archive.tgz");
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut builder = tar::Builder::new(GzEncoder::new(
                fs::File::create(&file).unwrap(),
                Compression::fast(),
            ));
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("../escape").unwrap();
            header.set_cksum();
            builder
                .append_data(&mut header, "link", io::empty())
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        assert!(unpack_transfer_archive(
            &file,
            &root.join("out"),
            cancel.clone(),
            Arc::new(|_| {})
        )
        .unwrap_err()
        .contains("特殊文件"));
        {
            let mut builder = tar::Builder::new(GzEncoder::new(
                fs::File::create(&file).unwrap(),
                Compression::fast(),
            ));
            for name in ["A.txt", "a.txt"] {
                let mut header = tar::Header::new_gnu();
                header.set_size(1);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, &b"x"[..]).unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        assert!(unpack_transfer_archive(
            &file,
            &root.join("out"),
            cancel.clone(),
            Arc::new(|_| {})
        )
        .unwrap_err()
        .contains("同名"));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            unpack_transfer_archive(&file, &root.join("out"), cancel, Arc::new(|_| {}))
                .unwrap_err()
                .contains("取消")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
