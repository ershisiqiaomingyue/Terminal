use super::claim::QueuedAgentRequest;
use crate::{QueuedThreadArtifact, QueuedThreadImage, QueuedThreadItem};
use std::collections::VecDeque;

fn is_host_generated(item: &QueuedAgentRequest) -> bool {
    item.request
        .turn_id
        .as_deref()
        .is_some_and(|turn| turn.starts_with(crate::background::BACKGROUND_RESULT_PREFIX))
}
pub(crate) fn queue_items(queue: &VecDeque<QueuedAgentRequest>) -> Vec<QueuedThreadItem> {
    queue
        .iter()
        .filter(|item| !is_host_generated(item))
        .map(|item| QueuedThreadItem {
            id: item.id.clone(),
            content: item.request.message.clone(),
            images: item
                .request
                .attachments
                .iter()
                .filter_map(|image| {
                    image.path.as_ref().map(|path| QueuedThreadImage {
                        name: image.name.clone(),
                        media_type: image.media_type.clone(),
                        path: path.clone(),
                    })
                })
                .collect(),
            artifacts: item
                .request
                .artifacts
                .iter()
                .filter(|artifact| {
                    artifact.status != "workspace_context"
                        && !(artifact.kind == "directory"
                            && item
                                .request
                                .workspace_path
                                .as_deref()
                                .is_some_and(|workspace| {
                                    artifact
                                        .path
                                        .trim_end_matches('/')
                                        .trim_end_matches('\\')
                                        .eq_ignore_ascii_case(
                                            workspace.trim_end_matches('/').trim_end_matches('\\'),
                                        )
                                }))
                })
                .map(|artifact| QueuedThreadArtifact {
                    name: artifact.name.clone(),
                    kind: artifact.kind.clone(),
                    path: artifact.path.clone(),
                    size: artifact.size,
                })
                .collect(),
        })
        .collect()
}
