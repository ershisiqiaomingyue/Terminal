use super::*;

#[tauri::command]
pub fn prepare_workspace_agent_context(paths: Vec<String>) -> Result<(), String> {
    let artifacts = paths
        .into_iter()
        .map(|path| {
            let metadata =
                fs::metadata(&path).map_err(|error| format!("无法读取工作区：{error}"))?;
            Ok(LocalArtifact {
                name: Path::new(&path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&path)
                    .to_string(),
                path,
                kind: if metadata.is_dir() {
                    "directory"
                } else {
                    "file"
                }
                .to_string(),
                size: Some(metadata.len()),
                modified_at: None,
                status: "ready".to_string(),
                content: None,
                content_truncated: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Some(context) = load_workspace_agent_context(&artifacts) else {
        return Err("没有找到可用的工作区目录".to_string());
    };
    let _ = context;
    Ok(())
}

pub(super) fn apply_steer_runtime_inputs(
    cache_root: &Path,
    steer: &AgentSteerRequest,
    messages: &mut Vec<AgentMessage>,
    windows_paths: &mut Vec<String>,
    tool_policy: &mut AgentToolPolicy,
    model_supports_tools: bool,
    explicitly_disabled: bool,
    supports_vision: Option<bool>,
    image_attachments: &mut Vec<ProviderImageAttachment>,
    image_token_reserve: &AtomicUsize,
) -> Result<String, String> {
    let source_message_id = steer.source_message_id.as_deref().unwrap_or_default();
    let normalized_images = normalize_cached_images(
        cache_root,
        &steer.attachments,
        supports_vision,
        source_message_id,
    )?;
    if !normalized_images.is_empty() {
        image_attachments.extend(normalized_images);
    }
    let content = steer_message_content(steer);
    let mut paths_changed = false;
    for artifact in &steer.artifacts {
        let path = artifact.path.trim();
        if !path.is_empty() && !windows_paths.iter().any(|current| current == path) {
            windows_paths.push(path.to_string());
            paths_changed = true;
        }
    }
    // The tool-policy notice below must NOT be pushed onto `base_system_prompt`: that
    // string is the system block at the very front of the provider prefix, so mutating it
    // invalidates the provider's content-hash cache for every request of the remaining
    // turn (measured: a single leading-token edit drops a warm prefix from ~97% to 0%
    // cache reads). Transient runtime controls belong in the transcript tail instead.
    let mut message = AgentMessage::user(content.clone());
    message.source_message_id = steer.source_message_id.clone();
    if (!steer.artifacts.is_empty() || paths_changed)
        && !explicitly_disabled
        && model_supports_tools
    {
        let was_disabled = *tool_policy == AgentToolPolicy::Disabled;
        *tool_policy = AgentToolPolicy::Auto;
        if was_disabled {
            message
                .content
                .push_str("\n\n[本轮工具策略已更新] 用户在追加要求中提供了本地来源。除非用户明确禁止，否则必须优先使用受控工具验证这些来源。");
        }
    }
    messages.push(message);
    let active_images = image_attachments_for_messages(image_attachments, messages);
    update_image_token_reserve(&active_images, image_token_reserve);
    Ok(content)
}

pub(super) fn append_image_source_labels(
    content: &mut String,
    attachments: &[AgentImageAttachment],
    heading: &str,
) {
    if attachments.is_empty() {
        return;
    }
    content.push_str("\n\n");
    content.push_str(heading);
    content.push('\n');
    for attachment in attachments {
        content.push_str("- 图片：");
        content.push_str(&attachment.name);
        content.push('\n');
    }
}

pub(super) fn normalize_request_image_attachments(
    cache_root: &Path,
    current: &[AgentImageAttachment],
    history: &[AgentContextTurn],
    supports_vision: Option<bool>,
    current_user_message_id: &str,
) -> Result<Vec<ProviderImageAttachment>, String> {
    let mut images = normalize_cached_images(
        cache_root,
        current,
        supports_vision,
        current_user_message_id,
    )?;
    if supports_vision == Some(false) {
        return Ok(images);
    }
    for turn in history {
        if !turn.role.eq_ignore_ascii_case("user") || turn.attachments.is_empty() {
            continue;
        }
        let Some(source_message_id) = turn
            .message_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        // Durable history is untrusted input. A stale or malformed historical
        // image must not block a new text turn; it is simply omitted from the
        // provider payload while its name remains in the transcript.
        for attachment in &turn.attachments {
            if let Ok(mut restored) = normalize_cached_images(
                cache_root,
                std::slice::from_ref(attachment),
                supports_vision,
                source_message_id,
            ) {
                images.append(&mut restored);
            }
        }
    }
    Ok(images)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredProviderSettings {
    pub(super) providers: Vec<AgentProviderSettings>,
}

pub fn local_artifacts(paths: Vec<String>) -> Result<Vec<LocalArtifact>, String> {
    paths
        .into_iter()
        .map(|path| {
            let path_buf = PathBuf::from(&path);
            let metadata = fs::metadata(&path_buf)
                .map_err(|error| format!("无法读取本地路径 {path}: {error}"))?;
            let is_directory = metadata.is_dir();
            let name = path_buf
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(&path)
                .to_string();
            let modified_at = metadata.modified().ok().and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs().to_string())
            });
            Ok(LocalArtifact {
                path,
                name,
                kind: if is_directory { "directory" } else { "file" }.to_string(),
                size: (!is_directory).then_some(metadata.len()),
                modified_at,
                status: "ready".to_string(),
                content: None,
                content_truncated: false,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedChatAttachment {
    pub artifact: LocalArtifact,
    pub image: Option<AgentImageAttachment>,
    pub source_version: String,
}

/// Stat local sources on use; unchanged image selections reuse their immutable
/// snapshot. Text files remain references so tools see current contents.
#[tauri::command]
pub async fn prepare_chat_attachment(
    app: AppHandle,
    path: String,
    previous: Option<PreparedChatAttachment>,
) -> Result<PreparedChatAttachment, String> {
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("agent-attachments");
    tauri::async_runtime::spawn_blocking(move || {
        prepare_local_attachment(&root, &path, previous.as_ref())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn prepare_local_attachment(
    root: &Path,
    path: &str,
    previous: Option<&PreparedChatAttachment>,
) -> Result<PreparedChatAttachment, String> {
    let artifact = local_artifacts(vec![path.to_string()])?.remove(0);
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let version = format!(
        "{}:{:?}",
        metadata.len(),
        metadata.modified().map_err(|error| error.to_string())?
    );
    let media_type = metadata
        .is_file()
        .then(|| local_image_media_type(Path::new(path)))
        .flatten();
    let image = if let Some(media_type) = media_type {
        let reusable = previous
            .filter(|previous| previous.source_version == version && previous.artifact.path == path)
            .and_then(|previous| previous.image.as_ref())
            .filter(|image| image.media_type == media_type)
            .filter(|image| {
                image.path.as_deref().is_some_and(|cached| {
                    crate::attachment_cache::attachment_name(root, cached).is_some()
                        && Path::new(cached).is_file()
                })
            });
        if let Some(image) = reusable {
            fs::OpenOptions::new()
                .write(true)
                .open(image.path.as_ref().unwrap())
                .and_then(|file| file.set_modified(std::time::SystemTime::now()))
                .map_err(|error| format!("无法更新图片缓存引用: {error}"))?;
            Some(image.clone())
        } else {
            let data = crate::attachment_cache::read_bounded_image(Path::new(path))?;
            let cached = crate::attachment_cache::store_object(root, &data)?;
            Some(AgentImageAttachment {
                name: artifact.name.clone(),
                media_type: media_type.into(),
                data_url: String::new(),
                path: Some(cached.to_string_lossy().into_owned()),
            })
        }
    } else {
        None
    };
    Ok(PreparedChatAttachment {
        artifact,
        image,
        source_version: version,
    })
}

#[tauri::command]
pub async fn read_cached_image(
    app: AppHandle,
    image: AgentImageAttachment,
) -> Result<AgentImageAttachment, String> {
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("agent-attachments");
    tauri::async_runtime::spawn_blocking(move || {
        crate::attachment_cache::resolve_image(&root, &image)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn normalize_cached_images(
    root: &Path,
    images: &[AgentImageAttachment],
    supports_vision: Option<bool>,
    owner: &str,
) -> Result<Vec<ProviderImageAttachment>, String> {
    if supports_vision == Some(false) {
        return normalize_image_attachments(images, supports_vision, owner);
    }
    let resolved = images
        .iter()
        .map(|image| crate::attachment_cache::resolve_image(root, image))
        .collect::<Result<Vec<_>, _>>()?;
    normalize_image_attachments(&resolved, supports_vision, owner)
}

#[tauri::command]
pub async fn store_browser_attachment(
    app: AppHandle,
    filename: Option<String>,
    name: Option<String>,
    media_type: Option<String>,
    data: Vec<u8>,
) -> Result<LocalArtifact, String> {
    if data.len() as u64 > MAX_BROWSER_ATTACHMENT_BYTES {
        return Err(format!(
            "附件超过 {} MB 限制",
            MAX_BROWSER_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }
    let _ = media_type;
    let name = browser_attachment_filename(filename, name);
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法定位附件缓存目录: {error}"))?
        .join("agent-attachments");
    let data_len = data.len() as u64;
    let path = tauri::async_runtime::spawn_blocking(move || {
        crate::attachment_cache::store_object(&directory, &data)
    })
    .await
    .map_err(|error| format!("保存附件任务失败: {error}"))??;
    Ok(LocalArtifact {
        path: path.to_string_lossy().to_string(),
        name,
        kind: "file".to_string(),
        size: Some(data_len),
        modified_at: None,
        status: "ready".to_string(),
        content: None,
        content_truncated: false,
    })
}

fn browser_attachment_filename(filename: Option<String>, legacy_name: Option<String>) -> String {
    let path = PathBuf::from(
        filename
            .or(legacy_name)
            .unwrap_or_else(|| "attachment.bin".to_string()),
    );
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment.bin")
        .chars()
        .map(|value| if value.is_control() { '_' } else { value })
        .collect::<String>()
}

#[cfg(test)]
mod browser_attachment_tests {
    use super::browser_attachment_filename;

    #[test]
    fn accepts_current_and_legacy_filename_fields_and_falls_back_for_clipboard_blobs() {
        assert_eq!(
            browser_attachment_filename(Some("screen.png".into()), None),
            "screen.png"
        );
        assert_eq!(
            browser_attachment_filename(None, Some("wechat.png".into())),
            "wechat.png"
        );
        assert_eq!(browser_attachment_filename(None, None), "attachment.bin");
    }
}

pub(super) fn local_image_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

#[tauri::command]
pub fn read_local_image(path: String) -> Result<AgentImageAttachment, String> {
    let path_buf = PathBuf::from(path.trim());
    let media_type = local_image_media_type(&path_buf)
        .ok_or_else(|| "所选文件不是支持的图片格式".to_string())?;
    let metadata = fs::metadata(&path_buf).map_err(|error| format!("无法读取图片：{error}"))?;
    if !metadata.is_file() {
        return Err("图片路径不是文件".to_string());
    }
    if metadata.len() > MAX_LOCAL_IMAGE_BYTES {
        return Err(format!(
            "图片超过 {} MB 限制",
            MAX_LOCAL_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let data = fs::read(&path_buf).map_err(|error| format!("无法读取图片：{error}"))?;
    let name = path_buf
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("图片")
        .to_string();
    Ok(AgentImageAttachment {
        path: None,
        name,
        media_type: media_type.to_string(),
        data_url: format!("data:{media_type};base64,{}", BASE64_STANDARD.encode(data)),
    })
}
