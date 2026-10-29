//! User-owned assets shared by development builds, installed builds and windows.
use serde_json::Value;
use std::{fs, path::{Path, PathBuf}, sync::OnceLock};
use tauri::{AppHandle, Manager};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 本项目用户目录为 `.terminal`（更早的 `.android-tool` 只在迁移时出现）。
pub(crate) const USER_DIR: &str = ".terminal";
const LEGACY_USER_DIR: &str = ".android-tool";

pub(crate) fn root() -> Result<PathBuf, String> {
    if let Some(root) = ROOT.get() { return Ok(root.clone()); }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.is_empty()).map(PathBuf::from)
        .ok_or("无法定位用户目录，不能保存 Android Toolkit 数据")?;
    Ok(home.join(USER_DIR))
}

pub(crate) fn directory(name: &str) -> Result<PathBuf, String> {
    let path = root()?.join(name);
    fs::create_dir_all(&path).map_err(|e| format!("无法创建用户数据目录 {}：{e}", path.display()))?;
    Ok(path)
}

pub(crate) fn initialize(app: &AppHandle) -> Result<(), String> {
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    let root = home.join(USER_DIR);
    let _ = ROOT.set(root.clone());
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let config = app.path().app_config_dir().map_err(|e| e.to_string())?;
    // 这是 v1 遗留目录名（迁移来源），必须保持历史拼写，否则老用户已下载的
    // 工具再也定位不到。品牌名改成 Android Toolkit 后这里不能跟着改。
    let tools = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .map(|path| path.join("Android Tool/tools"));
    // 先补上旧用户目录的内容：`migrate` 与下面的资源播种都以“新根里有啥”
    // 为判据，顺序反了会让已存在的老配置被当成全新安装。
    let legacy_root = home.join(LEGACY_USER_DIR);
    if legacy_root.is_dir() { copy_missing_tree(&legacy_root, &root)?; }
    migrate(&root, &data, &config, tools.as_deref())?;
    cleanup_legacy_app_data(&root, std::env::var_os("LOCALAPPDATA").map(PathBuf::from).as_deref());
    let mut candidates = Vec::new();
    #[cfg(debug_assertions)]
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/agent-instructions"));
    if let Ok(resources) = app.path().resource_dir() {
        candidates.push(resources.join("resources/agent-instructions"));
        candidates.push(resources.join("agent-instructions"));
    }
    if let Some(source) = candidates.into_iter().find(|path| path.join("AGENTS.md").is_file()) {
        initialize_agent_instructions(&root, &source)?;
        initialize_android_tool_instructions(&root, &source.join("ANDROID_TOOL.md"))?;
        initialize_development_resources(&root, &source.parent().unwrap().join("development"))?;
    }
    Ok(())
}

/// Seed the Android Toolkit-specific global context once for both fresh installs
/// and upgrades. Keep the file user-owned after the first copy.
fn initialize_android_tool_instructions(root: &Path, source: &Path) -> Result<(), String> {
    let marker = root.join(".android-tool-instructions-v1.done");
    if marker.is_file() || !source.is_file() { return Ok(()); }
    copy_missing_tree(source, &root.join("ANDROID_TOOL.md"))?;
    atomic_write(&marker, b"1\n")
}

/// New resources have their own once-only migration. Existing edited files and
/// deliberately deleted v2 domain guidance are not replaced or resurrected.
fn initialize_development_resources(root: &Path, resources: &Path) -> Result<(), String> {
    let marker = root.join(".development-resources-v1.done");
    if marker.is_file() || !resources.join("lsp-servers.json").is_file() { return Ok(()); }
    copy_missing_tree(&resources.join("skills"), &root.join("skills/development"))?;
    copy_missing_tree(&resources.join("lsp-servers.json"), &root.join("tools/lsp-servers.json"))?;
    atomic_write(&marker, b"1\n")
}

