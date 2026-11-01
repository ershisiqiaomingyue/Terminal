/**
 * 面板内联提示条（单面板作用域，替代全局 toast）。
 *
 * 为什么要它：同一个终端页开了两块/四块面板时，「正在连接」「已连接」「连接失败」
 * 走全局 `notify` 会飘到整个窗口右上角——用户看到了别处的提示，却不知道是哪一块
 * 面板出的问题。这里把提示渲染进 `.terminal-pane` 内部（顶部/右上角），只有出问题
 * 的那块面板显示，其它面板看不见。
 *
 * 状态完全由父级持有（父级掌握重试计时器与 tabId），本组件只负责画。
 */
import React from "react";
import { Button } from "rain-ui";
import { AlertTriangle, CheckCircle2, Info, RotateCw, X } from "lucide-react";
import type { SshRetryState } from "./paneRearrange";
import { SSH_RETRY_MAX_ATTEMPTS } from "./paneRearrange";

export type PaneNoticeKind = "info" | "success" | "error";

export interface PaneNotice {
  id: number;
  text: string;
  kind: PaneNoticeKind;
  /** 重试中的倒计时文案（可选），由父级按状态拼好。 */
}

export interface PaneBannerProps {
  notice: PaneNotice | null;
  retry: SshRetryState | null;
  /** 距离下次自动重试还有几秒（纯展示，父级每秒刷新一次）。 */
  retryInSeconds?: number;
  onDismissNotice: () => void;
  /** 立刻重试（不等倒计时）。 */
  onRetryNow: () => void;
  /** 关掉重试状态（放弃自动重试，保留页签）。 */
  onDismissRetry: () => void;
}

const NOTICE_ICON: Record<PaneNoticeKind, React.ReactNode> = {
  info: <Info size={12} />,
  success: <CheckCircle2 size={12} />,
  error: <AlertTriangle size={12} />,
};

export const PaneBanner: React.FC<PaneBannerProps> = ({
  notice,
  retry,
  retryInSeconds,
  onDismissNotice,
  onRetryNow,
  onDismissRetry,
}) => {
  if (!notice && !retry) return null;
  // 重试状态优先：它带着按钮和进度，比普通提示更重要。
  if (retry) {
    const exhausted = retry.phase === "waiting-input";
    return (
      <div
        // 重试中只是「正在处理」，用警告色；次数用尽才算这次没能自愈，转成错误色。
        className={`terminal-pane-banner ${exhausted ? "is-error" : "is-warning"}`}
        role="status"
        // 面板主体的 onMouseDown 会把焦点抢回 xterm；这里拦住，按钮才点得动。
        onMouseDown={(event) => event.stopPropagation()}
      >
        <span className="terminal-pane-banner-icon">
          <AlertTriangle size={12} />
        </span>
        <span className="terminal-pane-banner-text">
          {exhausted
            ? `自动重连 ${retry.attempts} 次仍未成功，直接敲键盘即可继续重连。`
            : `连接已断开，正在重连（第 ${Math.min(retry.attempts + 1, SSH_RETRY_MAX_ATTEMPTS)}/${SSH_RETRY_MAX_ATTEMPTS} 次${
                retryInSeconds === undefined ? "" : `，${retryInSeconds}s 后`
              }）…`}
          {retry.lastError ? <small>{retry.lastError}</small> : null}
        </span>
        <Button className="terminal-pane-banner-action" type="button" variant="bare" onClick={onRetryNow}>
          <RotateCw size={11} /> 立即重连
        </Button>
        <Button
          className="terminal-pane-banner-close"
          type="button" variant="bare"
          onClick={onDismissRetry}
          aria-label="停止自动重连"
          title="停止自动重连"
        >
          <X size={11} />
        </Button>
      </div>
    );
  }
  const current = notice!;
  return (
    <div
      className={`terminal-pane-banner is-${current.kind}`}
      role="status"
      onMouseDown={(event) => event.stopPropagation()}
    >
      <span className="terminal-pane-banner-icon">{NOTICE_ICON[current.kind]}</span>
      <span className="terminal-pane-banner-text">{current.text}</span>
      <Button
        className="terminal-pane-banner-close"
        type="button" variant="bare"
        onClick={onDismissNotice}
        aria-label="关闭提示"
        title="关闭提示"
      >
        <X size={11} />
      </Button>
    </div>
  );
};
