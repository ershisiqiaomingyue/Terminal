/**
 * 多面板窗口管理纯逻辑层（补位 / 换位 / 抢占 / 落点判定）。
 *
 * 单独成文件的原因和 `floatingGeometry.ts` 一样：多面板里真正容易出事故的是
 * 「关掉一个面板后剩下的是不是自动前移补位」「拖拽换位会不会把某个页签弄丢」
 * 「抢占后 bindings 长度和布局对不对得上」这类纯状态迁移，放在 tsx 里就只能
 * 靠手拖验证。这里全部是纯函数，可以用
 * `node --experimental-strip-types --test tests/paneLayout.test.mts` 直接跑。
 *
 * 约束（也是接线时的约定）：
 * - 纯函数：不 mutate 入参、不读 localStorage、不开定时器、不 import React；
 * - 不 import 任何 `@/` 别名路径（Node 单测不认别名），一律相对路径 + `.ts` 后缀；
 * - 越界槽位、非法落点一律**降级返回**（返回原值的拷贝 / 空计划），绝不抛异常，
 *   因为拖拽事件里抛一次就会把整个面板树炸掉。
 *
 * 槽位顺序沿用 `types.ts` 的网格阅读顺序：0=左上、1=右上、2=左下、3=右下。
 * 在 `1x2` 下就是 0=左栏、1=右栏；`2x2` 下是田字格四格。
 */

import { layoutForCount, normalizeBindings, paneCount } from "./types.ts";
import type { PaneBindings, TerminalLayout } from "./types.ts";

/** 槽位下标 0..3，按网格阅读顺序：左上、右上、左下、右下。 */
export type PaneSlot = number;

/** 网格最多 4 格（`2x2`），下标上限就是它。 */
export const MAX_PANE_SLOTS = 4;

/** 全部合法槽位，渲染时按顺序铺。 */
export const PANE_SLOTS: readonly PaneSlot[] = [0, 1, 2, 3];

/** 布局对应的网格形状。 */
export interface PaneGrid {
  layout: TerminalLayout;
  cols: number;
  rows: number;
}

/** 落点区域：四条边是「抢占」区，中心是「交换」区。 */
export type DropZone = "left" | "right" | "top" | "bottom" | "center";

export const DROP_ZONES: readonly DropZone[] = ["left", "right", "top", "bottom", "center"];

/**
 * `hitZone` 的默认边带比例。
 *
 * 语义：**两侧边缘带合计占该轴的比例**。0.5 表示每条边各占 25%，中间 50%×50%
 * 的方块是「交换」区。改成 0 → 永远返回 center；改成 1 → 永远返回某条边。
 */
export const DEFAULT_EDGE_RATIO = 0.5;

const GRID_DEFS: Record<TerminalLayout, { cols: number; rows: number }> = {
  "1x1": { cols: 1, rows: 1 },
  "1x2": { cols: 2, rows: 1 },
  "2v": { cols: 1, rows: 2 },
  "2x2": { cols: 2, rows: 2 },
};

/** 布局 → 网格形状。非法入参降级成 `1x1` 的形状，但不改 `layout` 字段本身。 */
export function gridOf(layout: TerminalLayout): PaneGrid {
  const def = GRID_DEFS[layout] ?? GRID_DEFS["1x1"];
  return { layout, cols: def.cols, rows: def.rows };
}

/** 由 bindings 长度反推当前布局（调用方不一定把 layout 传进来）。 */
export function layoutOfBindings(bindings: PaneBindings): TerminalLayout {
  return layoutForCount(bindings.length);
}

/** 槽位是否落在长度为 `size` 的 bindings 内。 */
function isSlot(slot: PaneSlot, size: number): boolean {
  return Number.isInteger(slot) && slot >= 0 && slot < size && slot < MAX_PANE_SLOTS;
}

/** 把槽位上的值看成「已占用」：null / undefined / 空串都算空槽。 */
function isOccupied(value: string | null | undefined): value is string {
  return typeof value === "string" && value.length > 0;
}

const toSlotList = (size: number): PaneSlot[] =>
  Array.from({ length: size }, (_unused, index) => index);

