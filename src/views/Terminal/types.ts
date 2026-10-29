export interface TerminalTab {
  id: string;
  title: string;
  type: "adb" | "local" | "wsl" | "ssh";
  isRoot?: boolean;
  sessionId?: string;
  /** 本地终端用：powershell / pwsh / cmd */
  shell?: string;
  /** ssh / wsl 配置 id */
  profileId?: string;
  /** 会话的工作目录（本地为绝对路径，adb 为设备路径，ssh 为远程 root） */
  root?: string;
  /**
   * 终端打开时的工作目录（首次 cd 前捕获）。右栏「跟随终端目录」关闭时，
   * 文件/Git 面板锚定在这里而不是实时 root（v3 §22 三轮用户需求）。
   */
  rootAtOpen?: string;
}

export interface NewTerminalRequest {
  type: TerminalTab["type"];
  title: string;
  shell?: string;
  profileId?: string;
  root?: string;
}

export type DrawerPane = "host" | "git" | "files";

/** 中栏面板布局：单屏 / 左右双屏 / 四屏田字格。 */
export type TerminalLayout = "1x1" | "1x2" | "2v" | "2x2";

export const TERMINAL_LAYOUTS: readonly TerminalLayout[] = ["1x1", "1x2", "2v", "2x2"];

export function paneCount(layout: TerminalLayout): number {
  return layout === "1x1" ? 1 : layout === "1x2" || layout === "2v" ? 2 : 4;
}

/** 面板槽位 → 页签 id（null = 空占位），长度恒等于 paneCount(layout)。 */
export type PaneBindings = (string | null)[];

/** 把任意长度的绑定数组裁剪/补齐到目标布局所需的数量。 */
export function normalizeBindings(bindings: PaneBindings, layout: TerminalLayout): PaneBindings {
  const size = paneCount(layout);
  const next = bindings.slice(0, size);
  while (next.length < size) next.push(null);
  return next;
}

export function readTerminalLayout(value: unknown): TerminalLayout {
  return value === "1x2" || value === "2v" || value === "2x2" || value === "1x1" ? value : "1x1";
}

/**
 * 需要同时显示 `tabCount` 个终端时，最小能容纳的布局。
 *
 * 用途：`bindTabToPane` 在槽位全占满时先升布局，而不是把已有页签顶出去
 * （1x1 下顶掉就等于把上一个终端的面板卸载，用户看到"切到第 2 个第 1 个空了"）。
 * 布局只升不降，降级交给用户显式操作，避免窗口自己跳来跳去。
 */
export function layoutForCount(tabCount: number): TerminalLayout {
  if (tabCount <= 1) return "1x1";
  if (tabCount <= 2) return "1x2";
  return "2x2";
}

/** 升到能容纳 `tabCount` 个终端的布局；已经够大就原样返回（只升不降）。 */
export function upgradeLayoutFor(current: TerminalLayout, tabCount: number): TerminalLayout {
  const needed = layoutForCount(tabCount);
  const rank = (value: TerminalLayout) => (value === "1x1" ? 1 : value === "1x2" ? 2 : 4);
  return rank(needed) > rank(current) ? needed : current;
}

/**
 * 把占用往前挤、空格留到末尾，长度不变。
 *
 * 关掉中间某个终端后绑定会留出空洞（`["a", null, "c", "d"]`），空洞后面的终端
 * 就落在「非前 N 个」的位置上，`visibleLayout` 便无法收敛（收敛会把 c、d 藏掉）。
 * 关页签时顺手压实，用户看到的才是「关一个就少一格」，而不是「剩下 3 个却还画 4 格」。
 */
export function compactBindings(bindings: PaneBindings): PaneBindings {
  const kept: PaneBindings = bindings.filter((id): id is string => Boolean(id));
  while (kept.length < bindings.length) kept.push(null);
  return kept;
}

/**
 * 实际要画几格：**由当前占用数决定，而不是由残留的 `layout` 决定**。
 *
 * `layout` 只升不降，用户把 2x2 用剩 2 个终端后它仍是 2x2；再叠加「显式开空槽」
 * 标记，网格就会画出 4 格、下面两格是空闲占位板，而布局按钮亮的却是 1x2——
 * 用户看到的就是「只激活两个面板，却显示了 4 个」（m18580）。
 *
 * 规则：用户显式点过布局按钮（`showEmptySlots`）= 要的就是这个格数，原样返回；
 * 否则按占用数收敛到最小布局，多出来的空格不画。朝向（`2v` 上下 / `1x2` 左右）
 * 沿用 `layout`，避免收敛时把用户摆好的上下分栏翻成左右。
 */
export function visibleLayout(
  layout: TerminalLayout,
  occupiedPanes: number,
  showEmptySlots: boolean,
): TerminalLayout {
  if (showEmptySlots) return layout;
  if (occupiedPanes <= 1) return "1x1";
  if (occupiedPanes === 2) return layout === "2v" ? "2v" : "1x2";
  return "2x2";
}

/**
 * 「显式开空槽」这个意图什么时候该被收回。
 *
 * 它是**一次性**的：用户点布局按钮那一刻代表「给我这么多格子」，此后只要终端变少
 * （关页签、面板叉删、页签前移）就该重新收敛，否则很久以前点过的 2x2 会一直留着
 * 空格——只剩 2 个终端仍画 4 格，正是 m18580 的现场。
 *
 * 只收「变少」不收「变多」：用户刚点 2x2 就往空槽里拖终端时占用数会上升，这时若
 * 把标记收掉，网格会立刻缩成 1x2，刚铺开的空槽白点了。
 */
export function dropsEmptySlotIntent(previousOccupied: number, occupiedPanes: number): boolean {
  return occupiedPanes < previousOccupied;
}
