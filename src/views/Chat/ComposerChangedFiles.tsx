import { useState } from "react";
import { Copy, ExternalLink, FolderOpen } from "lucide-react";
import { ContextMenu } from "rain-ui";
import { FileChangeNotice } from "rain-ui";
import { formatInvokeError, invoke } from "@/platform/runtime";
import { ComposerNoticeSlot } from "./ComposerNoticeDock";
import { fileChangeEntries } from "./ChatTaskPanels";
import { looksRemotePath } from "./filePreviewTargets";
import type { ComposerFileChanges } from "./types";

type ComposerChangedFilesProps = {
  changes: ComposerFileChanges;
  onOpen: (path: string) => void;
  workspacePath?: string;
  /** 停靠区里的条目 id；调试面板用独立 id，免得和真实通知抢同一个位置。 */
  slotId?: string;
};

/**
 * 对话页的“文件改动”通知。
 *
 * 它是停靠区（ComposerNoticeDock）里的**第一个**条目：浮在所有通知卡片之上、
 * 紧贴对话内容的下沿，而「追加消息」排在最后、贴着输入框——两头拉开，中间的
 * 权限申请 / AI 询问 / 任务编排 / 后台任务按 `order` 顺序居中排布。
 *
 * 它是独立卡（`independent`）：展开态由自己持有，不进停靠区的「同时只开一个」
 * 选择器。展开态现在是卡片内部向下铺开的列表（与其它通知卡一样参与停靠区高度
 * 计算），独占状态只是为了不和别的卡的折叠互斥逻辑纠缠。
 *
 * 不透明：卡片填充走 notice-panel.css 的 `--notice-fill`（--bg-card 去 alpha），
 * 不用 theme 里那层半透明的 --bg-card，否则上方的对话文字会透出来。 */
export function ComposerChangedFiles({ changes, onOpen, workspacePath, slotId }: ComposerChangedFilesProps) {
  const [menu, setMenu] = useState<{ path: string; status: string; x: number; y: number; trigger: HTMLElement } | null>(null);
  const [notice, setNotice] = useState("");
  const files = fileChangeEntries(changes);

  const openExternal = async (reveal: boolean) => {
    if (!workspacePath || !menu) return;
    if (looksRemotePath(menu.path)) { setNotice("远程文件无法在本机打开，请在右侧预览里查看。"); return; }
    try { await invoke("open_workspace_file", { path: menu.path, workspaceRoot: workspacePath, reveal }); }
    catch (error) { setNotice(`无法打开文件：${formatInvokeError(error)}`); }
  };

  const copyPath = async () => {
    if (!menu) return;
    try { await navigator.clipboard.writeText(menu.path); setNotice("已复制文件路径。"); }
    catch (error) { setNotice(`复制路径失败：${formatInvokeError(error)}`); }
  };

  return (
    <>
      <ComposerNoticeSlot
        id={slotId ?? "files"}
        kind="files"
        label="文件改动"
        summary={`${files.length} 个文件`}
        independent
        render={({ expanded, setExpanded }) => (
          <FileChangeNotice
            className="composer-file-notice"
            files={files}
            addedLines={changes.addedLines}
            removedLines={changes.removedLines}
            workspacePath={workspacePath}
            expanded={expanded}
            onExpandedChange={setExpanded}
            onOpenFile={onOpen}
            onRowContextMenu={(path, status, position, trigger) => setMenu({ path, status, ...position, trigger })}
          />
        )}
      />
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} label="改动文件操作" trigger={menu.trigger} onClose={() => setMenu(null)} items={[
          { label: "打开文件位置", icon: <FolderOpen size={14} />, disabled: !workspacePath || looksRemotePath(menu.path), onSelect: () => void openExternal(true) },
          { label: "用默认程序打开", icon: <ExternalLink size={14} />, disabled: !workspacePath || menu.status === "deleted" || looksRemotePath(menu.path), onSelect: () => void openExternal(false) },
          { label: "复制路径", icon: <Copy size={14} />, onSelect: () => void copyPath() },
        ]} />
      )}
      {notice && <div className="reply-file-notice" role="status">{notice}</div>}
    </>
  );
}
