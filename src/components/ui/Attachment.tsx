import React from "react";
import {
  FileArchive,
  FileCode2,
  FileImage,
  FileJson,
  FileText,
  FolderOpen,
  X,
} from "lucide-react";
import { Button } from "./Button";

/**
 * Attachment
 * ---------------------------------------------------------------------------
 * The composer attachment card. Two shapes share one component because the
 * product treats them as one list:
 *
 *   tile – square image thumbnail, for images dropped into the composer
 *   row  – horizontal card with icon + name + size, for files and folders
 *
 * Kinds mirror what the app actually classifies when something is dropped:
 * image / directory / code / text / archive / json / document. Each gets its
 * own glyph so a mixed list is readable without selecting anything.
 */

export type AttachmentKind =
  | "image"
  | "directory"
  | "code"
  | "text"
  | "archive"
  | "json"
  | "document";

export type AttachmentProps = {
  name: string;
  kind?: AttachmentKind;
  /** Image source. Tiles fall back to the image glyph when absent. */
  src?: string;
  /** Human readable size, e.g. "1.4 MB". */
  detail?: string;
  shape?: "tile" | "row";
  size?: number;
  onRemove?: () => void;
  onOpen?: () => void;
  /** Failed preparation: shows a retry affordance instead of the size. */
  error?: string;
  onRetry?: () => void;
  /** Preparation still running. */
  preparing?: boolean;
  /**
   * Replaces the icon face. Use when the preview cannot be expressed as a
   * plain `src` — e.g. it is fetched lazily from disk and arrives later.
   */
  preview?: React.ReactNode;
  className?: string;
};

const KIND_LABEL: Record<AttachmentKind, string> = {
  image: "图片",
  directory: "工作目录",
  code: "代码",
  text: "文件",
  archive: "压缩包",
  json: "JSON",
  document: "文档",
};

const KIND_GLYPH: Record<AttachmentKind, React.ReactNode> = {
  image: <FileImage size={17} />,
  directory: <FolderOpen size={17} />,
  code: <FileCode2 size={17} />,
  text: <FileText size={17} />,
  archive: <FileArchive size={17} />,
  json: <FileJson size={17} />,
  document: <FileText size={17} />,
};

/** Extension-based kind, for callers that only have a filename. */
export function attachmentKindOf(name: string, isDirectory = false): AttachmentKind {
  if (isDirectory) return "directory";
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  if (["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"].includes(ext)) return "image";
  if (ext === "json") return "json";
  if (["zip", "apk", "jar", "aar", "aab", "gz", "7z", "tar"].includes(ext)) return "archive";
  if (ext === "pdf") return "document";
  if (
    [
      "ts", "tsx", "js", "jsx", "vue", "css", "scss", "java", "kt", "kts",
      "c", "cc", "cpp", "h", "hpp", "rs", "go", "py", "sh", "bat", "ps1",
      "gradle", "xml", "html",
    ].includes(ext)
  ) {
    return "code";
  }
  return "text";
}

export const Attachment: React.FC<AttachmentProps> = ({
  name,
  kind = "text",
  src,
  detail,
  shape = "row",
  size = 56,
  onRemove,
  onOpen,
  error,
  onRetry,
  preparing = false,
  preview,
  className,
}) => {
  const classes = ["ui-attachment", `is-${kind}`, `is-${shape}`, className].filter(Boolean).join(" ");
  const glyph = <span className="ui-attachment-glyph">{KIND_GLYPH[kind]}</span>;

  /* Remove lives beside the face, never inside it: nesting a button in a
     button is invalid and swallows the click in some browsers. */
  const removeBtn = onRemove ? (
    <Button
      type="button"
      variant="bare"
      className="ui-attachment-remove"
      icon={<X size={11} />}
      aria-label={`移除 ${name}`}
      title={`移除 ${name}`}
      onClick={onRemove}
    />
  ) : null;

  if (shape === "tile") {
    return (
      <div className={classes} style={{ width: size, height: size }}>
        <Button
          type="button"
          variant="bare"
          className="ui-attachment-face"
          title={onOpen ? `预览 ${name}` : name}
          aria-label={onOpen ? `预览 ${name}` : name}
          onClick={onOpen}
          disabled={!onOpen}
        >
          {src ? <img src={src} alt={name} /> : glyph}
        </Button>
        {removeBtn}
      </div>
    );
  }

  return (
    <div className={classes}>
      {preview ? (
        preview
      ) : (
        <Button
          type="button"
          variant="bare"
          className="ui-attachment-face"
          title={onOpen ? `打开 ${name}` : name}
          aria-label={onOpen ? `打开 ${name}` : name}
          onClick={onOpen}
          disabled={!onOpen}
        >
          {glyph}
        </Button>
      )}
      <div className="ui-attachment-meta">
        <span className="ui-attachment-name">{name}</span>
        {error ? (
          <Button type="button" variant="bare" className="ui-attachment-retry" title={error} onClick={onRetry}>
            准备失败 · 重试
          </Button>
        ) : (
          <small>{preparing ? "正在准备…" : detail || KIND_LABEL[kind]}</small>
        )}
      </div>
      {removeBtn}
    </div>
  );
};
