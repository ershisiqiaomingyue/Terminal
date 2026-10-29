import type { AgentArtifact, AgentImageAttachment } from "@/platform/chatApi";
import type { ChatMessage, ChatSourceKind } from "./types";
import { localArtifactPathKey } from "./localArtifactPaths.ts";

type ChatAttachment = NonNullable<ChatMessage["attachments"]>[number];
type ThreadUserMedia = {
  attachments?: AgentImageAttachment[];
  artifacts?: AgentArtifact[];
};

export function visibleMessageAttachments(attachments: ChatMessage["attachments"], workspacePath?: string) {
  if (!workspacePath) return attachments;
  const workspace = localArtifactPathKey(workspacePath);
  return attachments?.filter((attachment) => attachment.origin === "input" || attachment.kind !== "directory"
    || !attachment.path || localArtifactPathKey(attachment.path) !== workspace);
}

const IMAGE_EXTENSIONS = new Set(["bmp", "gif", "jpeg", "jpg", "png", "svg", "webp"]);
const CODE_EXTENSIONS = new Set(["c", "cc", "cpp", "cs", "css", "go", "h", "hpp", "html", "java", "js", "json", "kt", "kts", "md", "mjs", "py", "rs", "sh", "sql", "swift", "toml", "ts", "tsx", "xml", "yaml", "yml"]);
const TEXT_EXTENSIONS = new Set(["csv", "log", "properties", "text", "txt"]);
const ARCHIVE_EXTENSIONS = new Set(["7z", "aab", "aar", "apk", "apks", "gz", "jar", "rar", "tar", "tgz", "zip"]);

function attachmentKind(name: string, artifactKind = ""): ChatSourceKind {
  if (artifactKind === "directory") return "directory";
  const extension = name.toLowerCase().split(".").pop() || "";
  if (IMAGE_EXTENSIONS.has(extension)) return "image";
  if (CODE_EXTENSIONS.has(extension)) return "code";
  if (TEXT_EXTENSIONS.has(extension)) return "text";
  if (ARCHIVE_EXTENSIONS.has(extension)) return "archive";
  return "document";
}

function cleanName(name: unknown, path: unknown) {
  if (typeof name === "string" && name.trim()) return name.trim();
  if (typeof path !== "string") return "附件";
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

/** Projects native thread media into the stable attachment shape used by chat rows. */
export function projectThreadUserAttachments(
  media: ThreadUserMedia,
  retained: ChatMessage["attachments"] = [],
): ChatMessage["attachments"] {
  const projected: ChatAttachment[] = [];
  const sameIdentity = (left: ChatAttachment, right: ChatAttachment) => {
    if (left.imagePath || right.imagePath) {
      if (left.imagePath && right.imagePath) return left.imagePath === right.imagePath && left.name === right.name;
      if (!left.path || !right.path) return false;
    }
    if (left.path || right.path) return Boolean(left.path && right.path && left.path === right.path);
    if (left.kind === "image" && right.kind === "image") {
      return left.name === right.name && left.dataUrl === right.dataUrl;
    }
    return left.name === right.name && left.kind === right.kind && left.dataUrl === right.dataUrl;
  };
  const merge = (previous: ChatAttachment, attachment: ChatAttachment): ChatAttachment => ({
      ...previous,
      ...attachment,
      path: attachment.path || previous.path,
      dataUrl: attachment.dataUrl || previous.dataUrl,
      ...(attachment.imagePath || previous.imagePath ? {
        imagePath: attachment.imagePath || previous.imagePath,
        mediaType: attachment.mediaType || previous.mediaType,
      } : {}),
  });
  const add = (attachment: ChatAttachment) => {
    const index = projected.findIndex((candidate) => sameIdentity(candidate, attachment));
    if (index >= 0) projected[index] = merge(projected[index], attachment);
    else projected.push(attachment);
  };

  const implicitPaths = new Set((media.artifacts || []).filter((artifact) => artifact?.status === "workspace_context"
    && typeof artifact.path === "string").map((artifact) => localArtifactPathKey(artifact.path!)));
  retained.filter((attachment) => attachment.origin === "input" || attachment.kind !== "directory" || !attachment.path
    || !implicitPaths.has(localArtifactPathKey(attachment.path))).forEach(add);
  for (const attachment of media.attachments || []) {
    if (!attachment || typeof attachment.name !== "string" || (!attachment.dataUrl && !attachment.path)) continue;
    add({ name: attachment.name, kind: "image", ...(attachment.dataUrl ? { dataUrl: attachment.dataUrl } : {}),
      ...(attachment.path ? { imagePath: attachment.path, mediaType: attachment.mediaType } : {}) });
  }
  for (const artifact of media.artifacts || []) {
    if (!artifact || artifact.status === "workspace_context") continue;
    const name = cleanName(artifact.name, artifact.path);
    const path = typeof artifact.path === "string" && artifact.path ? artifact.path : undefined;
    add({ name, kind: attachmentKind(name, typeof artifact.kind === "string" ? artifact.kind : ""), path,
      ...(artifact.status === "user_attachment" ? { origin: "input" as const } : {}) });
  }

  // Native image metadata has a path while provider image payloads do not.
  // Join them only when the name identifies exactly one item on each side;
  // otherwise parallel screenshots or same-name files would be lost.
  const removed = new Set<number>();
  const names = new Set(projected.filter((item) => item.kind === "image").map((item) => item.name));
  for (const name of names) {
    const withoutPath = projected
      .map((item, index) => ({ item, index }))
      .filter(({ item }) => item.kind === "image" && item.name === name && !item.path);
    const withPath = projected
      .map((item, index) => ({ item, index }))
      .filter(({ item }) => item.kind === "image" && item.name === name && Boolean(item.path));
    if (withoutPath.length !== 1 || withPath.length !== 1) continue;
    const providerImage = withoutPath[0];
    const nativeImage = withPath[0];
    if (providerImage.item.dataUrl && nativeImage.item.dataUrl
      && providerImage.item.dataUrl !== nativeImage.item.dataUrl) continue;
    projected[nativeImage.index] = merge(nativeImage.item, providerImage.item);
    removed.add(providerImage.index);
  }
  const attachments = projected.filter((_, index) => !removed.has(index));
  return attachments.length ? attachments : undefined;
}
