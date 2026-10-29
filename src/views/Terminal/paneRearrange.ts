import { normalizeBindings, paneCount, type PaneBindings, type TerminalLayout } from "./types.ts";

/**
 * 把某个已绑定的页签挪到目标槽位（纯函数，有单测）。
 *
 * 为什么不是简单裁剪：切 1x2 → 1x1 时，`normalizeBindings` 只保留槽 0；
 * 若用户激活的是槽 1 的终端，被裁掉的正是他要看的那个，剩下的反而是
 * 后台那个——「点单格按钮，结果显示了另一个终端」。
 *
 * 这里把目标页签搬到槽 0 再裁剪，保证切到小布局后留下的就是用户点的那一个；
 * 其余页签仍在左栏列表里，随时可以再拖回来。
 */
export function promoteTabToFirstSlot(
  bindings: PaneBindings,
  tabId: string | null | undefined,
  layout: TerminalLayout,
): PaneBindings {
  const size = paneCount(layout);
  if (!tabId) return normalizeBindings(bindings, layout);
  // 顺序很要紧：必须先找到页签再裁剪。先归一化会在 1x1 下把它直接裁掉，
  // 后面 indexOf 只能返回 -1——正是「点单格，结果留下的是另一个终端」的复现。
  const from = bindings.indexOf(tabId);
  if (from <= 0) return normalizeBindings(bindings, layout);
  const rest = bindings.filter((id) => id !== tabId);
  return normalizeBindings([tabId, ...rest], layout);
}

/** 某个槽位在当前布局下是否真的占着屏幕（被比例覆盖压扁的格子不算）。 */
export function isSlotVisible(
  slot: number,
  layout: TerminalLayout,
  colRatios: readonly number[],
  rowRatios: readonly number[],
): boolean {
  const perRow = layout === "1x1" || layout === "2v" ? 1 : 2;
  const rowIndex = Math.floor(slot / perRow);
  const colIndex = slot % perRow;
  if ((layout === "2v" || layout === "2x2") && rowIndex === 1 && (rowRatios[0] ?? 0.5) >= 0.99) {
    return false;
  }
  if (layout === "2v" || layout === "2x2") {
    if (rowIndex === 0 && (rowRatios[0] ?? 0.5) <= 0.01) return false;
  }
  if (layout === "1x2" || layout === "2x2") {
    const value = colRatios[rowIndex] ?? 0.5;
    if (colIndex === 1 && value >= 0.99) return false;
    if (colIndex === 0 && value <= 0.01) return false;
  }
  return true;
}

/** SSH 断连后的自动重试节奏：每 10 秒一次，共 10 次。 */
export const SSH_RETRY_INTERVAL_MS = 10_000;
export const SSH_RETRY_MAX_ATTEMPTS = 10;

export type SshRetryPhase =
  /** 正在按节奏自动重试（第 1..10 次）。 */
  | "retrying"
  /** 10 次都没成功，界面停在这里等用户敲键盘再触发。 */
  | "waiting-input";

export interface SshRetryState {
  tabId: string;
  phase: SshRetryPhase;
  /** 已经发起的重试次数（0..SSH_RETRY_MAX_ATTEMPTS）。 */
  attempts: number;
  /** 下一次自动重试的时间戳；null 表示不会再自动重试。 */
  nextAt: number | null;
  /** 最近一次失败原因（给面板内提示条显示）。 */
  lastError: string;
}

/**
 * 重试耗尽后用户敲了键：**任何按键都算**「我要重连」。
 *
 * 用户明确要求「就是要 enter 啊，任何按键都可以」——断开的会话里按键本来就没有
 * 接收方，所以回车、方向键、Ctrl 组合键全都当成重连意图，不再挑可打印字符。
 * 唯一排除的是空串（xterm 在焦点切换等场合会投递空数据，那不代表用户按键）以及
 * 单纯的焦点/鼠标上报序列（SGR、DECCKM 这类**不产生输入**的转义），否则鼠标划过
 * 面板就会触发重连。
 */
export function shouldReconnectOnInput(data: string): boolean {
  if (!data) return false;
  // 鼠标上报（SGR `\u001b[<…M` / X10 `\u001b[M`）、窗口尺寸上报（`\u001b[8;…t`）与
  // 焦点变化（`\u001b[I` / `\u001b[O`）是内核自己发的事件，不是用户按键。
  const withoutEvents = data.replace(
    // eslint-disable-next-line no-control-regex
    /\u001b\[(?:<[\d;]*[Mm]|M[\s\S]{3}|8;[\d;]*t|[IO])/g,
    "",
  );
  return withoutEvents.length > 0;
}
