use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose::STANDARD, Engine as _};

/// Publish an immutable object atomically. Names and upload order do not affect
/// identity, and concurrent uploads can never expose a partially written file.
pub(crate) fn store_object(root: &Path, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    fs::create_dir_all(root).map_err(|error| format!("无法创建附件目录: {error}"))?;
    let path = root.join(format!("{:x}", Sha256::digest(bytes)));
    if path.is_file() {
        // Renew the orphan grace period when an old object is attached again.
        fs::OpenOptions::new().write(true).open(&path).and_then(|file| file.set_modified(SystemTime::now()))
            .map_err(|error| format!("无法更新附件引用: {error}"))?;
        return Ok(path);
    }
    let mut pending = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    pending.write_all(bytes).map_err(|error| format!("无法保存附件: {error}"))?;
    pending.as_file().sync_all().map_err(|error| error.to_string())?;
    match pending.persist_noclobber(&path) {
        Ok(_) => Ok(path),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(error) => Err(format!("无法保存附件: {}", error.error)),
    }
}

pub(crate) fn resolve_image(root: &Path, image: &agent_core::AgentImageAttachment)
    -> Result<agent_core::AgentImageAttachment, String>
{
    let Some(path) = image.path.as_deref() else { return Ok(image.clone()); };
    if attachment_name(root, path).is_none() {
        return Err(format!("图片“{}”的缓存引用无效，请重新添加", image.name));
    }
    let path = fs::canonicalize(path).map_err(|error| format!("无法读取图片“{}”: {error}", image.name))?;
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    if path.parent() != Some(canonical_root.as_path()) {
        return Err("图片缓存路径越界".into());
    }
    let bytes = read_bounded_image(&path)?;
    Ok(agent_core::AgentImageAttachment {
        data_url: format!("data:{};base64,{}", image.media_type, STANDARD.encode(bytes)),
        path: None,
        ..image.clone()
    })
}

pub(crate) fn read_bounded_image(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let limit = (agent_core::MAX_AGENT_IMAGE_DATA_URL_CHARS.saturating_sub(128) / 4 * 3) as u64;
    let file = fs::File::open(path).map_err(|error| format!("无法读取图片: {error}"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit { return Err("图片超过模型附件大小限制，请缩小图片后重试".into()); }
    Ok(bytes)
}

/// Migrate inline images when a legacy session is next saved. This keeps old
/// sessions readable without a startup scan or rewriting every conversation.
pub(crate) fn externalize_images(root: &Path, value: &mut serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values { externalize_images(root, value)?; }
        }
        serde_json::Value::Object(object) => {
            let inline = object.get("dataUrl").and_then(serde_json::Value::as_str)
                .filter(|url| url.starts_with("data:image/"))
                .and_then(|url| url.split_once(";base64,"))
                .map(|(header, data)| (header.trim_start_matches("data:").to_string(), data.to_string()));
            if let Some((media_type, data)) = inline {
                // Leave malformed legacy payloads readable; normalization will
                // still reject them when explicitly submitted to a model.
                if let Ok(bytes) = STANDARD.decode(&data) {
                    let path = store_object(root, &bytes)?.to_string_lossy().into_owned();
                    let key = if object.contains_key("kind") { "imagePath" } else { "path" };
                    object.insert(key.into(), path.into());
                    object.insert("mediaType".into(), media_type.into());
                    object.remove("dataUrl");
                }
            }
            for value in object.values_mut() { externalize_images(root, value)?; }
        }
        _ => {}
    }
    Ok(())
}

