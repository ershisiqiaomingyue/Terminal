import { FileArchive, FileCode2, FileImage, FileJson, FileText, FolderOpen } from "lucide-react";
import type { ChatMessage, ChatSourceKind } from "./types";
import { visibleLinkText } from "./linkText";

export const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

export const CODE_EXT = new Set(["ts", "tsx", "js", "jsx", "vue", "css", "scss", "java", "kt", "kts", "c", "cc", "cpp", "h", "hpp", "rs", "go", "py", "sh", "bat", "ps1", "gradle", "xml", "html"]);

export const TEXT_EXT = new Set(["txt", "md", "json", "yaml", "yml", "toml", "ini", "log", "csv", "properties", "cfg", "conf"]);

export const ARCHIVE_EXT = new Set(["zip", "apk", "jar", "aar", "aab", "gz", "7z", "tar"]);

export const DOC_EXT = new Set(["pdf"]);

export function fileExtension(name: string) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function classifyDropped(name: string, isDirectory: boolean): ChatSourceKind | null {
  if (isDirectory) return "directory";
  const ext = fileExtension(name);
  if (IMAGE_EXT.has(ext)) return "image";
  if (CODE_EXT.has(ext)) return "code";
  if (TEXT_EXT.has(ext)) return "text";
  if (ARCHIVE_EXT.has(ext)) return "archive";
  if (DOC_EXT.has(ext)) return "document";
  return null;
}

export function readBrowserFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error || new Error("无法读取拖入文件"));
    reader.onload = () => resolve(String(reader.result || ""));
    reader.readAsDataURL(file);
  });
}

export function richTextToMarkdown(html: string) {
  const documentRef = new DOMParser().parseFromString(html, "text/html");
  const blocks = new Set(["P", "DIV", "LI", "BR", "H1", "H2", "H3", "H4", "H5", "H6", "PRE"]);
  const visit = (node: Node): string => {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || "";
    if (!(node instanceof HTMLElement)) return "";
    if (node instanceof HTMLAnchorElement) {
      const label = [...node.childNodes].map(visit).join("").trim();
      const href = node.href;
      // 很多站点（例如 GitHub 的复制按钮）在 HTML 里只放短标题，
      // 短标题会让用户以为地址被改写了，这里改回完整地址。
      return /^(https?:|mailto:)/i.test(href) ? visibleLinkText(label, href) : label;
    }
    const content = [...node.childNodes].map(visit).join("");
    return blocks.has(node.tagName) ? `${content}\n` : content;
  };
  return [...documentRef.body.childNodes].map(visit).join("").replace(/\n{3,}/g, "\n\n").trim();
}

export function formatBytes(size?: number) {
  if (size == null) return "";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(size < 10 * 1024 ? 1 : 0)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

export function SourceIcon({ name = "", kind }: { name?: string; kind: ChatSourceKind }) {
  const ext = fileExtension(name);
  if (kind === "directory") return <FolderOpen size={17} className="local-path-icon local-path-icon-folder" />;
  if (ext === "json") return <FileJson size={17} className="local-path-icon local-path-icon-json" />;
  if (kind === "image") return <FileImage size={17} className="local-path-icon local-path-icon-image" />;
  if (kind === "archive") return <FileArchive size={17} className="local-path-icon local-path-icon-archive" />;
  if (kind === "code") return <FileCode2 size={17} className="local-path-icon local-path-icon-code" />;
  return <FileText size={17} className="local-path-icon local-path-icon-file" />;
}

export function messageAttachmentKey(
  attachment: NonNullable<ChatMessage["attachments"]>[number],
  index: number,
) {
  if (attachment.path) return `path:${attachment.path}`;
  if (attachment.dataUrl) return `image:${attachment.name}:${attachment.dataUrl}`;
  return `${attachment.kind}:${attachment.name}:${index}`;
}
