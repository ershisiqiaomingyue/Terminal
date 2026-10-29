use super::super::*;

use super::text::{is_process_boundary, public_text_for_items};

pub(crate) fn user_message_item(
    id: String,
    content: String,
    attachments: &[AgentImageAttachment],
    artifacts: &[LocalArtifact],
    workspace_path: Option<&str>,
) -> ThreadItem {
    ThreadItem::UserMessage {
        id,
        content,
        attachments: attachments
            .iter()
            .map(|image| {
                json!({
                    "name":image.name, "mediaType":image.media_type, "dataUrl":image.data_url,
                    "path":image.path,
                })
            })
            .collect(),
        artifacts: artifacts
            .iter()
            .filter(|artifact| {
                artifact.status != "workspace_context"
                    && (artifact.status == "user_attachment"
                        || artifact.kind != "directory"
                        || !workspace_path.is_some_and(|workspace| {
                            agent_core::policy_path_key(Path::new(workspace))
                                == agent_core::policy_path_key(Path::new(&artifact.path))
                        }))
            })
            .map(|artifact| {
                json!({
                    "path":artifact.path, "name":artifact.name, "kind":artifact.kind,
                    "size":artifact.size, "modifiedAt":artifact.modified_at,
                    "status":artifact.status, "contentTruncated":artifact.content_truncated,
                })
            })
            .collect(),
    }
}

pub(crate) fn display_segments_for_snapshot(
    snapshot: &agent_core::ThreadSnapshot,
    retain_partial: bool,
) -> Vec<Value> {
    let Some(turn) = snapshot.active_turn.as_ref() else {
        return Vec::new();
    };
    let mut segments = Vec::new();
    let mut start = 0;
    let mut assistant_index = 0;
    for (index, item) in snapshot.items.iter().enumerate() {
        let ThreadItem::UserMessage {
            id,
            content,
            attachments,
            artifacts,
        } = item
        else {
            continue;
        };
        if start < index {
            let assistant_id = if assistant_index == 0 {
                format!("assistant:{}", turn.id)
            } else {
                format!("assistant:{}:segment-{assistant_index}", turn.id)
            };
            segments.push(json!({"id":assistant_id, "role":"assistant", "content":public_text_for_items(&snapshot.items[start..index]), "process_start":start, "process_end":index}));
            assistant_index += 1;
        }
        segments.push(json!({"id":id, "role":"user", "content":content, "attachments":attachments, "artifacts":artifacts, "process_start":index, "process_end":index + 1}));
        start = index + 1;
    }
    if start < snapshot.items.len() {
        let assistant_id = if assistant_index == 0 {
            format!("assistant:{}", turn.id)
        } else {
            format!("assistant:{}:segment-{assistant_index}", turn.id)
        };
        let items = &snapshot.items[start..];
        let after_tool = items
            .iter()
            .rposition(is_process_boundary)
            .map_or(0, |index| index + 1);
        let tail_text = public_text_for_items(&items[after_tool..]);
        let content = if retain_partial || tail_text.trim().is_empty() {
            public_text_for_items(items)
        } else {
            tail_text
        };
        segments.push(json!({"id":assistant_id, "role":"assistant", "content":content, "process_start":start, "process_end":snapshot.items.len()}));
    }
    segments
}
