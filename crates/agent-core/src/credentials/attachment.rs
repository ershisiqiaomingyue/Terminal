use crate::contract::INLINE_BINARY_ARTIFACT_STATUS;
use crate::{AgentImageAttachment, AgentSteerRequest, LocalArtifact, ProviderImageAttachment};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_AGENT_IMAGE_DATA_URL_CHARS: usize = 12 * 1024 * 1024;
pub const MAX_INLINE_BINARY_DATA_URL_CHARS: usize = 96 * 1024;
const MIN_IMAGE_TOKENS: usize = 1_024;
const MAX_IMAGE_TOKENS: usize = 16_384;

/// Return the browser-provided binary payload without treating its virtual
/// path as a local filesystem path. The browser side stores a complete data
/// URL, so this check also prevents arbitrary text from being interpreted as
/// a binary attachment.
pub fn inline_binary_data_url(artifact: &LocalArtifact) -> Option<&str> {
    if artifact.status != INLINE_BINARY_ARTIFACT_STATUS {
        return None;
    }
    let content = artifact.content.as_deref()?.trim();
    let (header, payload) = content.split_once(',')?;
    if !header
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        || !header.to_ascii_lowercase().ends_with(";base64")
        || payload.bytes().any(
            |byte| !matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'='),
        )
    {
        return None;
    }
    Some(content)
}

pub fn normalize_image_attachments(
    attachments: &[AgentImageAttachment],
    supports_vision: Option<bool>,
    source_message_id: &str,
) -> Result<Vec<ProviderImageAttachment>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    if supports_vision == Some(false) {
        return Err("当前模型明确不支持图片输入".to_string());
    }
    let source_message_id = source_message_id.trim();
    if source_message_id.is_empty() {
        return Err("图片附件缺少所属消息".to_string());
    }
    attachments
        .iter()
        .map(|attachment| {
            let media_type = attachment.media_type.trim().to_ascii_lowercase();
            let data_url = attachment.data_url.trim().to_string();
            if !media_type.starts_with("image/") {
                return Err(format!("附件 {} 不是图片类型", attachment.name));
            }
            if data_url.len() > MAX_AGENT_IMAGE_DATA_URL_CHARS
                || !data_url.starts_with(&format!("data:{media_type};base64,"))
            {
                return Err(format!(
                    "附件 {} 的图片数据无效或超过大小限制",
                    attachment.name
                ));
            }
            Ok(ProviderImageAttachment {
                name: attachment.name.clone(),
                media_type,
                data_url,
                source_message_id: source_message_id.to_string(),
            })
        })
        .collect()
}

pub fn image_attachments_for_messages(
    attachments: &[ProviderImageAttachment],
    messages: &[crate::AgentMessage],
) -> Vec<ProviderImageAttachment> {
    attachments
        .iter()
        .filter(|attachment| {
            messages
                .iter()
                .any(|message| crate::message_owns_image(message, &attachment.source_message_id))
        })
        .cloned()
        .collect()
}

pub fn estimate_image_attachment_tokens(attachments: &[ProviderImageAttachment]) -> usize {
    attachments
        .iter()
        .map(|attachment| {
            // The wire format does not carry decoded pixel dimensions. Use the
            // base64 payload as a conservative estimate with a bounded range.
            let payload_chars = attachment
                .data_url
                .split_once(',')
                .map(|(_, payload)| payload.len())
                .unwrap_or(attachment.data_url.len());
            let decoded_bytes = payload_chars.saturating_mul(3) / 4;
            (decoded_bytes.saturating_add(767) / 768).clamp(MIN_IMAGE_TOKENS, MAX_IMAGE_TOKENS)
        })
        .sum()
}

pub fn update_image_token_reserve(attachments: &[ProviderImageAttachment], reserve: &AtomicUsize) {
    reserve.store(
        estimate_image_attachment_tokens(attachments),
        Ordering::Relaxed,
    );
}

