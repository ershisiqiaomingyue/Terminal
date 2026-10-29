import { useCallback, useEffect, useState } from "react";
import { Check, Copy, Eraser, File, FileX, FolderOpen, LoaderCircle, Trash2, Upload, X } from "lucide-react";
import { Button, Card } from "@/components/ui";
import { ContextMenu } from "@/components/ui/ContextMenu";
import { formatInvokeError, invoke } from "@/platform/runtime";
import { formatTransferBytes, useFileTransfers } from "./fileTransfers";
import { useDownloadHistory, clearDownloadHistory, removeDownloadRecord } from "./downloadHistoryStore";
import { TransferProgressRows } from "./TransferProgress";
import { useDownloadFiles } from "./useDownloadFiles";
import type { TransferPreparation } from "./useTransferActions";

export type TransferPanelProps = { onClose: () => void; preparing?: TransferPreparation | null; preparationError?: string;
  onUploadFiles?: () => void; onUploadFolder?: () => void; uploadDisabled?: boolean };
// 传输记录的时间需要一眼可读：下载历史卡片很窄，完整 toLocaleString 会被截断。
const formatRecordTime = (timestamp: number) => {
  const value = new Date(timestamp);
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${pad(value.getMonth() + 1)}-${pad(value.getDate())} ${pad(value.getHours())}:${pad(value.getMinutes())}`;
};
export function DownloadHistory(props: TransferPanelProps) { return <TransferHistory {...props} direction="download" />; }
export function UploadHistory(props: TransferPanelProps) { return <TransferHistory {...props} direction="upload" />; }
function TransferHistory({ onClose, direction, preparing, preparationError, onUploadFiles, onUploadFolder, uploadDisabled }: TransferPanelProps & { direction: "upload" | "download" }) {
  const history = useDownloadHistory();
  const downloads = useDownloadFiles(history, direction === "download");
  const current = useFileTransfers().filter((job) => job.direction === direction && (direction === "upload" || job.status !== "completed"));
  const title = direction === "download" ? "下载历史" : "上传记录";
  const [error, setError] = useState("");
  const [menu, setMenu] = useState<{ id: string; path: string; x: number; y: number; trigger: HTMLElement } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  useEffect(() => { if (menu && !history.some((record) => record.id === menu.id)) setMenu(null); }, [history, menu]);
  const open = async (path: string, reveal = false) => {
    setError("");
    try {
      const checks = await downloads.refresh();
      const check = checks?.find((item) => item.path === path);
      if (check?.status !== "present") throw new Error(check?.status === "missing" ? "本地文件已删除" : "本地文件无法访问");
      await invoke("open_code_download", { path, reveal });
    } catch (reason) { setError(`无法${reveal ? "打开文件位置" : "打开文件"}：${formatInvokeError(reason)}`); }
  };
  return <>
    <div className="code-toolbar-popover-heading"><strong>{title}</strong><span style={{ display: "inline-flex", alignItems: "center", gap: 2, marginLeft: "auto" }}>{direction === "download" && history.length > 0 && <Button variant="subtle" size="xs" icon={<Eraser size={14} />} aria-label="清空下载历史" title="清空下载历史（不删除本地文件）" onClick={() => clearDownloadHistory()} />}<Button variant="subtle" size="xs" icon={<X size={14} />} aria-label={`关闭${title}`} onClick={onClose} /></span></div>
    <div className="code-download-list custom-scrollbar">
      {preparing?.direction === direction && <Card tone="muted" className="code-transfer-card code-transfer-preparing">
        <span className="code-transfer-direction" aria-hidden="true"><LoaderCircle size={16} className="inline-spin" /></span>
        <span className="code-transfer-card-content"><strong title={preparing.name}>{preparing.name}</strong><small>{direction === "upload" ? "正在检查上传目录…" : "正在检查下载大小…"}</small></span>
      </Card>}
      <TransferProgressRows jobs={current} />
      {direction === "download" && history.map((record) => {
        const info = downloads.files[record.path];
        // 磁盘检查结果优先：面板打开时 useDownloadFiles 会立刻探测一次，之后每 5 秒复查。
        const status = info?.status ?? record.availability;
        const missing = status === "missing";
        const unavailable = status === "unavailable";
        const disabled = downloads.mutations.busy || missing || unavailable;
        const openHint = missing ? `文件不存在 · ${record.path}` : unavailable ? `无法访问 · ${info?.error || record.path}` : record.path;
        return <Card key={record.id} tone="muted" className={`code-transfer-card code-download-item${missing ? " is-missing" : unavailable ? " is-unavailable" : ""}`}>
        <button type="button" className="code-download-open" disabled={disabled} title={openHint} aria-label={missing ? `${record.name}（文件不存在）` : `打开 ${record.name}`} onClick={() => void open(record.path)} onContextMenu={(event) => { event.preventDefault(); setMenu({ id: record.id, path: record.path, x: event.clientX, y: event.clientY, trigger: event.currentTarget }); }} onKeyDown={(event) => { if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) { event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); setMenu({ id: record.id, path: record.path, x: rect.left, y: rect.bottom, trigger: event.currentTarget }); } }}>
          <span className="code-transfer-direction" aria-hidden="true">{missing || unavailable ? <FileX size={14} /> : <File size={14} />}</span>
          <span className="code-transfer-card-content">
            <span className="code-transfer-title"><strong title={record.name}>{record.name}</strong><span className="code-transfer-stamp"><span className="code-transfer-bytes">{formatTransferBytes(record.bytes)}</span><span className="code-transfer-time">{formatRecordTime(record.finishedAt)}</span></span></span>
            <span className="code-transfer-meta"><span className="code-transfer-server" title={record.server}>{record.server}</span>{missing ? <span className="code-transfer-status">文件不存在</span> : unavailable ? <span className="code-transfer-status">无法访问</span> : null}</span>
            <small className="code-transfer-path" title={record.path}>{record.path}</small>
          </span>
        </button>
        <span className="code-transfer-card-actions">{missing || unavailable ? null : <Check size={14} className="code-download-success" aria-label="已下载" />}</span>
        <span className="code-download-item-actions"><Button variant="subtle" size="xs" icon={<FolderOpen size={15} />} disabled={disabled} aria-label={`打开文件位置 ${record.name}`} title={missing ? "文件不存在，无法打开" : unavailable ? "无法访问，无法打开" : "打开文件位置"} onClick={() => void open(record.path, true)} /><Button variant="subtle" size="xs" icon={<Trash2 size={15} />} disabled={disabled} aria-label={`删除本地文件 ${record.name}`} title={missing ? "文件不存在，无需删除" : unavailable ? "无法访问，无法删除" : "删除本地文件"} onClick={() => downloads.remove(record)} /></span>
      </Card>; })}
      {!(direction === "download" && history.length) && !current.length && preparing?.direction !== direction && <p className="code-toolbar-empty">暂无{direction === "download" ? "下载" : "上传"}记录</p>}
    </div>
    {downloads.mutations.dialog}
    {downloads.error && <p className="code-download-error" role="alert">{downloads.error}</p>}
    {error && <p className="code-download-error" role="alert">{error}</p>}
    {preparationError && <p className="code-download-error" role="alert">{preparationError}</p>}
    {direction === "upload" && <div className="code-transfer-upload-actions"><Button variant="secondary" size="sm" icon={<Upload size={14} />} disabled={uploadDisabled} onClick={onUploadFiles}>上传文件</Button><Button variant="subtle" size="sm" icon={<FolderOpen size={14} />} disabled={uploadDisabled} onClick={onUploadFolder}>上传文件夹</Button></div>}
    {menu && (() => {
      const record = history.find((item) => item.id === menu.id);
      const missing = record?.availability === "missing" || downloads.files[menu.path]?.status === "missing";
      const unavailable = record?.availability === "unavailable" || downloads.files[menu.path]?.status === "unavailable";
      const fileBlocked = missing || unavailable;
      return <ContextMenu {...menu} label="下载文件操作" onClose={closeMenu} items={[
        { label: "打开", icon: <File size={14} />, disabled: fileBlocked, onSelect: () => void open(menu.path) },
        { label: "打开文件位置", icon: <FolderOpen size={14} />, disabled: fileBlocked, onSelect: () => void open(menu.path, true) },
        { label: "复制路径", icon: <Copy size={14} />, onSelect: () => { void navigator.clipboard.writeText(menu.path); } },
        { label: "删除此记录", icon: <Eraser size={14} />, onSelect: () => removeDownloadRecord(menu.id) },
      ]} />;
    })()}
  </>;
}