/**
 * 关闭某槽位后其余槽位前移补位，空槽归 null。
 *
 * 长度保持不变（等于当前布局的槽位数），所以关掉中间某个面板后，后面的面板会
 * 依次往前顶一格，最后留一个 null —— 这正是「补位」要的效果。
 */
export function compactBindings(bindings: PaneBindings): PaneBindings {
  const size = bindings.length;
  const kept = bindings.filter(isOccupied);
  const next: (string | null)[] = kept.slice(0, size);
  while (next.length < size) next.push(null);
  return next;
}

/**
 * 拖拽换位：把 from 槽位的绑定移到 to 槽位（两者直接交换）。
 *
 * from === to、或任一槽位越界时返回原绑定数组的**拷贝**（调用方可以放心 setState）。
 */
export function swapBindings(bindings: PaneBindings, from: PaneSlot, to: PaneSlot): PaneBindings {
  const next: PaneBindings = bindings.slice();
  const size = bindings.length;
  if (!isSlot(from, size) || !isSlot(to, size) || from === to) return next;
  const moved = next[from];
  next[from] = next[to];
  next[to] = moved;
  return next;
}

const clamp01 = (value: number): number =>
  Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0.5;

/**
 * 指针在槽位内的相对位置（0..1）判定落点：中心方块以外为对应边的抢占区。
 *
 * `edgeRatio` 是两侧边带合计占该轴的比例（默认 0.5 → 每边 25%）。落在中心方块里
 * 返回 `center`（交换）；某个轴上已经出了中心带、另一轴还在中心带里，就由出去的
 * 那个轴决定方向；两轴都出去了取偏移更大的那个轴，正好相等时优先左右。
 * 入参先夹到 0..1，NaN/Infinity 视为 0.5（回到中心），函数永不抛。
 */
export function hitZone(xRatio: number, yRatio: number, edgeRatio: number = DEFAULT_EDGE_RATIO): DropZone {
  const x = clamp01(xRatio);
  const y = clamp01(yRatio);
  const edge = clamp01(edgeRatio);
  // 中心方块半宽：edgeRatio=0.5 → 0.25，即中间 50% 宽高的方块。
  const centerHalf = (1 - edge) / 2;
  const dx = x - 0.5;
  const dy = y - 0.5;
  const inCenterX = Math.abs(dx) <= centerHalf;
  const inCenterY = Math.abs(dy) <= centerHalf;

  if (inCenterX && inCenterY) return "center";
  if (inCenterX) return dy < 0 ? "top" : "bottom";
  if (inCenterY) return dx < 0 ? "left" : "right";
  if (Math.abs(dx) >= Math.abs(dy)) return dx < 0 ? "left" : "right";
  return dy < 0 ? "top" : "bottom";
}

/** 抢占（缩放拖拽）的结果。 */
export interface ResizePlan {
  /** 抢占后的布局。边带落点固定收敛到 `1x2`（`2x2` 的抢占就是一次「折叠成两栏」）。 */
  layout: TerminalLayout;
  /** 新 bindings，长度恒等于 `paneCount(layout)`。 */
  bindings: PaneBindings;
  /** 被挤出网格的页签 id（按原槽位顺序）。调用方必须把它们恢复成页签，不能丢。 */
  displaced: string[];
}

/** 原样返回一个「什么都没变」的计划（长度按当前布局归一化）。 */
function unchangedPlan(bindings: PaneBindings): ResizePlan {
  const layout = layoutOfBindings(bindings);
  return { layout, bindings: normalizeBindings(bindings.slice(), layout), displaced: [] };
}

/**
 * 缩放抢占：把 from 槽位拖到 to 槽位的某条边。
 *
 * 规则（接线时按这个实现 UI）：
 * - `center`：只交换 from / to 两个槽位的绑定，布局不变。
 * - `left` / `top`：**被拖的窗口落在拖到的那一侧**（占新布局的第 0 槽），目标窗口
 *   退到对侧（第 1 槽）。
 * - `right` / `bottom`：目标窗口占第 0 槽，被拖的窗口占第 1 槽（同样是被拖窗口在
 *   拖到的那一侧）。
 * - 结果布局固定是 `1x2`：`TerminalLayout` 只有左右分栏、没有上下分栏，所以
 *   top/bottom 也只能收敛成左右两栏（槽位先后按拖入方向排）。
 * - 原来 `2x2` 里的另外两个面板**一个都不会消失**，全部按原槽位顺序进 `displaced`，
 *   由调用方把它们退回页签列表。
 *
 * 无效输入（槽位越界、from === to、from 是空槽、落点非法）→ 返回原样计划，不抛。
 */
