use super::*;

pub(super) fn active_images(
    attachments: &[ProviderImageAttachment],
    messages: &[AgentMessage],
    reserve: &AtomicUsize,
) -> Vec<ProviderImageAttachment> {
    let active = image_attachments_for_messages(attachments, messages);
    update_image_token_reserve(&active, reserve);
    active
}

pub(super) fn activate_messages(
    messages: &mut Vec<AgentMessage>,
    runtime_context: &str,
    attachments: &[ProviderImageAttachment],
    reserve: &AtomicUsize,
) {
    cache_context::retain_runtime(messages, runtime_context);
    let active = image_attachments_for_messages(attachments, messages);
    update_image_token_reserve(&active, reserve);
}
