import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, ChevronRight, FileCode2, Folder, FolderOpen, Link2, LoaderCircle } from "lucide-react";
import type { BrowserEntry } from "./useCodeBrowser";
import type { GitFile } from "./useGitBrowser";
import { formatInvokeError } from "@/platform/runtime";
import { formatFileDate, formatFileSize, type FileMetadata } from "./FileMetadataPopover";

type Props = {
  root: string;
  dropDirectory?: string;
  tree: Record<string, BrowserEntry[]>;
  openFolders: Record<string, boolean>;
  loadingFolders: Record<string, boolean>;
  activeFile: string;
  query: string;
  connected: boolean;
  metadataScope: string;
  loadMetadata: (entry: BrowserEntry) => Promise<FileMetadata>;
  showHiddenFiles?: boolean;
  fileStatus: (path: string) => GitFile | undefined;
  directoryChanged: (path: string) => boolean;
  onOpen: (entry: BrowserEntry) => void;
  onToggle: (entry: BrowserEntry) => void;
  onDirectoryMenu: (entry: BrowserEntry, event: React.MouseEvent<HTMLButtonElement>) => void;
  onMove: (entry: BrowserEntry, destinationDirectory: string) => void;
  onDragEntryChange?: (entry: BrowserEntry | null) => void;
};
import { canMoveEntry } from "./treeDrag";
import { useTreeRowDrag } from "./useTreeRowDrag";
export { canMoveEntry } from "./treeDrag";
const nameOrder = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });
const orderedEntries = new WeakMap<BrowserEntry[], BrowserEntry[]>();
const MAX_METADATA_CACHE_ENTRIES = 256;
function sortedEntries(entries: BrowserEntry[]) {
  let sorted = orderedEntries.get(entries);
  if (!sorted) {
    sorted = [...entries].sort((left, right) => Number(right.isDirectory) - Number(left.isDirectory) || nameOrder.compare(left.name, right.name));
    orderedEntries.set(entries, sorted);
  }
  return sorted;
}

