import React, { useId, useState } from "react";
import { ArrowUpRight, ChevronDown, ChevronUp, FileDiff, Undo2 } from "lucide-react";
import { Button } from "./Button";
import { Card } from "./Card";

/**
 * FileChangeCard
 * ---------------------------------------------------------------------------
 * The card that settles at the end of an assistant reply, listing the files the
 * turn touched. This is the counterpart to the in-progress notice: while the
 * reply is streaming the composer shows a compact 文件改动 row, and once the
 * turn finishes the same changes are summarised here.
 *
 * Shared Card shell and the settled reply styling from ChatFileChanges and
 * chatAppearance, kept here so the gallery and conversation render alike:
 *   header   icon 36px, title "已编辑 N 个文件" followed by total +N −N
 *   actions  撤销 / 查看变更 on the header
 *   list     3 rows by default, "再显示 N 个文件" to expand, deleted struck through
 *   card     max-width 760px, shared Card surface and radius
 */

export type FileChangeStatus = "created" | "modified" | "deleted";

export type FileChangeEntry = {
  path: string;
  status: FileChangeStatus;
  addedLines?: number;
  removedLines?: number;
};

export type FileChangeCardProps = {
  files: FileChangeEntry[];
  /** Total diff; defaults to the sum of the entries. */
  addedLines?: number;
  removedLines?: number;
  showLineCounts?: boolean;
  /** Strips a workspace prefix so rows show a relative path. */
  workspacePath?: string;
  /** Rows collapsed before expanding. */
  previewCount?: number;
  onOpenFile?: (path: string) => void;
  /**
   * Right-click or Shift+F10 on a row. The caller receives the path, the status
   * and whichever anchor is available: the pointer position when opened by
   * mouse, or the row rect when opened by keyboard.
   */
  onRowContextMenu?: (
    path: string,
    status: FileChangeStatus,
    position: { x: number; y: number },
    trigger: HTMLElement,
  ) => void;
  onReview?: () => void;
  onUndo?: () => void;
  onExpandedChange?: (expanded: boolean) => void;
  /** Already-undone paths are hidden from the list. */
  revertedPaths?: string[];
  className?: string;
};

const displayPath = (path: string, workspacePath?: string) => {
  const normalized = path.replace(/\\/g, "/");
  const root = workspacePath?.replace(/\\/g, "/").replace(/\/+$/, "");
  return root && normalized.toLowerCase().startsWith(`${root.toLowerCase()}/`)
    ? normalized.slice(root.length + 1)
    : normalized;
};

export const FileChangeCard: React.FC<FileChangeCardProps> = ({
  files,
  addedLines,
  removedLines,
  showLineCounts = true,
  workspacePath,
  previewCount = 3,
  onOpenFile,
  onRowContextMenu,
  onReview,
  onUndo,
  onExpandedChange,
  revertedPaths = [],
  className,
}) => {
  const [expanded, setExpanded] = useState(false);
  const listId = useId();

  const visible = files.filter((file) => !revertedPaths.includes(file.path));
  if (!visible.length) return null;

  const totalAdded = addedLines ?? visible.reduce((sum, file) => sum + (file.addedLines || 0), 0);
  const totalRemoved = removedLines ?? visible.reduce((sum, file) => sum + (file.removedLines || 0), 0);
  const shown = expanded ? visible : visible.slice(0, previewCount);
  const hidden = visible.length - previewCount;

  return (
    <Card
      className={["ui-file-changes", className].filter(Boolean).join(" ")}
      role="region"
      aria-label="本轮文件变更"
    >
      <header>
        <span className="ui-file-changes-icon" aria-hidden="true">
          <FileDiff size={18} />
        </span>
        <div className="ui-file-changes-heading">
          <b>已编辑 {visible.length} 个文件</b>
          {showLineCounts && <span className="ui-file-changes-total">
            <em>+{totalAdded}</em>
            <i>−{totalRemoved}</i>
          </span>}
        </div>
        <div className="ui-file-changes-actions">
          {onUndo && (
            <Button
              type="button"
              variant="subtle"
              size="xs"
              icon={<Undo2 size={13} />}
              title="撤销本轮全部文件改动"
              onClick={onUndo}
            >
              撤销
            </Button>
          )}
          {onReview && (
            <Button
              type="button"
              variant="secondary"
              size="xs"
              icon={<ArrowUpRight size={13} />}
              title="查看文件变更"
              onClick={onReview}
            >
              查看变更
            </Button>
          )}
        </div>
      </header>

      <div id={listId} className="ui-file-changes-files">
        {shown.map((file) => (
          <Button
            key={`${file.status}-${file.path}`}
            type="button"
            variant="bare"
            className={`ui-file-changes-file is-${file.status}`}
            title={file.path}
            onClick={() => onOpenFile?.(file.path)}
            onContextMenu={(event) => {
              if (!onRowContextMenu) return;
              event.preventDefault();
              onRowContextMenu(file.path, file.status, { x: event.clientX, y: event.clientY }, event.currentTarget);
            }}
            onKeyDown={(event) => {
              if (!onRowContextMenu) return;
              if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
              event.preventDefault();
              const rect = event.currentTarget.getBoundingClientRect();
              onRowContextMenu(file.path, file.status, { x: rect.left, y: rect.bottom }, event.currentTarget);
            }}
          >
            <span>{displayPath(file.path, workspacePath)}</span>
            {showLineCounts && <span className="ui-file-changes-stats">
              <em>+{file.addedLines || 0}</em>
              <i>−{file.removedLines || 0}</i>
            </span>}
          </Button>
        ))}
      </div>

      {hidden > 0 && (
        <Button
          type="button"
          variant="subtle"
          size="xs"
          className="ui-file-changes-expand"
          icon={expanded ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
          aria-controls={listId}
          aria-expanded={expanded}
          onClick={() => {
            setExpanded(!expanded);
            onExpandedChange?.(!expanded);
          }}
        >
          {expanded ? "收起文件" : `再显示 ${hidden} 个文件`}
        </Button>
      )}
    </Card>
  );
};
