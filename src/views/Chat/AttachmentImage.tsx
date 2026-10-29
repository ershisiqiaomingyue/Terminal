import { useEffect, useRef, useState } from "react";
import { FileImage } from "lucide-react";
import { prepareChatAttachment, readCachedImage } from "@/platform/chatApi";
import { formatInvokeError } from "@/platform/runtime";

/** Load visible thumbnails only. Preview data stays out of session/history
 * state and is released with the row. */
export function AttachmentImage({ name, dataUrl, path, imagePath, mediaType, onPreview, className }: {
  name: string; dataUrl?: string; path?: string; imagePath?: string; mediaType?: string;
  onPreview?: (image: { name: string; dataUrl: string }) => void; className?: string;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const [url, setUrl] = useState(dataUrl || "");
  const [error, setError] = useState("");
  const [attempt, retry] = useState(0);
  useEffect(() => {
    setUrl(dataUrl || "");
    setError("");
    if (dataUrl) return;
    let alive = true;
    let started = false;
    const load = () => {
      if (started) return;
      started = true;
      const loadImage = async () => {
        if (imagePath) {
          const extension = name.toLowerCase().split(".").pop();
          const inferredType = extension === "jpg" || extension === "jpeg" ? "image/jpeg"
            : extension === "svg" ? "image/svg+xml" : `image/${extension || "png"}`;
          return readCachedImage({ name, path: imagePath, mediaType: mediaType || inferredType, dataUrl: "" });
        }
        if (!path || path.startsWith("browser:")) throw new Error("图片路径不可用，请重新添加图片。");
        // Older history contains the original file path without a cached image
        // reference. Prepare it through the same bounded reader as the composer.
        const prepared = await prepareChatAttachment(path);
        if (!prepared.image) throw new Error("该附件没有可预览的图片内容。");
        return readCachedImage(prepared.image);
      };
      void loadImage().then((image) => {
        if (!alive) return;
        if (!image.dataUrl) throw new Error("图片未返回预览内容。");
        setUrl(image.dataUrl);
      }).catch((reason) => { if (alive) setError(formatInvokeError(reason)); });
    };
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) { observer.disconnect(); load(); }
    }, { rootMargin: "120px" });
    if (anchor.current) observer.observe(anchor.current);
    return () => { alive = false; observer.disconnect(); };
  }, [name, dataUrl, path, imagePath, mediaType, attempt]);
  return <button ref={anchor} type="button" className={className}
    title={error ? `${error} · 点击重试` : `预览 ${name}`} aria-label={error ? `重试加载 ${name}` : `预览 ${name}`}
    onClick={() => { if (error) retry((value) => value + 1); else if (url) onPreview?.({ name, dataUrl: url }); }}>
    {url ? <img src={url} alt={name} onError={() => { setUrl(""); setError("图片无法加载"); }} /> : <FileImage size={18} aria-hidden="true" />}
  </button>;
}
