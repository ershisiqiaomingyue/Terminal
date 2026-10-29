import React, { useCallback, useEffect, useId, useRef, useState } from "react";
import { ChevronRight, FolderOpen } from "lucide-react";
import { Button } from "./Button";
import type { FileChangeEntry, FileChangeStatus } from "./FileChangeCard";
import { NoticeFileList } from "./NoticeContent";

/**
 * FileChangeNotice
 * ---------------------------------------------------------------------------
 * The 文件改动 row shown above the composer. It is deliberately separate from
 * `NoticePanel`: this row also carries the +N −N summary.
 *
 * Interaction contract — no timers, no hover:
 *   - the summary row toggles on click
 *   - Escape collapses it
 *   - a click outside does NOT collapse it: it is a persistent layout row, not a
 *     transient popover
 *   - collapsed it is one pill; expanded the pill becomes a card and the list
 *     opens downward inside it. The card fill is opaque (`--notice-fill`), so the
 *     conversation behind it never shows through.
 */

export type FileChangeNoticeProps = {
  files: FileChangeEntry[];
  addedLines?: number;
  removedLines?: number;
  workspacePath?: string;
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  onOpenFile?: (path: string) => void;
  onRowContextMenu?: (
    path: string,
    status: FileChangeStatus,
    position: { x: number; y: number },
    trigger: HTMLElement,
  ) => void;
  /** Extra classes on the root, e.g. composer placement helpers. */
  className?: string;
};

export const FileChangeNotice: React.FC<FileChangeNoticeProps> = ({
  files,
  addedLines,
  removedLines,
  workspacePath,
  expanded: controlled,
  onExpandedChange,
  onOpenFile,
  onRowContextMenu,
  className,
}) => {
  const [uncontrolled, setUncontrolled] = useState(false);
  const expanded = controlled ?? uncontrolled;
  const contentId = useId();

  const totalAdded = addedLines ?? files.reduce((sum, file) => sum + (file.addedLines || 0), 0);
  const totalRemoved = removedLines ?? files.reduce((sum, file) => sum + (file.removedLines || 0), 0);

  const rootRef = useRef<HTMLDivElement>(null);

  const apply = useCallback(
    (next: boolean) => {
      if (controlled === undefined) setUncontrolled(next);
      onExpandedChange?.(next);
    },
    [controlled, onExpandedChange],
  );

  /** Escape collapses the list; inputs, menus and modals own their own Escape. */
  useEffect(() => {
    if (!expanded) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      const target = event.target;
      if (
        target instanceof Element &&
        target.closest(
          'input, textarea, [contenteditable="true"], [role="menu"], [role="listbox"], [role="alertdialog"], [aria-modal="true"]',
        )
      )
        return;
      event.preventDefault();
      rootRef.current?.querySelector<HTMLButtonElement>('[aria-expanded="true"]')?.focus({ preventScroll: true });
      apply(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [expanded, apply]);

  return (
    <div
      ref={rootRef}
      className={["ui-file-notice", expanded && "is-expanded", className].filter(Boolean).join(" ")}
    >
      <div className="ui-file-notice-row">
        <Button
          type="button"
          variant="bare"
          className="ui-file-notice-summary"
          aria-label="文件改动"
          aria-expanded={expanded}
          aria-controls={expanded ? contentId : undefined}
          onClick={() => apply(!expanded)}
        >
          <span className="ui-file-notice-icon" aria-hidden="true">
            <FolderOpen size={15} />
          </span>
          <span className="ui-file-notice-label">文件改动</span>
          <span className="ui-file-notice-diff">
            <em>+{totalAdded}</em>
            <i>−{totalRemoved}</i>
          </span>
          <small className="ui-file-notice-count">{files.length} 个文件</small>
          <ChevronRight size={14} className="ui-file-notice-chevron" aria-hidden="true" />
        </Button>
      </div>
      {expanded && (
        <div id={contentId} className="ui-file-notice-body" role="region" aria-label="文件改动列表">
          <NoticeFileList
            files={files}
            workspacePath={workspacePath}
            onOpen={onOpenFile}
            onRowContextMenu={onRowContextMenu}
          />
        </div>
      )}
    </div>
  );
};

export default FileChangeNotice;