/// Exports remain portable: cache paths are an internal persistence format.
pub(crate) fn inline_export_images(root: &Path, value: &mut serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values { inline_export_images(root, value)?; }
        }
        serde_json::Value::Object(object) => {
            let reference = object.get("imagePath").or_else(|| object.get("path"))
                .and_then(serde_json::Value::as_str)
                .filter(|path| attachment_name(root, path).is_some()).map(str::to_string);
            let media_type = object.get("mediaType").and_then(serde_json::Value::as_str)
                .filter(|mime| mime.starts_with("image/")).map(str::to_string);
            if let (Some(path), Some(media_type)) = (reference, media_type) {
                let image = agent_core::AgentImageAttachment {
                    name: object.get("name").and_then(serde_json::Value::as_str).unwrap_or("image").into(),
                    media_type, data_url: String::new(), path: Some(path.clone()),
                };
                object.insert("dataUrl".into(), resolve_image(root, &image)?.data_url.into());
                object.remove("imagePath");
                if object.get("path").and_then(serde_json::Value::as_str) == Some(path.as_str()) {
                    object.remove("path");
                }
            }
            for value in object.values_mut() { inline_export_images(root, value)?; }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) const ORPHAN_ATTACHMENT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) fn attachment_name(root: &Path, value: &str) -> Option<String> {
    let root = path_key(&root.to_string_lossy());
    let value = path_key(value);
    let name = value.strip_prefix(&format!("{}/", root.trim_end_matches('/')))?;
    if name.is_empty() || name.contains('/') || matches!(name, "." | "..") {
        return None;
    }
    Some(name.to_string())
}

fn path_key(value: &str) -> String {
    let value = value.replace('\\', "/");
    #[cfg(target_os = "windows")]
    let value = value.to_lowercase();
    value
}

