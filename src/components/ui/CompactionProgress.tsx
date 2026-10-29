import React from "react";
import { AlertCircle, Check, CircleDashed } from "lucide-react";
import { Spinner } from "./Spinner";

/**
 * CompactionProgress
 * ---------------------------------------------------------------------------
 * The context-compaction strip that sits above the composer. It is NOT a
 * NoticePanel: it has no header to click and no collapse. It is a single status
 * line plus a determinate track.
 *
 * Running and completed are one continuous strip, not two different widgets:
 * the same row, the same track and the same percent slot, so the moment the
 * compaction finishes the line does not jump, resize or re-order — the spinner
 * becomes a tick and the track settles at 100%. `detail` carries the sentence
 * the caller builds, e.g. "12,400 → 3,200 Token" when finished, or the message
 * counts while running.
 */

export type CompactionStatus = "running" | "completed" | "interrupted" | "failed";

export type CompactionProgressProps = {
  status: CompactionStatus;
  /** 0-100. Omitted for states that cannot report progress. */
  progress?: number | null;
  /** Secondary sentence under the label. */
  detail?: React.ReactNode;
  className?: string;
  /**
   * `strip`（默认）= 独立状态条，带进度轨。
   * `row` = 作为时间线里的一行渲染：没有外框与进度轨，与「执行指令」那种
   * 工具组行同高、同缩进，只留图标 + 标签 + 明细 + 百分比文字。
   */
  variant?: "strip" | "row";
};

const LABEL: Record<CompactionStatus, string> = {
  running: "正在压缩上下文",
  completed: "上下文已压缩",
  interrupted: "上下文压缩已中断",
  failed: "上下文压缩失败",
};

const ICON: Record<CompactionStatus, React.ReactNode> = {
  running: <Spinner size={14} />,
  // Same check as the tool-status circle: 11px, heavy stroke, white on a
  // filled accent circle (the fill comes from CSS, not from the glyph).
  completed: <Check size={11} strokeWidth={2.8} />,
  interrupted: <CircleDashed size={14} />,
  failed: <AlertCircle size={14} />,
};

export const CompactionProgress: React.FC<CompactionProgressProps> = ({
  status,
  progress,
  detail,
  className,
  variant = "strip",
}) => {
  const row = variant === "row";
  const showTrack = status === "running" || status === "completed";
  // A running compaction whose backend has not reported a percentage yet still
  // shows the track: it sweeps instead of jumping from nothing to a number.
  const indeterminate = status === "running" && progress == null;
  const value = status === "completed" ? 100 : Math.min(100, Math.max(0, progress ?? 0));
  const percentText = indeterminate ? "…" : `${Math.round(value)}%`;

  return (
    <div
      className={["ui-compaction", `is-${status}`, row ? "is-row" : "", className].filter(Boolean).join(" ")}
      data-status={status}
      role="status"
      aria-live="polite"
    >
      <div className="ui-compaction-copy">
        <span className="ui-compaction-icon" aria-hidden="true">{ICON[status]}</span>
        <span className="ui-compaction-label">{LABEL[status]}</span>
        {row && detail && <small className="ui-compaction-detail">{detail}</small>}
      </div>
      {!row && showTrack && (
        <>
          <div
            className={`ui-compaction-track${indeterminate ? " is-indeterminate" : ""}`}
            role="progressbar"
            aria-label={LABEL[status]}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={indeterminate ? undefined : Math.round(value)}
          >
            <span style={indeterminate ? undefined : { width: `${value}%` }} />
          </div>
          <span className="ui-compaction-percent">{percentText}</span>
        </>
      )}
      {row && showTrack && <span className="ui-compaction-percent">{percentText}</span>}
      {!row && detail && <small className="ui-compaction-detail">{detail}</small>}
    </div>
  );
};

export default CompactionProgress;