// AospIde's tree keeps link identity separate from the target's file type.
// Reuse that presentation here with this app's icons and semantic tokens.
export function CodeFileTree(props: Props) {
  const [hovered, setHovered] = useState<{ entry: BrowserEntry; left: number; top: number; loading: boolean; data?: FileMetadata; error?: string } | null>(null);
  const drag = useTreeRowDrag({ enabled: props.connected, onMove: props.onMove, onDragEntryChange: props.onDragEntryChange });
  const moveTarget = drag.moveTarget;
  const cache = useRef(new Map<string, FileMetadata>());
  const hoverPath = useRef("");
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => { cache.current.clear(); setHovered(null); hoverPath.current = ""; }, [props.metadataScope]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const enterFile = (entry: BrowserEntry, element: HTMLElement) => {
    if (entry.isDirectory || !props.connected || drag.dragging) return;
    if (timer.current) clearTimeout(timer.current);
    const cacheKey = `${props.metadataScope}:${entry.path}`;
    const existing = cache.current.get(cacheKey);
    const bounds = element.getBoundingClientRect();
    const cardWidth = 264;
    const left = bounds.right + cardWidth + 12 < window.innerWidth ? bounds.right + 8 : Math.max(8, bounds.left - cardWidth - 8);
    const top = Math.min(Math.max(8, bounds.top), Math.max(8, window.innerHeight - 150));
    hoverPath.current = entry.path;
    if (existing) {
      cache.current.delete(cacheKey);
      cache.current.set(cacheKey, existing);
    }
    setHovered(existing
      ? { entry, left, top, loading: false, data: existing }
      : { entry, left, top, loading: true });
    if (existing) return;
    timer.current = setTimeout(() => {
      void props.loadMetadata(entry).then((data) => {
        // Hovering a large tree should not retain one metadata object per file
        // for the lifetime of the viewer. Map insertion order gives us a small
        // and predictable LRU approximation without another dependency.
        cache.current.delete(cacheKey);
        cache.current.set(cacheKey, data);
        while (cache.current.size > MAX_METADATA_CACHE_ENTRIES) {
          cache.current.delete(cache.current.keys().next().value!);
        }
        if (hoverPath.current === entry.path) setHovered({ entry, left, top, loading: false, data });
      }).catch((reason) => {
        if (hoverPath.current === entry.path) setHovered({ entry, left, top, loading: false, error: formatInvokeError(reason) });
      });
    }, 180);
  };
  const leaveFile = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = undefined;
    hoverPath.current = "";
    setHovered(null);
  };
  const renderEntries = (path: string, depth = 0): React.ReactNode => sortedEntries(props.tree[path] || [])
    .filter((entry) => (props.showHiddenFiles || !entry.name.startsWith(".")) && (entry.isDirectory || !props.query || `${entry.name} ${entry.symlinkTarget || ""}`.toLowerCase().includes(props.query.toLowerCase())))
    .map((entry) => {
      const linked = entry.symlinkTarget != null;
      const expanded = Boolean(props.openFolders[entry.path]);
      const loading = Boolean(props.loadingFolders[entry.path]);
      const file = props.fileStatus(entry.path);
      const changed = entry.isDirectory && props.directoryChanged(entry.path);
      const title = linked ? `${entry.path} → ${entry.symlinkTarget}${entry.kind === "symlink" ? "（链接失效或无法访问）" : ""}` : entry.path;
      return <div key={entry.path}>
        <div className={`code-viewer-tree-row${drag.draggedEntry?.path === entry.path ? " is-dragging" : ""}`} onPointerDown={(event) => { if (!event.currentTarget.contains(event.target as Node)) return; drag.begin(entry, event); }} onPointerEnter={(event) => enterFile(entry, event.currentTarget)} onPointerLeave={leaveFile} onFocus={() => enterFile(entry, document.activeElement instanceof HTMLElement ? document.activeElement : document.body)} onBlur={leaveFile}>
        <button type="button" className={`archive-tree-item code-viewer-tree-item${props.activeFile === entry.path ? " selected" : ""}${props.dropDirectory === entry.path ? " is-upload-target" : ""}${moveTarget === entry.path ? " is-move-target" : ""}${linked ? " is-linked" : ""}${entry.kind === "symlink" ? " is-broken-link" : ""}`}
          data-code-directory={entry.isDirectory ? entry.path : path}
          style={{ paddingLeft: `${10 + depth * 14}px` }} disabled={!props.connected} title={linked ? title : undefined}
          aria-expanded={entry.isDirectory ? expanded : undefined} aria-busy={loading || undefined}
          onContextMenu={(event) => { event.preventDefault(); props.onDirectoryMenu(entry, event); }}
          onClick={() => entry.isDirectory ? props.onToggle(entry) : props.onOpen(entry)}>
          <span className="code-viewer-tree-icon" aria-hidden="true">
            {entry.isDirectory ? (expanded ? <FolderOpen size={14} /> : <Folder size={14} />) : <FileCode2 size={14} />}
            {linked && <Link2 className="code-viewer-link-badge" size={9} />}
          </span>
          <span className="code-viewer-tree-label"><span className="tree-name">{entry.name}</span>
            {linked && <span className="code-viewer-link-target">→ {entry.symlinkTarget}</span>}
          </span>
          {(file || changed) && <span className="code-viewer-git-status" title={file ? `Git 状态：${file.status}${file.status === "??" ? " · 未跟踪" : " · 未提交"}` : "目录包含 Git 改动"}>
            {file?.additions != null && <em className="is-added">+{file.additions}</em>}
            {file?.deletions != null && <em className="is-deleted">−{file.deletions}</em>}
            <b>{file?.status.trim() || "M"}</b>
          </span>}
          {entry.isDirectory && (loading ? <LoaderCircle size={13} className="inline-spin" /> : expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />)}
        </button>
        </div>
        {entry.isDirectory && expanded && renderEntries(entry.path, depth + 1)}
      </div>;
    });
  return <>{renderEntries(props.root)}{drag.dragging && drag.draggedEntry && createPortal(<div className="code-tree-drag-ghost" style={{ left: drag.pointer.x + 14, top: drag.pointer.y + 14 }} role="status" aria-live="polite">
    <span className="code-tree-drag-name">{drag.draggedEntry.name}</span>
    <span className="code-tree-drag-hint">{moveTarget ? `移动到 ${moveTarget.split(/[\\/]/).filter(Boolean).pop() || moveTarget}` : "拖到目标文件夹"}</span>
  </div>, document.body)}{hovered && createPortal(<div className="file-metadata-hover-card" style={{ left: hovered.left, top: hovered.top }} role="status" aria-label={`文件信息：${hovered.entry.name}`}>
    <strong title={hovered.entry.path}>{hovered.entry.name}</strong>
    {hovered.loading ? <span className="file-metadata-loading"><LoaderCircle size={13} className="inline-spin" />正在读取</span> : hovered.error ? <span className="file-metadata-error">{hovered.error}</span> : hovered.data && <dl>
      <dt>大小</dt><dd>{formatFileSize(hovered.data.size)}</dd>
      <dt>修改时间</dt><dd>{formatFileDate(hovered.data.modifiedAt)}</dd>
      <dt>创建时间</dt><dd>{formatFileDate(hovered.data.createdAt)}</dd>
    </dl>}
  </div>, document.body)}</>;
}
