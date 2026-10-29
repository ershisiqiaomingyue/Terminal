import type { ReactNode } from "react";
import type { FileDiffRow } from "./fileDiff";

/** Shared, scrollable unified diff body for file previews. */
export function FileDiffView({ rows, truncated = false, renderLine }: {
  rows: FileDiffRow[];
  truncated?: boolean;
  renderLine?: (text: string, index: number) => ReactNode;
}) {
  return <div className="ui-file-diff" data-file-search-root tabIndex={0} aria-label="文件差异内容">
    {rows.length ? <div className="ui-file-diff-rows">{rows.map((row, index) =>
      <div key={index} className={`ui-file-diff-row is-${row.kind}`}>
        <span className="ui-file-diff-number" data-file-search-ignore>{row.oldLine ?? ""}</span>
        <span className="ui-file-diff-number" data-file-search-ignore>{row.newLine ?? ""}</span>
        <span className="ui-file-diff-marker" data-file-search-ignore>{row.kind === "added" ? "+" : row.kind === "removed" ? "−" : " "}</span>
        <code>{renderLine ? renderLine(row.text, index) : row.text}{"\n"}</code>
      </div>)}</div> : <p className="ui-file-diff-notice">没有文本差异</p>}
    {truncated && <p className="ui-file-diff-notice">差异较大，预览已截断。</p>}
  </div>;
}