export function planResize(
  bindings: PaneBindings,
  from: PaneSlot,
  to: PaneSlot,
  zone: DropZone,
): ResizePlan {
  const size = bindings.length;
  if (!isSlot(from, size) || !isSlot(to, size) || from === to) return unchangedPlan(bindings);
  const dragged = bindings[from];
  // 空槽拖不动：没有窗口就没有「抢占」。
  if (!isOccupied(dragged)) return unchangedPlan(bindings);

  const isEdge = zone === "left" || zone === "right" || zone === "top" || zone === "bottom";
  if (!isEdge) {
    // center（以及任何非法落点）→ 只交换。
    const layout = layoutOfBindings(bindings);
    return {
      layout,
      bindings: normalizeBindings(swapBindings(bindings, from, to), layout),
      displaced: [],
    };
  }

  const target = bindings[to] ?? null;
  const draggedFirst = zone === "left" || zone === "top";
  const nextBindings: PaneBindings = draggedFirst ? [dragged, target] : [target, dragged];

  // 其余页签按原槽位顺序挤出来 → displaced。用 compact 把 null 挤掉，序不变。
  const rest = bindings.map((id, index) => (index === from || index === to ? null : id));
  const displaced = compactBindings(rest).filter(isOccupied);

  return { layout: "1x2", bindings: nextBindings, displaced };
}

/**
 * 目标槽位为空时是否可以放入（用户要求：当前窗口已有终端则不允许放置）。
 *
 * 只判断目标槽本身：空槽 → true；已有会话 → false；越界 → false。
 */
export function canAcceptDrop(bindings: PaneBindings, slot: PaneSlot): boolean {
  if (!isSlot(slot, bindings.length)) return false;
  return !isOccupied(bindings[slot]);
}

/**
 * 该槽位是否正有会话在执行任务，需要弹「会打断」确认。busy 由调用方传入
 * （通常来自每个面板的 running 状态），这里只做安全取下标。
 */
export function needInterruptConfirm(busy: readonly boolean[], slot: PaneSlot): boolean {
  if (!Number.isInteger(slot) || slot < 0 || slot >= busy.length || slot >= MAX_PANE_SLOTS) {
    return false;
  }
  return busy[slot] === true;
}

/** 进入界面时的默认绑定：不开任何终端，返回全 null 的 bindings。 */
export function emptyBindings(layout: TerminalLayout): PaneBindings {
  const size = paneCount(layout);
  return toSlotList(size).map(() => null);
}

// ---------------------------------------------------------------------------
// 拖边线缩放 / 拖过一半抢占（用户需求 ⑤ ⑥ ⑦）
// ---------------------------------------------------------------------------

/** 两侧面板宽度比的下限：再窄就看不清终端内容了，拖到这里就停住。 */
export const PANE_MIN_RATIO = 0.2;

/**
 * 越过这条线就判定为「抢占」（v3 定稿：拖过邻块宽度的一半即提示）。
 *
 * 从对半（0.5）开始拖，邻块宽 = 1-r；吃掉邻块一半以上 ⇔ r > 0.75。
 * 中间 0.25..0.75 是普通缩放区间，留缓冲避免手一抖就触发确认框。
 */
export const PANE_CLAIM_THRESHOLD = 0.75;

/** 分栏比例夹在 [PANE_MIN_RATIO, 1 - PANE_MIN_RATIO]，非法值回落到 0.5。 */
export function clampPaneRatio(value: number): number {
  if (!Number.isFinite(value)) return 0.5;
  const low = PANE_MIN_RATIO;
  const high = 1 - PANE_MIN_RATIO;
  if (value < low) return low;
  if (value > high) return high;
  return value;
}

