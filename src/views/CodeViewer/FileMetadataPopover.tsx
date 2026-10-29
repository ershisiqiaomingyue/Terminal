import { useEffect, useState } from "react";
import { Info, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui";
import { formatInvokeError } from "@/platform/runtime";
import "./file-metadata.css";

export type FileMetadata = { size: number; modifiedAt?: number | null; createdAt?: number | null };

export function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  do { value /= 1024; unit++; } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2)} ${units[unit]}`;
}

export function formatFileDate(value?: number | null) {
  if (value == null || !Number.isFinite(value) || value <= 0) return "不可用";
  return new Intl.DateTimeFormat(undefined, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(value);
}

export function FileMetadataPopover({ cacheKey, load }: { cacheKey: string; load: () => Promise<FileMetadata> }) {
  const [open, setOpen] = useState(false);
  const [metadata, setMetadata] = useState<FileMetadata | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  useEffect(() => { setOpen(false); setMetadata(null); setError(""); setLoading(false); }, [cacheKey]);
  const toggle = () => {
    if (open) { setOpen(false); return; }
    setOpen(true);
    if (metadata || loading) return;
    setLoading(true); setError("");
    void load().then(setMetadata).catch((reason) => setError(formatInvokeError(reason))).finally(() => setLoading(false));
  };
  return <span className="file-metadata-control">
    <Button type="button" variant="subtle" size="xs" icon={<Info size={14} />} aria-label="文件信息" title="文件信息" aria-expanded={open} onClick={toggle} />
    {open && <div className="file-metadata-popover" role="status" aria-label="文件信息">
      <strong>文件信息</strong>
      {loading ? <span className="file-metadata-loading"><LoaderCircle size={13} className="inline-spin" />正在读取</span> : error ? <span className="file-metadata-error">{error}</span> : metadata && <dl>
        <dt>大小</dt><dd>{formatFileSize(metadata.size)}</dd><dt>修改时间</dt><dd>{formatFileDate(metadata.modifiedAt)}</dd><dt>创建时间</dt><dd>{formatFileDate(metadata.createdAt)}</dd>
      </dl>}
    </div>}
  </span>;
}

