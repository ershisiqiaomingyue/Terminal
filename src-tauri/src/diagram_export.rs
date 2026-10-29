use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

fn save_svg(directory: &Path, svg: &str) -> Result<PathBuf, String> {
    if svg.trim().is_empty() || !svg.contains("<svg") || !svg.contains("</svg>") {
        return Err("没有可下载的图表".into());
    }
    fs::create_dir_all(directory).map_err(|error| format!("无法访问系统下载目录：{error}"))?;
    for index in 0..10_000 {
        let name = if index == 0 {
            "mermaid.svg".into()
        } else {
            format!("mermaid ({index}).svg")
        };
        let path = directory.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(svg.as_bytes()).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(format!("保存图表失败：{error}"));
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("保存图表失败：{error}")),
        }
    }
    Err("下载目录中的同名图表过多，请整理后重试".into())
}

#[tauri::command]
pub async fn download_mermaid_svg(app: AppHandle, svg: String) -> Result<String, String> {
    let directory = app
        .path()
        .download_dir()
        .map_err(|error| format!("无法获取系统下载目录：{error}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        save_svg(&directory, &svg).map(|path| path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| format!("下载图表失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);
    struct DownloadDirectory(PathBuf);
    impl DownloadDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "android-tool-diagram-{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for DownloadDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saves_svg_and_preserves_existing_downloads() {
        let directory = DownloadDirectory::new();
        let first = "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>第一张图</text></svg>";
        let second = "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>第二张图</text></svg>";
        let first_path = save_svg(&directory.0, first).unwrap();
        let second_path = save_svg(&directory.0, second).unwrap();
        assert_eq!(first_path, directory.0.join("mermaid.svg"));
        assert_eq!(second_path, directory.0.join("mermaid (1).svg"));
        assert_eq!(fs::read_to_string(first_path).unwrap(), first);
        assert_eq!(fs::read_to_string(second_path).unwrap(), second);
    }

    #[test]
    fn empty_or_missing_svg_does_not_create_a_download() {
        let directory = DownloadDirectory::new();
        assert!(save_svg(&directory.0, "").is_err());
        assert!(save_svg(&directory.0, "flowchart TD\nA --> B").is_err());
        assert!(!directory.0.exists());
    }

    #[test]
    fn directory_errors_are_reported_without_overwriting_files() {
        let directory = DownloadDirectory::new();
        fs::write(&directory.0, "existing file").unwrap();
        assert!(save_svg(&directory.0, "<svg></svg>").is_err());
        assert_eq!(fs::read_to_string(&directory.0).unwrap(), "existing file");
        fs::remove_file(&directory.0).unwrap();
    }
}