/**
 * 根据拖拽位移算新的分栏比例。
 *
 * `startRatio` 是拖拽开始时的比例，`deltaPx` 是相对起点的位移（向右/向下为正），
 * `axisSize` 是该轴总像素宽/高。轴尺寸非法（0 / NaN）时原样返回起点，避免除零。
 */
export function paneRatioFromPointer(startRatio: number, deltaPx: number, axisSize: number): number {
  const start = clampPaneRatio(startRatio);
  if (!Number.isFinite(deltaPx) || !Number.isFinite(axisSize) || axisSize <= 0) return start;
  return clampPaneRatio(start + deltaPx / axisSize);
}

/** 浮点比较容差：`1 - 0.8` 在 IEEE754 下是 0.19999999999999996，不是 0.2。 */
const RATIO_EPSILON = 1e-9;

/**
 * 该比例是否已经越过抢占阈值，以及是哪一块赢。
 *
 * 返回 `0` = 前一块（第 0 列/行）独占；`1` = 后一块独占；`null` = 还在正常缩放区。
 * 用户需求 ⑦ 提到抢占前要确认，所以这里只做判断，确认与执行交给调用方。
 *
 * 边界比较必须带 `RATIO_EPSILON`：夹取后的 0.2 与 `1 - 0.8` 的浮点结果并不相等，
 * 不带容差会让「刚好拖到底」这个最容易出现的操作判定失败。
 */
export function paneClaimSide(ratio: number, threshold: number = PANE_CLAIM_THRESHOLD): 0 | 1 | null {
  const normalized = clampPaneRatio(ratio);
  const edge = clamp01(threshold);
  if (normalized >= edge - RATIO_EPSILON) return 0;
  if (normalized <= 1 - edge + RATIO_EPSILON) return 1;
  return null;
}

// ---------------------------------------------------------------------------
// 半宽覆盖 / 回拖恢复（v3 §22 二十六轮定稿）：空面板与真终端同一条规则，
// 不再弹确认框、不再把关掉的终端退回左栏——被覆盖侧只是**比例压到 0 隐藏**，
// 槽位与绑定保留（终端后台存活），回拖超过其拖前宽度的一半即恢复。
// ---------------------------------------------------------------------------

/** 覆盖态判定线（两侧各一条）：边线拖过哪条线，哪一侧就被吃掉隐藏。 */
export interface PaneCoverLines {
  /** 左（上）侧被吃掉的判定线：边线比例 ≤ 它 → 左侧隐藏、右侧独占。 */
  left: number;
  /** 右（下）侧被吃掉的判定线：边线比例 ≥ 它 → 右侧隐藏、左侧独占。 */
  right: number;
}

/**
 * 由拖拽起点的边线位置算两侧的覆盖判定线。
 *
 * 语义：边线**拖过某一侧拖前宽度的一半** → 该侧隐藏。普通拖拽里左（上）侧
 * 拖前宽度就是 `startRatio`、右（下）侧是 `1 - startRatio`，所以判定线分别是
 * `startRatio / 2` 与 `(1 + startRatio) / 2`；从对半开始拖时即 0.25 / 0.75。
 *
 * `collapsedOrigin` 只在起点已是覆盖态（某侧被压到 0）时传入：被压扁一侧按
 * **记忆的覆盖前边线位置**算恢复线（回拖超过它原宽的一半才恢复，不能一碰就弹回），
 * 此时可见的一侧占满整轴，判定线固定在中点 0.5。缺省按普通拖拽算，非法值降级。
 */
export function paneCoverLines(startRatio: number, collapsedOrigin: number | null): PaneCoverLines {
  const start = Number.isFinite(startRatio) ? Math.min(1, Math.max(0, startRatio)) : 0.5;
  const origin =
    collapsedOrigin != null && Number.isFinite(collapsedOrigin)
      ? Math.min(1, Math.max(0, collapsedOrigin))
      : null;
  const leftEnd = start <= 0.01 && origin != null ? origin : start;
  const rightStart = start >= 0.99 && origin != null ? origin : start;
  return { left: leftEnd / 2, right: (1 + rightStart) / 2 };
}