pub fn steer_message_content(steer: &AgentSteerRequest) -> String {
    let mut content = steer.message.trim().to_string();
    if !steer.artifacts.is_empty() {
        content.push_str("\n\n[追加要求中的本地来源]\n");
        for artifact in &steer.artifacts {
            let kind = if artifact.kind == "directory" {
                "目录"
            } else {
                "文件"
            };
            content.push_str(&format!("- {kind}：{}\n", artifact.path));
        }
    }
    if !steer.attachments.is_empty() {
        content.push_str("\n[追加要求中的图片]\n");
        for attachment in &steer.attachments {
            content.push_str(&format!("- 图片：{}\n", attachment.name));
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser_binary(content: &str) -> LocalArtifact {
        LocalArtifact {
            path: "attachment://sample.bin:1:3".to_string(),
            name: "sample.bin".to_string(),
            kind: "file".to_string(),
            size: Some(3),
            modified_at: Some("1".to_string()),
            status: INLINE_BINARY_ARTIFACT_STATUS.to_string(),
            content: Some(content.to_string()),
            content_truncated: false,
        }
    }

    #[test]
    fn recognizes_bounded_browser_binary_data_url() {
        let artifact = browser_binary("data:application/octet-stream;base64,AQID");
        assert_eq!(
            inline_binary_data_url(&artifact),
            Some("data:application/octet-stream;base64,AQID")
        );
    }

    #[test]
    fn rejects_non_data_url_binary_content() {
        let artifact = browser_binary("not-a-data-url");
        assert!(inline_binary_data_url(&artifact).is_none());
    }

    fn image(name: &str, payload: &str) -> AgentImageAttachment {
        AgentImageAttachment {
            path: None,
            name: name.to_string(),
            media_type: "image/png".to_string(),
            data_url: format!("data:image/png;base64,{payload}"),
        }
    }

    #[test]
    fn filters_images_to_user_messages_that_are_still_in_the_request() {
        let mut attachments =
            normalize_image_attachments(&[image("kept.png", "AQID")], Some(true), "user-kept")
                .expect("valid kept image");
        attachments.extend(
            normalize_image_attachments(
                &[image("compacted.png", "BAUG")],
                Some(true),
                "user-compacted",
            )
            .expect("valid compacted image"),
        );
        let mut user = crate::AgentMessage::user("keep this user");
        user.source_message_id = Some("user-kept".to_string());

        let active = image_attachments_for_messages(&attachments, &[user]);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "kept.png");
        assert_eq!(estimate_image_attachment_tokens(&active), 1_024);
    }

    #[test]
    fn checkpoint_restore_rebinds_durable_image_data_by_source_message_id() {
        let checkpoint = crate::AgentContextCheckpoint {
            id: "checkpoint-image".to_string(),
            summary: "image constraint".to_string(),
            evidence_policy_version: 3,
            covered_through_message_id: Some("user-image".to_string()),
            payload: Some(crate::AgentContextCheckpointPayload {
                format_version: crate::CONTEXT_CHECKPOINT_FORMAT_VERSION,
                producer_model_id: Some("vision-model".to_string()),
                entries: vec![crate::AgentContextCheckpointEntry::UserConstraint {
                    content: "inspect the screenshot".to_string(),
                    source_message_id: Some("user-image".to_string()),
                }],
            }),
        };
        let restored = crate::restore_context_checkpoint(&checkpoint, Some("vision-model"))
            .expect("checkpoint should restore");
        let durable_images =
            normalize_image_attachments(&[image("screen.png", "AQID")], Some(true), "user-image")
                .expect("durable image should normalize");

        let rebound = image_attachments_for_messages(&durable_images, &restored);
        assert_eq!(rebound.len(), 1);
        assert_eq!(rebound[0].source_message_id, "user-image");
        assert!(!serde_json::to_string(&checkpoint)
            .expect("checkpoint serialization")
            .contains("AQID"));
    }
}