/// Split only our unmodified v1 default. Custom global instructions remain the
/// user's explicit choice; domain workflows are editable, optional Skills.
fn initialize_agent_instructions(root: &Path, resources: &Path) -> Result<(), String> {
    let marker = root.join(".agent-instructions-v2.done");
    if marker.is_file() { return Ok(()); }
    copy_missing_tree(&resources.join("skills"), &root.join("skills/guidance"))?;
    let target = root.join("AGENTS.md");
    let previous = fs::read_to_string(&target).ok();
    let legacy = fs::read_to_string(resources.join("legacy/AGENTS.v1.md")).ok();
    let normalize = |text: &str| text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if previous.as_ref().zip(legacy.as_ref())
        .is_some_and(|(current, original)| normalize(current) == normalize(original))
    {
        copy_missing_tree(&target, &root.join("AGENTS.md.v1.bak"))?;
        let next = fs::read(resources.join("AGENTS.md")).map_err(|e| e.to_string())?;
        atomic_write(&target, &next)?;
    }
    // The v1 marker also preserves a deliberate deletion of global preferences.
    seed_agent_instructions(root, &resources.join("AGENTS.md"))?;
    atomic_write(&marker, b"2\n")
}

/// Copy an editable resource once. Never replace custom rules or restore a
/// file the user removed after initialization. No compiled prompt fallback.
fn seed_agent_instructions(root: &Path, source: &Path) -> Result<(), String> {
    let marker = root.join(".agent-instructions-v1.done");
    if marker.is_file() { return Ok(()); }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    if agent_core::load_global_agent_instructions(root).is_empty() {
        let target = root.join("AGENTS.md");
        if !target.exists() { copy_missing_tree(source, &target)?; }
    }
    atomic_write(&marker, b"1\n")
}

fn migrate(root: &Path, data: &Path, config: &Path, old_tools: Option<&Path>) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let marker = root.join(".storage-migration-v1.done");
    if marker.is_file() { return Ok(()); }
    let mut mappings = vec![(data.join("skills"), root.join("skills"))];
    copy_missing_tree(&data.join("model-plugins"), &root.join("plugins"))?;
    copy_missing_tree(&data.join("skills"), &root.join("skills"))?;
    copy_missing_tree(&config.join("agent-skills.json"), &root.join("skills/settings.json"))?;
    copy_missing_tree(&config.join("android-selinux-sources.json"), &root.join("skills/selinux-sources.json"))?;
    if let Some(old_tools) = old_tools {
        let new_tools = root.join("tools");
        copy_missing_tree(old_tools, &new_tools)?;
        mappings.push((old_tools.to_owned(), new_tools));
    }
    relocate_in_root(root, &mappings)?;
    atomic_write(&marker, b"1\n")
}

/// 把配置文件里记录的旧绝对路径改写成新位置。只动我们自己那两个文件：
/// `skills/settings.json`（停用列表）与 `tools/environment-tools.json`（工具根）。
pub(crate) fn relocate_config_paths(mappings: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    relocate_in_root(&root()?, mappings)
}

fn relocate_in_root(root: &Path, mappings: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    if mappings.is_empty() { return Ok(()); }
    for path in [root.join("skills/settings.json"), root.join("tools/environment-tools.json")] {
        if !path.is_file() { continue; }
        let mut value: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("旧配置不能迁移 {}：{e}", path.display()))?;
        if relocate_values(&mut value, mappings) {
            atomic_write(&path, &serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?)?;
        }
    }
    Ok(())
}

/// 最早那版 identifier 是 `com.androidtool.app`。那份 AppData 里只剩 WebView2
/// 的浏览器缓存（几十 MB），用户数据早就搬到了后来的目录，留着只会让人以为
/// 迁移没做完。（`com.androidtool.desktop` → `com.androidtoolkit.desktop` 的搬家
/// 由 [`crate::appdata_migration`] 负责，不在这里。）
///
/// 只删「目录里除了 EBWebView 什么也没有」的旧目录：一旦发现别的文件就整体
/// 跳过——宁可留着占空间，也不能误删用户自己的东西。删不干净就不写 marker，
/// 下次启动再试。
fn cleanup_legacy_app_data(root: &Path, local: Option<&Path>) {
    const LEGACY: [&str; 1] = ["com.androidtool.app"];
    let marker = root.join(".legacy-appdata-cleanup-v1.done");
    if marker.is_file() {
        return;
    }
    let Some(local) = local else { return };
    let mut done = true;
    for name in LEGACY {
        let dir = local.join(name);
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        let only_webview_cache = entries
            .filter_map(Result::ok)
            .all(|entry| entry.file_name() == "EBWebView");
        if only_webview_cache && fs::remove_dir_all(&dir).is_err() {
            done = false;
        }
    }
    if done {
        let _ = atomic_write(&marker, b"1\n");
    }
}