/**
 * 边线位置落在哪一侧的覆盖线之外：拖过左（上）块半宽 → `1`（后一块独占），
 * 拖过右（下）块半宽 → `0`（前一块独占），中间为 `null`（普通缩放 / 已恢复）。
 *
 * 返回值语义与 `paneClaimSide` 对齐（0 = 前一块赢、1 = 后一块赢），但阈值随
 * 拖拽起点相对计算，且覆盖态回拖用 `collapsedOrigin` 记忆的原宽判定恢复。
 * 永不抛异常，非法入参降级为「没越过」。
 */
export function paneCoverSide(
  ratio: number,
  startRatio: number,
  collapsedOrigin: number | null,
): 0 | 1 | null {
  const value = Number.isFinite(ratio) ? Math.min(1, Math.max(0, ratio)) : 0.5;
  const lines = paneCoverLines(startRatio, collapsedOrigin);
  if (value <= lines.left + RATIO_EPSILON) return 1;
  if (value >= lines.right - RATIO_EPSILON) return 0;
  return null;
}

/**
 * 抢占：让某个槽位独占整块中栏。
 *
 * 结果布局固定 `1x1`（只有一块面板时不存在分栏），其余页签全部进 `displaced`，
 * 由调用方退回页签列表——**绝不能静默丢弃**，用户还在上面跑着命令。
 */
export function claimPane(bindings: PaneBindings, slot: PaneSlot): ResizePlan {
  const size = bindings.length;
  if (!isSlot(slot, size)) return unchangedPlan(bindings);
  const winner = bindings[slot];
  if (!isOccupied(winner)) return unchangedPlan(bindings);
  const displaced = compactBindings(bindings.map((id, index) => (index === slot ? null : id))).filter(isOccupied);
  return { layout: "1x1", bindings: [winner], displaced };
}

/** 指针是否真的落在该槽位内（0..1 开区间）。落在外面说明拖到了别的面板上。 */
function isInsideZone(xRatio: number, yRatio: number): boolean {
  if (!Number.isFinite(xRatio) || !Number.isFinite(yRatio)) return false;
  return xRatio > 0 && xRatio < 1 && yRatio > 0 && yRatio < 1;
}

/**
 * 松手换位计划（需求 ⑤）：把 from 槽位的窗口放到 to 槽位上。
 *
 * 三种情形，都是「不丢页签」的：
 * - 目标空槽：窗口直接搬过去，原槽位置 null（长度不变，布局也不变）。
 * - 目标和源都占用：两者互换位置（`swapBindings`），这也是拖拽换位唯一说得通的语义
 *   —— 用户拖 A 压到 B 身上，期望的是 A 和 B 换，而不是 B 被顶掉。
 * - 指针不在目标槽内（`xRatio`/`yRatio` 越界）、槽位越界、from === to、源槽为空：
 *   一律返回原样计划，**不抛异常**（拖拽事件抛一次整棵面板树就炸了）。
 *
 * `xRatio`/`yRatio` 是相对 `to` 槽位的 0..1 坐标。用户说的「以 1/2 为界」只影响
 * 「是否算拖进了这个面板」，一旦进去就换位——两栏布局里拖左到右的哪一半都是换位，
 * 分哪半没有区别，所以这里不再按半区区分插入顺序，避免做出一个看不出来的差异。
 */
export function planDropOnPane(
  bindings: PaneBindings,
  from: PaneSlot,
  to: PaneSlot,
  xRatio: number,
  yRatio: number,
): ResizePlan {
  const size = bindings.length;
  if (!isSlot(from, size) || !isSlot(to, size) || from === to) return unchangedPlan(bindings);
  const dragged = bindings[from];
  if (!isOccupied(dragged)) return unchangedPlan(bindings);
  if (!isInsideZone(xRatio, yRatio)) return unchangedPlan(bindings);

  const layout = layoutOfBindings(bindings);
  if (!isOccupied(bindings[to])) {
    // 空槽：搬家。原槽置 null 而不是压缩，保持槽位数与布局一一对应。
    const next = bindings.slice();
    next[to] = dragged;
    next[from] = null;
    return { layout, bindings: normalizeBindings(next, layout), displaced: [] };
  }
  return { layout, bindings: normalizeBindings(swapBindings(bindings, from, to), layout), displaced: [] };
}
