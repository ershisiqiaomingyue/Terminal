import type { ChatMessage, ChatSource } from "./types";

const IMAGE_NAME_PATTERN = /\.(bmp|gif|jpe?g|png|svg|webp)$/i;

/**
 * Editing a message must keep an image an image.
 *
 * Native history can store a user image as a plain artifact (kind "document"
 * with no imagePath) even though the composer originally showed a preview.
 * Round-tripping that value verbatim demoted the draft to a file chip, so the
 * payload is re-classified from the image metadata we do have: an explicit
 * `imagePath`, an image data URL, or an image file extension.
 */
function sourceKindOf(attachment: NonNullable<ChatMessage["attachments"]>[number]): ChatSource["kind"] {
  if (attachment.imagePath) return "image";
  if (typeof attachment.dataUrl === "string" && attachment.dataUrl.startsWith("data:image/")) return "image";
  if (attachment.kind === "image") return "image";
  if (attachment.kind === "directory") return "directory";
  if (IMAGE_NAME_PATTERN.test(attachment.name || "")) return "image";
  return attachment.kind;
}

export function messageAttachmentsToSources(attachments: ChatMessage["attachments"] = []): ChatSource[] {
  return (attachments || []).flatMap((attachment) => {
    const kind = sourceKindOf(attachment);
    const path = attachment.path || attachment.imagePath;
    if (!path && !attachment.dataUrl) return [];
    return [{
      path: path || `browser:${attachment.name}`,
      name: attachment.name,
      kind,
      ...(kind === "image" && (attachment.imagePath || path) ? { imagePath: attachment.imagePath || path } : {}),
      ...(attachment.dataUrl ? { dataUrl: attachment.dataUrl } : {}),
    }];
  });
}

export function remainingComposerSources(current: ChatSource[], submitted: ChatSource[]) {
  return current.filter((source) => !submitted.some((sent) => source.path === sent.path
    && source.kind === sent.kind && (source.dataUrl === sent.dataUrl
      || Boolean(source.prepared && !source.dataUrl && sent.dataUrl))));
}

export function consumeComposerText(current: string, submitted: string) {
  return current === submitted ? "" : current;
}

export function retainedComposerSources(current: ChatSource[] | undefined, restored: ChatSource[]) {
  // An explicit empty draft must survive reads of older history metadata.
  return current ?? restored;
}

export function restoreComposerSources(current: ChatSource[], submitted: ChatSource[]) {
  return [...submitted.filter((source) => !current.some((draft) => draft.path === source.path)), ...current];
}