/// Copy without consuming the original or replacing a user's newer data.
pub(crate) fn copy_missing_tree(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(value) => value,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("无法读取旧数据 {}：{e}", source.display())),
    };
    // NPM executable links aren't needed by our direct Node adapters. Never
    // follow legacy links and accidentally migrate unrelated user directories.
    if metadata.file_type().is_symlink() { return Ok(()); }
    if fs::symlink_metadata(target).is_ok_and(|value| value.file_type().is_symlink()) {
        return Err(format!("迁移目标不能是符号链接：{}", target.display()));
    }
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_missing_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if metadata.is_file() && !target.exists() {
        let parent = target.parent().ok_or("迁移目标目录无效")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = temporary_path(target);
        fs::copy(source, &temporary).map_err(|e| format!("迁移 {} 失败：{e}", source.display()))?;
        let result = fs::hard_link(&temporary, target);
        let _ = fs::remove_file(&temporary);
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::AlreadyExists { return Err(error.to_string()); }
        }
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(".{}.{}-{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()))
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let temporary = temporary_path(path);
    let result = (|| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = fs::remove_file(temporary); }
    result
}

/// Bundle versions are immutable, so a new install can't overwrite tools being
/// used by another window/process. Development builds can use the active copy.
pub(crate) fn cache_bundle(source: &Path, cache: &Path, identity: &[u8]) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let version = Sha256::digest(identity).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let target = cache.join(&version);
    if !target.join(".complete").is_file() {
        copy_missing_tree(source, &target)?;
        atomic_write(&target.join(".complete"), b"1\n")?;
    }
    atomic_write(&cache.join("active-version.json"), &serde_json::to_vec(&version).map_err(|e| e.to_string())?)?;
    Ok(target)
}

pub(crate) fn active_bundle(cache: &Path) -> Option<PathBuf> {
    let version: String = serde_json::from_slice(&fs::read(cache.join("active-version.json")).ok()?).ok()?;
    if version.len() != 64 || !version.bytes().all(|byte| byte.is_ascii_hexdigit()) { return None; }
    let path = cache.join(version);
    path.join(".complete").is_file().then_some(path)
}

#[cfg(test)]
pub(crate) fn directory_fingerprint(root: &Path) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    fn visit(root: &Path, path: &Path, hash: &mut Sha256) -> Result<(), String> {
        let mut entries = fs::read_dir(path).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() { continue; }
            if kind.is_dir() { visit(root, &entry.path(), hash)?; }
            else if kind.is_file() {
                hash.update(entry.path().strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().as_bytes());
                hash.update([0]);
                let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    visit(root, root, &mut hash)?;
    Ok(hash.finalize().to_vec())
}

fn relocate_values(value: &mut Value, mappings: &[(PathBuf, PathBuf)]) -> bool {
    match value {
        Value::String(text) => {
            // Disabled Skill identities contain a serialized (repository, path).
            if let Ok(mut identity @ Value::Array(_)) = serde_json::from_str::<Value>(text) {
                if relocate_values(&mut identity, mappings) { *text = identity.to_string(); return true; }
            }
            let normalized = text.trim_start_matches(r"\\?\").replace('\\', "/");
            for (old, new) in mappings {
                let prefix = old.to_string_lossy().replace('\\', "/");
                let matches = if cfg!(windows) { normalized.to_lowercase().starts_with(&prefix.to_lowercase()) } else { normalized.starts_with(&prefix) };
                if matches && (normalized.len() == prefix.len() || normalized.as_bytes().get(prefix.len()) == Some(&b'/')) {
                    *text = new.join(normalized[prefix.len()..].trim_start_matches('/')).to_string_lossy().into_owned();
                    return true;
                }
            }
            false
        }
        Value::Array(values) => values.iter_mut().fold(false, |changed, value| relocate_values(value, mappings) || changed),
        Value::Object(values) => values.values_mut().fold(false, |changed, value| relocate_values(value, mappings) || changed),
        _ => false,
    }
}

#[cfg(test)]
#[path = "user_storage_tests.rs"]
mod tests;
