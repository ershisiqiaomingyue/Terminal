import { useState } from "react";
import { Copy, ExternalLink, FileDiff, FolderOpen } from "lucide-react";
import { FileChangeCard } from "rain-ui";
import { ContextMenu } from "rain-ui";
import { useConfirmAction } from "rain-ui";
import { formatInvokeError, invoke } from "@/platform/runtime";
import type { AgentEventPayload } from "@/platform/chatApi";
import { composerFileChanges, fileChangeEntries } from "./ChatTaskPanels";
import { looksRemotePath } from "./filePreviewTargets";
import type { FilePreviewOrigin } from "./filePreviewDiff";

export function MessageFileChanges({ event, onOpen, workspacePath, onLayoutChange, onUndo }: { event?: AgentEventPayload; onOpen: (path: string, origin?: FilePreviewOrigin) => void; workspacePath?: string; onLayoutChange?: () => void; /** Local action for debug fixtures. */ onUndo?: () => void }) {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [notice, setNotice] = useState("");
  const [revertedPaths, setRevertedPaths] = useState<string[]>([]);
  const [fileMenu, setFileMenu] = useState<{ file: ReturnType<typeof fileChangeEntries>[number]; x: number; y: number; trigger: HTMLElement } | null>(null);
  const changes = composerFileChanges(event);
  if (!changes) return null;
  const originalFiles = fileChangeEntries(changes);
  const files = originalFiles.filter((file) => !revertedPaths.includes(file.path));
  const reverted = originalFiles.filter((file) => revertedPaths.includes(file.path));
  const remainingChanges = { ...changes, entries: files, created: [], modified: [], deleted: [],
    addedLines: Math.max(0, changes.addedLines - reverted.reduce((total, file) => total + (file.addedLines || 0), 0)),
    removedLines: Math.max(0, changes.removedLines - reverted.reduce((total, file) => total + (file.removedLines || 0), 0)) };
  const { sessionId = "", turnId = "" } = changes;
  const reviewAvailable = event?.review_available !== false;
  // 远程（Unix 绝对路径）改动的备份与「打开文件位置」都落在本机磁盘上，对远程路径
  // 必然失败。这类行只保留「查看变更」（走远程读文件）与「复制路径」。
  const isRemote = (path: string) => looksRemotePath(path);
  const allRemoteFree = files.every((file) => !isRemote(file.path));
  const reviewFile = (path: string) => onOpen(path, reviewAvailable ? { sessionId, turnId } : undefined);
  const openExternal = async (file: ReturnType<typeof fileChangeEntries>[number], reveal: boolean) => {
    if (!workspacePath) return;
    if (isRemote(file.path)) { setNotice("远程文件无法在本机打开，请在右侧预览里查看。"); return; }
    try { await invoke("open_workspace_file", { path: file.path, workspaceRoot: workspacePath, reveal }); }
    catch (error) { setNotice(`无法${reveal ? "打开文件位置" : "用默认程序打开文件"}：${formatInvokeError(error)}`); }
  };
  const copyPath = async (path: string) => {
    try { await navigator.clipboard.writeText(path); setNotice("已复制文件路径。"); }
    catch (error) { setNotice(`复制路径失败：${formatInvokeError(error)}`); }
  };
  const performUndoAll = async () => {
    const restored: string[] = [];
    for (const file of files) { try { await invoke("undo_agent_file_change", { sessionId, turnId, path: file.path }); restored.push(file.path); } catch (error) { setNotice(`${restored.length ? `已撤销 ${restored.length} 个文件；` : ""}${formatInvokeError(error)}`); break; } }
    if (restored.length) { setRevertedPaths((previous) => [...new Set([...previous, ...restored])]); if (restored.length === files.length) setNotice(`已撤销本轮 ${restored.length} 个文件。`); }
  };
  const undoAll = () => confirmAction({ title: "撤销本轮全部改动", message: `确认撤销本轮 ${files.length} 个文件的改动？新建文件可能被删除，后续编辑也可能受到影响。`, confirmLabel: "确认撤销", onConfirm: performUndoAll });
  return <section className="message-file-changes" aria-label="本轮文件变更">
    <FileChangeCard
      files={originalFiles}
      revertedPaths={revertedPaths}
      addedLines={remainingChanges.addedLines}
      removedLines={remainingChanges.removedLines}
      showLineCounts={event?.line_counts_available !== false}
      workspacePath={workspacePath}
      onOpenFile={reviewFile}
      onReview={() => { if (files[0]) reviewFile(files[0].path); }}
      onUndo={reviewAvailable && allRemoteFree ? () => { if (onUndo) onUndo(); else void undoAll(); } : undefined}
      onExpandedChange={onLayoutChange}
      onRowContextMenu={(path, status, position, trigger) => {
        const file = files.find((item) => item.path === path && item.status === status);
        if (file) setFileMenu({ file, ...position, trigger });
      }}
    />
    {fileMenu && <ContextMenu x={fileMenu.x} y={fileMenu.y} label="改动文件操作" trigger={fileMenu.trigger} onClose={() => setFileMenu(null)} items={[
      { label: "查看变更", icon: <FileDiff size={14} />, onSelect: () => reviewFile(fileMenu.file.path) },
      { label: "打开文件位置", icon: <FolderOpen size={14} />, disabled: !workspacePath || isRemote(fileMenu.file.path), onSelect: () => void openExternal(fileMenu.file, true) },
      { label: "用默认程序打开", icon: <ExternalLink size={14} />, disabled: !workspacePath || fileMenu.file.status === "deleted" || isRemote(fileMenu.file.path), onSelect: () => void openExternal(fileMenu.file, false) },
      { label: "复制路径", icon: <Copy size={14} />, onSelect: () => void copyPath(fileMenu.file.path) },
    ]} />}
    {confirmationDialog}
    {notice && <div className="reply-file-notice">{notice}</div>}</section>;
}
