import { useRef } from "react";
import { MoreHorizontal, Upload } from "lucide-react";
import { Button, Input } from "rain-ui";
import { useFileTransfers } from "./fileTransfers";
import { ToolbarPopover } from "./ToolbarPopover";
import { UploadHistory, type TransferPanelProps } from "./DownloadHistoryPanel";

export function CodeBrowserControls({ query, onQuery, ssh, uploadOpen, onUploadToggle, onUploadClose, onMenu, transferPanel }: {
  query: string; onQuery: (query: string) => void; ssh: boolean; uploadOpen: boolean;
  onUploadToggle: () => void; onUploadClose: () => void; onMenu: (anchor: HTMLButtonElement) => void;
  transferPanel: Omit<TransferPanelProps, "onClose">;
}) {
  const anchor = useRef<HTMLSpanElement>(null);
  const uploading = useFileTransfers().some((job) => job.direction === "upload" && job.status === "running");
  return <div className="code-browser-controls">
    <Input aria-label="过滤文件或目录" value={query} onChange={(event) => onQuery(event.target.value)} placeholder="过滤文件或目录…" />
    <div className="code-browser-control-actions">
      {ssh && <span ref={anchor} className="code-transfer-anchor"><Button className="code-browser-icon-button" variant="ghost" icon={<Upload size={15} />} aria-label="上传记录" title="上传记录" aria-expanded={uploadOpen} aria-haspopup="dialog" onClick={onUploadToggle}>{uploading && <i className="code-viewer-download-dot" />}</Button></span>}
      <Button className="code-browser-icon-button" variant="ghost" icon={<MoreHorizontal size={15} />} aria-label="基础功能" title="基础功能" aria-haspopup="menu" onClick={(event) => onMenu(event.currentTarget)} />
    </div>
    {uploadOpen && anchor.current && <ToolbarPopover anchor={anchor.current} label="上传记录" onClose={onUploadClose}><UploadHistory {...transferPanel} onClose={onUploadClose} /></ToolbarPopover>}
  </div>;
}
