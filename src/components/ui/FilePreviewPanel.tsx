import React, { useState } from "react";
import { ArrowDown, ArrowUp, Check, Copy, RefreshCw, Search, X } from "lucide-react";
import { Button } from "./Button";

/**
 * FilePreviewPanel
 * ---------------------------------------------------------------------------
 * The shell around a read-only file preview: a header with the file name and
 * action buttons, an optional source/diff switch with +N −N counts, an optional
 * inline search bar, and a footer showing the full path.
 *
 * The body is a slot. The product's version reads bytes through a Tauri invoke
 * and syntax-highlights them; none of that belongs in the component library, so
 * the caller renders whatever it already has (plain <pre>, markdown, diff rows).
 */

export type PreviewMode = "source" | "diff";

export type FilePreviewPanelProps = {
  /** Full path; also the footer text. */
  path: string;
  /** Short name for the header. Derived from `path` when omitted. */
  name?: string;
  mode?: PreviewMode;
  onModeChange?: (mode: PreviewMode) => void;
  /** Shows the source/diff switch. Off when there is nothing to diff against. */
  showModes?: boolean;
  added?: number;
  removed?: number;
  /** Renders "部分差异" beside the counts. */
  partial?: boolean;
  searchable?: boolean;
  query?: string;
  onQueryChange?: (query: string) => void;
  /** 1-based position and total, shown as "2/7". */
  matchIndex?: number;
  matches?: number;
  onMoveMatch?: (offset: number) => void;
  onClose?: () => void;
  onCopy?: () => void;
  copied?: boolean;
  onRetry?: () => void;
  /** Disables actions while the body is still loading. */
  loading?: boolean;
  children?: React.ReactNode;
  footerNote?: React.ReactNode;
  /** Extra header control, e.g. a metadata popover, before the tool buttons. */
  headerExtra?: React.ReactNode;
  /**
   * Small note under the file name. Defaults to "只读预览"; pass a shorter or
   * environment-aware text in narrow panes.
   */
  titleNote?: string;
  /**
   * Fills a pane flush with its container: no rounding, no border, no shadow.
   * The container's own corners own the outer edge.
   */
  docked?: boolean;
  className?: string;
};

export const FilePreviewPanel = React.forwardRef<HTMLElement, FilePreviewPanelProps>(({
  path,
  name,
  mode = "source",
  onModeChange,
  showModes = false,
  added = 0,
  removed = 0,
  partial = false,
  searchable = false,
  query: controlledQuery,
  onQueryChange,
  matchIndex = 0,
  matches = 0,
  onMoveMatch,
  onClose,
  onCopy,
  copied = false,
  onRetry,
  loading = false,
  children,
  footerNote,
  headerExtra,
  titleNote = "只读预览",
  docked = false,
  className,
}, ref) => {
  const [searchOpen, setSearchOpen] = useState(false);
  const [internalQuery, setInternalQuery] = useState("");
  const query = controlledQuery ?? internalQuery;
  const setQuery = onQueryChange ?? setInternalQuery;
  const shortName = name ?? path.split(/[\\/]/).filter(Boolean).pop() ?? path;

  const toggleSearch = () => {
    setSearchOpen((open) => !open);
    setQuery("");
  };

  return (
    <aside
      ref={ref}
      className={["ui-preview", docked && "is-docked", className].filter(Boolean).join(" ")}
      aria-label={`文件预览：${shortName}`}
      onKeyDown={(event) => {
        if (searchable && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
          event.preventDefault();
          event.stopPropagation();
          setSearchOpen(true);
          event.currentTarget.querySelector<HTMLInputElement>(".ui-preview-search-input")?.focus();
        }
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          if (searchOpen) { setSearchOpen(false); setQuery(""); }
          else onClose?.();
        }
      }}
    >
      <header className="ui-preview-header">
        <div className="ui-preview-title">
          <strong title={path || shortName}>{shortName}</strong>
          {titleNote ? <small title={titleNote}>{titleNote}</small> : null}
        </div>
        <div className="ui-preview-tools">
          {headerExtra}
          {searchable && (
            <Button
              type="button"
              variant="subtle"
              size="xs"
              className="ui-preview-tool"
              icon={<Search size={14} />}
              title="搜索文件内容"
              aria-label="搜索文件内容"
              aria-expanded={searchOpen}
              disabled={loading}
              onClick={toggleSearch}
            />
          )}
          {onCopy && (
            <Button
              type="button"
              variant="subtle"
              size="xs"
              className="ui-preview-tool"
              icon={copied ? <Check size={14} /> : <Copy size={14} />}
              title={copied ? "已复制" : "复制文件内容"}
              aria-label={copied ? "已复制" : "复制文件内容"}
              disabled={loading}
              onClick={onCopy}
            />
          )}
          {onRetry && (
            <Button
              type="button"
              variant="subtle"
              size="xs"
              className="ui-preview-tool"
              icon={<RefreshCw size={14} />}
              title="重新读取文件"
              aria-label="重新读取文件"
              onClick={onRetry}
            />
          )}
          {onClose && (
            <Button
              type="button"
              variant="subtle"
              size="xs"
              className="ui-preview-tool"
              icon={<X size={16} />}
              title="关闭文件预览"
              aria-label="关闭文件预览"
              onClick={onClose}
            />
          )}
        </div>
      </header>

      {showModes && (
        <div className="ui-preview-modes">
          <div role="group" aria-label="文件显示模式">
            {(["source", "diff"] as const).map((value) => (
              <Button
                key={value}
                type="button"
                variant="subtle"
                size="xs"
                className={`ui-preview-mode${mode === value ? " is-active" : ""}`}
                aria-pressed={mode === value}
                onClick={() => onModeChange?.(value)}
              >
                {value === "source" ? "源码" : "差异"}
              </Button>
            ))}
          </div>
          {mode === "diff" && (
            <span className="ui-preview-counts" title="本次修改与当前磁盘文件的差异">
              <b>+{added}</b>
              <i>−{removed}</i>
              {partial && <small>部分差异</small>}
            </span>
          )}
        </div>
      )}

      {searchOpen && (
        <div className="ui-preview-search" role="search">
          <input
            autoFocus
            type="text"
            className="ui-preview-search-input"
            aria-label="文件内查找"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                onMoveMatch?.(event.shiftKey ? -1 : 1);
              }
            }}
          />
          <span aria-live="polite">
            {matches ? `${matchIndex + 1}/${matches}` : query ? "无匹配" : "0/0"}
          </span>
          <Button
            type="button"
            variant="subtle"
            size="xs"
            className="ui-preview-tool"
            icon={<ArrowUp size={13} />}
            title="上一个匹配"
            aria-label="上一个匹配"
            disabled={!matches}
            onClick={() => onMoveMatch?.(-1)}
          />
          <Button
            type="button"
            variant="subtle"
            size="xs"
            className="ui-preview-tool"
            icon={<ArrowDown size={13} />}
            title="下一个匹配"
            aria-label="下一个匹配"
            disabled={!matches}
            onClick={() => onMoveMatch?.(1)}
          />
        </div>
      )}

      <div className="ui-preview-content">{children}</div>

      {footerNote}

      <footer className="ui-preview-path" title={path}>
        {path}
      </footer>
    </aside>
  );
});