pub(crate) fn prune_attachments(
    root: &Path,
    deleted_references: &HashSet<String>,
    live_references: &HashSet<String>,
    now: SystemTime,
) -> Result<usize, String> {
    if !root.exists() {
        return Ok(0);
    }
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("附件缓存目录不是普通目录，未执行清理".into());
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let mut removed = 0;
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let Some(name) = attachment_name(root, &path.to_string_lossy()) else {
            continue;
        };
        if live_references.contains(&name) {
            continue;
        }
        // Fresh, unowned uploads may still be preparing a request. Only exact
        // deleted references bypass the orphan grace period.
        let old_orphan = metadata.modified().ok().and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ORPHAN_ATTACHMENT_GRACE);
        // Content objects can be shared with a just-added, unsaved draft.
        // Keep their grace period even when the last durable owner is deleted.
        let content_object = name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit());
        if (!deleted_references.contains(&name) || content_object) && !old_orphan {
            continue;
        }
        let canonical_path = fs::canonicalize(&path).map_err(|error| error.to_string())?;
        if canonical_path.parent() != Some(canonical_root.as_path()) {
            return Err("附件路径越过缓存目录，未执行清理".into());
        }
        fs::remove_file(&path).map_err(|error| format!("无法清理附件缓存: {error}"))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_objects_are_shared_atomic_and_restorable_after_reopening() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("agent-attachments");
        let paths = std::thread::scope(|scope| {
            let jobs = (0..6).map(|_| scope.spawn(|| store_object(&root, b"same image").unwrap())).collect::<Vec<_>>();
            jobs.into_iter().map(|job| job.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(paths.iter().all(|path| path == &paths[0]));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(fs::read(&paths[0]).unwrap(), b"same image");
        let image = agent_core::AgentImageAttachment { name: "renamed.png".into(), media_type: "image/png".into(),
            data_url: String::new(), path: Some(paths[0].to_string_lossy().into_owned()) };
        let persisted = serde_json::to_string(&image).unwrap();
        assert!(!persisted.contains("dataUrl"));
        let reopened = serde_json::from_str(&persisted).unwrap();
        let restored = resolve_image(&root, &reopened).unwrap();
        assert_eq!(restored.name, "renamed.png");
        assert_eq!(restored.data_url, "data:image/png;base64,c2FtZSBpbWFnZQ==");
        fs::remove_file(&paths[0]).unwrap();
        assert!(resolve_image(&root, &reopened).unwrap_err().contains("renamed.png"));
    }

    #[test]
    fn legacy_inline_migration_deduplicates_and_preserves_original_paths() {
        let temp = tempfile::tempdir().unwrap();
        let mut messages = serde_json::json!([
            {"name":"one.png", "kind":"image", "path":"D:/one.png", "dataUrl":"data:image/png;base64,YQ=="},
            {"name":"two.png", "mediaType":"image/png", "dataUrl":"data:image/png;base64,YQ=="},
            {"name":"bad.png", "dataUrl":"data:image/png;base64,?"}
        ]);
        externalize_images(temp.path(), &mut messages).unwrap();
        assert_eq!(messages[0]["path"], "D:/one.png");
        assert_eq!(messages[0]["imagePath"], messages[1]["path"]);
        assert!(messages[0].get("dataUrl").is_none());
        assert!(messages[2].get("dataUrl").is_some());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        inline_export_images(temp.path(), &mut messages).unwrap();
        assert_eq!(messages[0]["dataUrl"], "data:image/png;base64,YQ==");
        assert_eq!(messages[0]["path"], "D:/one.png");
        assert!(messages[1].get("path").is_none());
    }

    #[test]
    fn cache_references_cannot_read_original_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir_all(&root).unwrap();
        let original = temp.path().join("secret.png");
        fs::write(&original, b"secret").unwrap();
        let image = agent_core::AgentImageAttachment { name: "image".into(), media_type: "image/png".into(),
            data_url: String::new(), path: Some(original.to_string_lossy().into_owned()) };
        assert!(resolve_image(&root, &image).is_err());
    }

    #[test]
    fn deleting_one_owner_cannot_remove_a_fresh_content_object_in_another_draft() {
        let temp = tempfile::tempdir().unwrap();
        let path = store_object(temp.path(), b"draft image").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let deleted = HashSet::from([name]);
        assert_eq!(prune_attachments(temp.path(), &deleted, &HashSet::new(), SystemTime::now()).unwrap(), 0);
        assert!(path.exists());
        assert_eq!(prune_attachments(temp.path(), &deleted, &HashSet::new(),
            SystemTime::now() + ORPHAN_ATTACHMENT_GRACE + Duration::from_secs(1)).unwrap(), 1);
    }

    #[test]
    fn cleanup_preserves_shared_uploads_fresh_orphans_and_original_files() {
        let root = std::env::temp_dir().join(format!("android-tool-attachment-gc-{}-{}",
            std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()));
        let cache = root.join("agent-attachments");
        fs::create_dir_all(&cache).unwrap();
        for name in ["deleted.txt", "shared.txt", "fresh.txt"] {
            fs::write(cache.join(name), name).unwrap();
        }
        fs::write(root.join("original.txt"), "original").unwrap();
        let deleted = HashSet::from(["deleted.txt".into(), "shared.txt".into()]);
        let live = HashSet::from(["shared.txt".into()]);
        assert_eq!(prune_attachments(&cache, &deleted, &live, SystemTime::now()).unwrap(), 1);
        assert!(!cache.join("deleted.txt").exists());
        assert!(cache.join("shared.txt").exists());
        assert!(cache.join("fresh.txt").exists());
        assert_eq!(prune_attachments(&cache, &deleted, &live,
            SystemTime::now() + ORPHAN_ATTACHMENT_GRACE + Duration::from_secs(1)).unwrap(), 1);
        assert!(cache.join("shared.txt").exists());
        assert!(root.join("original.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_names_cannot_target_originals_or_nested_paths() {
        let root = Path::new("C:/cache/agent-attachments");
        assert_eq!(attachment_name(root, "C:\\cache\\agent-attachments\\file.md").as_deref(), Some("file.md"));
        for value in ["C:/project/file.md", "C:/cache/agent-attachments/../original.md",
            "C:/cache/agent-attachments/nested/file.md", "C:/cache/agent-attachments-other/file.md"] {
            assert!(attachment_name(root, value).is_none());
        }
    }
}
