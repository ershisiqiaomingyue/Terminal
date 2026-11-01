import React from "react";
import { Check, Circle, CircleHelp, CircleMinus, CirclePause, X } from "lucide-react";
import { Spinner } from "rain-ui";
import type { ToolGroupPresentationState } from "./timelinePresentation";

const STATUS_LABELS: Record<ToolGroupPresentationState, string> = {
  done: "成功",
  error: "失败",
  cancelled: "已取消",
  interrupted: "已中断",
  unknown: "结果未保存",
  skipped: "未执行",
  pending: "等待执行",
  running: "正在执行",
};

/**
 * 现代轻质单圆圈状态指示器：
 * - done: 实心圆圈背景 + 白色对勾 (做了就打勾)
 * - running: 滚动加载圆弧 (正在做就滚动)
 * - pending: 纯净实线单圆圈描边 (没做的就空心圈)
 * - error/cancelled: 错误红色圆圈 + 白色 X
 */
export function ToolActivityStatus({ state }: { state: ToolGroupPresentationState }) {
  const label = STATUS_LABELS[state] || state;
  return (
    <span
      className={`agent-activity-status activity-status-icon ${state}`}
      data-state={state}
      role="img"
      aria-hidden={false}
      aria-label={label}
    >
      {state === "done" && <Check size={11} strokeWidth={2.8} />}
      {state === "running" && <Spinner size={12} strokeWidth={2.6} />}
      {state === "pending" && <Circle size={12} strokeWidth={2.2} />}
      {(state === "error" || state === "cancelled") && <X size={11} strokeWidth={2.8} />}
      {state === "interrupted" && <CirclePause size={12} strokeWidth={2.2} />}
      {state === "skipped" && <CircleMinus size={12} strokeWidth={2.2} />}
      {state === "unknown" && <CircleHelp size={12} strokeWidth={2.2} />}
    </span>
  );
}
