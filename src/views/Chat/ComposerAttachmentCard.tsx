import { Attachment, attachmentKindOf, type AttachmentKind } from "rain-ui";
import type { ChatSource } from "./types";
import { formatBytes } from "./chatSources";
import { AttachmentImage } from "./AttachmentImage";

interface ComposerAttachmentCardProps {
  source: ChatSource;
  onRemove: () => void;
  onPreview?: (image: { name: string; dataUrl: string }) => void;
  onRetry?: () => void;
}

/**
 * Composer attachment card.
 *
 * The frame is the library `Attachment` (same 48px card, 30px icon chip and
 * kind colours as everywhere else). Two product concerns stay here:
 *
 *   - image thumbnails are lazy-loaded through `AttachmentImage`, because
 *     previews are read from disk on demand and must not be fetched eagerly;
 *   - `preparationError` turns the size line into a retry action.
 */
export function ComposerAttachmentCard({ source, onRemove, onPreview, onRetry }: ComposerAttachmentCardProps) {
  const isImage = source.kind === "image";
  const kind: AttachmentKind = source.kind === "directory" ? "directory" : attachmentKindOf(source.name);
  const preparing = source.preparing && !source.prepared;
  const detail = isImage
    ? formatBytes(source.size) || "图片"
    : source.kind === "directory"
      ? "工作目录"
      : formatBytes(source.size) || "文件";

  return (
    <Attachment
      name={source.name}
      kind={kind}
      detail={detail}
      shape="row"
      preparing={preparing}
      error={source.preparationError}
      onRetry={onRetry}
      onRemove={onRemove}
      preview={
        isImage ? (
          <AttachmentImage
            name={source.name}
            dataUrl={source.dataUrl}
            path={source.path}
            imagePath={source.imagePath || source.prepared?.image?.path || undefined}
            mediaType={source.prepared?.image?.mediaType}
            className="ui-attachment-image"
            onPreview={onPreview}
          />
        ) : undefined
      }
    />
  );
}
