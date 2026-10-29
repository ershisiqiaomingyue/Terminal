import { useEffect, useRef } from "react";
import { Download, FolderOpen, HardDrive, History, MoreHorizontal, Plus, RefreshCw, Search, Server } from "lucide-react";
import { Button, SegmentedControl, Switch } from "@/components/ui";
import type { BrowserTarget } from "./useCodeBrowser";
import { ToolbarPopover } from "./ToolbarPopover";
import { DownloadHistory } from "./DownloadHistoryPanel";
import { useFileTransfers } from "./fileTransfers";
import type { TransferPanelProps } from "./DownloadHistoryPanel";
import "./code-viewer-toolbar.css";

type Props = {
  active: boolean; target: BrowserTarget; root: string; loading: boolean; connected: boolean;
  sshConnected: boolean; sshId?: string; directoryOpen: boolean;
  autoRefresh: boolean; autoSave: boolean; hasFile: boolean; projectName: string; filePath: string;
  searchOpen: boolean; settingsOpen: boolean;
  onSearch: () => void; onSettingsToggle: () => void; onSettingsClose: () => void;
  onAutoRefresh: () => void; onAutoSave: () => void; onSource: (source: string) => void;
  onLocalOpen: () => void; onRemoteOpen: () => void; onSshMenu: () => void;
  onRefresh: () => void; onNewTab: () => void; onHistory: () => void;
  downloadOpen: boolean; onDownloadToggle: () => void; onDownloadClose: () => void;
  transferPanel: Omit<TransferPanelProps, "onClose">;
};
export function CodeViewerToolbar(props: Props) {
  const { target, root, loading, connected, sshConnected } = props;
  const downloadAnchor = useRef<HTMLSpanElement>(null);
  const settingsAnchor = useRef<HTMLSpanElement>(null);
  const close = props.onDownloadClose;
  const closeSettings = props.onSettingsClose;
  const jobs = useFileTransfers();
  const downloading = jobs.some((job) => job.direction === "download" && job.status === "running");
  // 关闭回调放进 ref：调用方传内联箭头函数时，下面两个 effect 不能每次渲染都重跑，
  // 否则刚打开的弹层会被立刻关掉。
  const closers = useRef({ close, closeSettings });
  closers.current = { close, closeSettings };
  useEffect(() => {
    if (!props.active) { closers.current.close(); closers.current.closeSettings(); }
  }, [props.active]);
  useEffect(() => { closers.current.closeSettings(); }, [target.kind, props.sshId, sshConnected]);
  // 第一行只显示「项目 › 文件相对路径」，比原来只显示根目录名更容易定位当前文件。
  const breadcrumb = props.projectName ? <span className="code-viewer-root-name" title={`${target.kind === "ssh" ? `${target.name} · ` : ""}${root}`}>
    <b>{props.projectName}</b>{props.filePath && <><i aria-hidden="true">›</i><span>{props.filePath}</span></>}
  </span> : null;
  return <header className="code-viewer-toolbar code-viewer-toolbar-b">
    <h1>代码查看</h1>
    <SegmentedControl className="code-viewer-source-tabs" label="文件来源" value={target.kind} options={[
      { value: "local", label: <><HardDrive size={13} /><span>本地</span></> },
      { value: "ssh", label: <><Server size={13} /><span>SSH</span></> },
    ]} onChange={props.onSource} />
    {breadcrumb}
    <div className="code-viewer-toolbar-actions">
      {props.hasFile && <Button variant="subtle" size="sm" icon={<Search size={15} />} aria-label="搜索文件内容" title="搜索文件内容" aria-pressed={props.searchOpen} onClick={() => { close(); closeSettings(); props.onSearch(); }} />}
      <Button className="code-viewer-open-button" variant="subtle" size="sm" icon={<FolderOpen size={15} />} aria-label="打开文件夹" title="打开文件夹" aria-expanded={props.directoryOpen} aria-haspopup={target.kind === "ssh" ? "dialog" : undefined} onClick={() => {
        close(); closeSettings(); if (target.kind === "local") props.onLocalOpen(); else if (!sshConnected) props.onSshMenu(); else props.onRemoteOpen();
      }} />
      {!!root && <Button variant="subtle" size="sm" disabled={loading || !connected} icon={<RefreshCw size={15} className={loading ? "inline-spin" : undefined} />} aria-label="刷新目录" title="刷新目录" onClick={props.onRefresh} />}
      <span className="code-viewer-toolbar-divider" />
      <Button variant="subtle" size="sm" icon={<Plus size={15} />} aria-label="新标签打开" title="新标签打开当前目录" onClick={props.onNewTab} />
      <Button variant="subtle" size="sm" icon={<History size={15} />} aria-label="历史记录" title="最近打开的目录" onClick={() => { close(); closeSettings(); props.onHistory(); }} />
      <span className="code-transfer-anchor" ref={downloadAnchor}><Button className="code-viewer-download-button" variant="subtle" size="sm" icon={<Download size={15} />} aria-label="下载历史" title="下载历史" aria-expanded={props.downloadOpen} aria-haspopup="dialog" onClick={() => { closeSettings(); props.onDownloadToggle(); }}>{downloading && <i className="code-viewer-download-dot" />}</Button></span>
      <span className="code-transfer-anchor" ref={settingsAnchor}><Button variant="subtle" size="sm" icon={<MoreHorizontal size={15} />} aria-label="查看器设置" title="查看器设置" aria-expanded={props.settingsOpen} aria-haspopup="dialog" onClick={() => { close(); props.onSettingsToggle(); }} /></span>
    </div>
    {props.downloadOpen && downloadAnchor.current && <ToolbarPopover anchor={downloadAnchor.current} label="下载历史" onClose={close}><DownloadHistory {...props.transferPanel} onClose={close} /></ToolbarPopover>}
    {props.settingsOpen && settingsAnchor.current && <ToolbarPopover anchor={settingsAnchor.current} label="查看器设置" onClose={closeSettings}>
      <div className="code-viewer-settings">
        <div className="code-toolbar-popover-heading"><strong>查看器设置</strong></div>
        <div className="code-viewer-setting-row">
          <span><b>自动刷新</b><small>每 5 秒检查磁盘上的当前文件</small></span>
          <Switch checked={props.autoRefresh} disabled={!connected} aria-label="自动刷新当前文件" onChange={props.onAutoRefresh} />
        </div>
        <div className="code-viewer-setting-row">
          <span><b>自动保存</b><small>停止输入 1.2 秒后写入磁盘</small></span>
          <Switch checked={props.autoSave} disabled={!connected} aria-label="自动保存当前文件" onChange={props.onAutoSave} />
        </div>
      </div>
    </ToolbarPopover>}
  </header>;
}
