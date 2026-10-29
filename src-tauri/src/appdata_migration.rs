//! 把旧 identifier 的数据目录搬到新 identifier 下（一次性、可重复执行）。
//!
//! 旧版虽然产品名叫 AndroidToolkit，但 identifier 是 `com.androidtool.desktop`，
//! 用户能在资源管理器、设置页「打开日志目录」里直接看到这个名字，于是改成
//! `com.androidtoolkit.desktop`。数据必须跟着搬，否则历史对话、provider 配置、
//! SSH 连接都会「凭空消失」。
//!
//! 规则：
//! 1. 先 `rename`（同盘瞬时完成，不动一个字节）。
//! 2. 改名被拒（老实例还开着、文件被占用）时退化成复制，跳过纯缓存的 WebView 目录。
//! 3. 新目录已经存在就什么都不做——绝不把两边的数据合并。

use crate::app_identity;
use std::{fs, io, path::PathBuf};

/// 远古 identifier（正式版 / 开发版）。
///
/// 注意：**故意不**把 `com.androidtoolkit.desktop` 列为迁移源——那是 Android-Tool
/// 正在使用的数据目录，搬走会让那边丢失配置。这里只处理更早的 `com.androidtool.desktop`。
const LEGACY_IDENTIFIER: &str = "com.androidtool.desktop";
const LEGACY_DEV_IDENTIFIER: &str = "com.androidtool.desktop.dev";

/// 把 `APPDATA` 与 `LOCALAPPDATA` 下的旧目录迁移到 `identifier` 对应位置。
pub fn migrate(identifier: &str) -> Result<(), String> {
    let legacy = legacy_identifier(identifier);
    if legacy == identifier {
        return Ok(());
    }
    let mut failures = Vec::new();
    for variable in ["APPDATA", "LOCALAPPDATA"] {
        let Some(base) = std::env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let (old, new) = (base.join(&legacy), base.join(identifier));
        if let Err(error) = move_directory(&old, &new) {
            failures.push(format!("{variable}\\{legacy} -> {identifier}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("；"))
    }
}

fn legacy_identifier(current: &str) -> String {
    let legacy = if current == app_identity::DEV_IDENTIFIER {
        LEGACY_DEV_IDENTIFIER
    } else {
        LEGACY_IDENTIFIER
    };
    legacy.to_string()
}

/// 新目录已存在（搬过了 / 全新安装）时不碰旧目录，避免把两份数据搅在一起。
fn move_directory(old: &PathBuf, new: &PathBuf) -> Result<(), String> {
    if new.exists() || !old.is_dir() {
        return Ok(());
    }
    if fs::rename(old, new).is_ok() {
        return Ok(());
    }
    // 老实例占着文件时 Windows 会拒绝改名：退化成复制。复制完再尽力删掉旧目录，
    // 删不掉也无所谓——新目录已经齐全，下次启动会走上面的早退。
    copy_without_cache(old, new).map_err(|error| format!("复制失败：{error}"))?;
    let _ = fs::remove_dir_all(old);
    Ok(())
}

/// 复制整个目录，但跳过 WebView 的磁盘缓存（几百兆、重建即成，没必要搬）。
fn copy_without_cache(source: &PathBuf, target: &PathBuf) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_webview_cache(&name.to_string_lossy()) {
            continue;
        }
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_without_cache(&from, &to)?;
        } else if !to.exists() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn is_webview_cache(name: &str) -> bool {
    name == "EBWebView" || name.starts_with("webview-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("android-data-{name}-{stamp}"));
        fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    #[test]
    fn rename_moves_the_directory_and_never_merges() {
        let base = scratch("rename");
        let old = base.join(LEGACY_IDENTIFIER);
        fs::create_dir_all(old.join("logs")).expect("mkdir");
        fs::write(old.join("agent-history.sqlite3"), b"history").expect("write");
        let new = base.join(app_identity::IDENTIFIER);

        move_directory(&old, &new).expect("migrate");
        assert!(!old.exists(), "旧目录应当已经不在原地");
        assert_eq!(
            fs::read(new.join("agent-history.sqlite3")).expect("read"),
            b"history"
        );
        assert!(new.join("logs").is_dir(), "子目录要一起搬过来");

        // 第二次执行（或旧客户端又把目录建回来）时，新目录已存在 → 一个字都不动。
        fs::create_dir_all(&old).expect("recreate");
        fs::write(old.join("keep.txt"), b"keep").expect("write");
        move_directory(&old, &new).expect("second migrate");
        assert!(old.join("keep.txt").is_file(), "旧目录必须原样保留");
    }

    #[test]
    fn copy_fallback_skips_webview_caches_but_keeps_data() {
        let base = scratch("copy");
        let old = base.join("old");
        fs::create_dir_all(old.join("logs")).expect("mkdir logs");
        fs::create_dir_all(old.join("EBWebView").join("Default")).expect("mkdir edge");
        fs::create_dir_all(old.join("webview-dev")).expect("mkdir webview");
        fs::write(old.join("logs").join("run.json"), b"log").expect("write log");
        fs::write(old.join("EBWebView").join("Default").join("cache"), b"cache").expect("write cache");
        let new = base.join("new");

        copy_without_cache(&old, &new).expect("copy");
        assert!(new.join("logs").join("run.json").is_file());
        assert!(!new.join("EBWebView").exists(), "WebView 缓存不该被搬");
        assert!(!new.join("webview-dev").exists(), "WebView 缓存不该被搬");
    }

    #[test]
    fn legacy_identifier_follows_the_current_one() {
        assert_eq!(legacy_identifier(app_identity::IDENTIFIER), LEGACY_IDENTIFIER);
        assert_eq!(
            legacy_identifier(app_identity::DEV_IDENTIFIER),
            LEGACY_DEV_IDENTIFIER
        );
    }
}
