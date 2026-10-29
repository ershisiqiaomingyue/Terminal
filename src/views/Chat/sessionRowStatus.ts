/**
 * 侧栏会话行的状态与「可见行数」推导（纯函数，可单测）。
 *
 * 状态优先级照 dsh 的 `sessionStatuses`：待处理交互 > 运行中 > 空闲。待处理交互
 * 分成「提问」和「审批」两类，行尾用紧凑文案替换更新时间，读者在侧栏就能看出
 * 这条对话是在等我回答还是在等我点允许。
 *
 * 可见行数同样照 dsh 的水位线：每组默认只露 5 行，但置顶、当前会话、带待处理
 * 交互的行会被自动抬到可见区，否则「有东西等你」的那一行可能正好被折叠掉。
 */

export const SESSION_WATERMARK_BASE = 5;
/** 自动抬升的最高水位；再往后靠「展开其余」按钮，避免长列表把侧栏撑爆。 */
export const SESSION_WATERMARK_AUTO_MAX = 50;

export type RowState = "warning" | "ongoing" | "idle";

/** 会话行的主状态；`trailingLabel` 存在时替换行尾的相对时间。 */
export interface SessionRowStatus {
  state: RowState;
  label: string;
  trailingLabel?: string;
}

/** 运行状态视图：phase 为空表示没有活动，pending 只在 waiting 时有意义。 */
export interface RunStatusView {
  phase: "running" | "waiting";
  pending?: "question" | "approval";
}

/**
 * 把运行状态翻成行的显示状态。
 * 待处理交互压过运行中：一个回合还在跑，但工具在等你回答时，行尾必须说「待回答」。
 */
export function sessionRowStatus(status: RunStatusView | undefined): SessionRowStatus {
  if (status?.phase === "waiting") {
    return status.pending === "question"
      ? { state: "warning", label: "等待回答", trailingLabel: "待回答" }
      : { state: "warning", label: "等待审批", trailingLabel: "待审批" };
  }
  if (status?.phase === "running") return { state: "ongoing", label: "进行中" };
  return { state: "idle", label: "空闲" };
}

/**
 * 一组里先露几行。
 *
 * 基准水位线是 5，任何置顶 / 当前 / 待处理交互的行都会把水位抬到能看见它（上限
 * `SESSION_WATERMARK_AUTO_MAX`）；超过水位线时，多出来的行靠调用方渲染的
 * 「展开其余 N 个会话」按钮补上。
 */
export function visibleSessionLimit<T extends { id: string; pinned?: boolean }>(
  sessions: readonly T[],
  currentId: string,
  isAttention: (session: T) => boolean,
  base = SESSION_WATERMARK_BASE,
): number {
  let required = base;
  for (const [index, session] of sessions.entries()) {
    if (index > SESSION_WATERMARK_AUTO_MAX) break;
    const auto = Boolean(session.pinned) || session.id === currentId || isAttention(session);
    if (auto) required = Math.max(required, index + 1);
  }
  return Math.min(sessions.length, required);
}
