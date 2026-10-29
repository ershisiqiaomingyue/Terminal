/**
 * 滚动语义的纯函数（开发方案 6.3 表 #7 / R-47）。
 *
 * 单独成文件的原因：`terminalAdapter.ts` 在运行时导入 xterm.js 与自研内核的
 * `?url` wasm 资源，Node 的单测进程加载不了；这里只放与内核无关的判定，
 * 好让 `tests/terminalSemantics.test.mts` 能直接跑它。
 */

/**
 * "跟随最新输出"判定：只有视口停在缓冲底部时才自动滚动。
 *
 * POC V2 实测：自研内核的 `viewportY` 语义与 xterm 一致（0 = 顶部，底部 = `baseY`），
 * 内核 `lib/terminal.ts:174` 的 "0 = at bottom"
 * 注释是错的；`onScroll` 回调参数在两个内核里都是视口绝对行号。因此两个内核共用
 * 这一个判定。
 */
export function isFollowingLatestLine(buffer: { viewportY: number; baseY: number }): boolean {
  return buffer.viewportY >= buffer.baseY;
}

/** 滚动条轨道上下各留多少空隙（CSS px）。 */
export const SCROLLBAR_TRACK_PADDING = 4;
/** 滑块最短长度：历史再长也要抓得住。 */
export const SCROLLBAR_MIN_THUMB = 20;

export interface ScrollbarGeometry {
  /** 滑块相对轨道顶部的偏移（CSS px）。 */
  thumbTop: number;
  /** 滑块长度（CSS px）。 */
  thumbHeight: number;
  /** 有没有可滚动的内容；false 时整条滚动条不显示。 */
  visible: boolean;
}

/**
 * 自绘滚动条的几何计算。
 *
 * 为什么不用内核内建的滚动条：自研内核（ghostty-web）把滚动条**画进 canvas 自己
 * 的宽高里**（`renderer.ts:1069` `scrollbarX = canvasWidth - 8 - 4`），而 canvas 只有
 * `cols * cellWidth` 宽、比宿主窄一截，于是滚动条缩在画布右缘，既不是贴着面板右框，
 * 又会在正文字符上方压出一条轨道底色。这里改成 DOM 绘制，位置由 CSS 决定，
 * 几何只接内核给的 `rows / baseY / viewportY`（语义与 xterm 一致）。
 */
export function scrollbarGeometry(input: {
  rows: number;
  baseY: number;
  viewportY: number;
  trackHeight: number;
}): ScrollbarGeometry {
  const { rows, baseY, viewportY, trackHeight } = input;
  const inner = trackHeight - SCROLLBAR_TRACK_PADDING * 2;
  if (baseY <= 0 || inner <= 0 || rows <= 0) return { thumbTop: 0, thumbHeight: 0, visible: false };
  const thumbHeight = Math.min(inner, Math.max(SCROLLBAR_MIN_THUMB, (rows / (baseY + rows)) * inner));
  // position：0 = 视口在回滚缓冲顶部，1 = 停在最新输出（`viewportY === baseY`）。
  // 所以滑块越靠下越接近底部，直接用 position 而非 1 - position。
  const position = Math.min(1, Math.max(0, viewportY / baseY));
  const thumbTop = SCROLLBAR_TRACK_PADDING + (inner - thumbHeight) * position;
  return { thumbTop, thumbHeight, visible: true };
}

/**
 * 拖动滑块换算成"要滚动多少行"（正数 = 往新输出方向滚，与 `scrollLines` 同向）。
 *
 * 视口语义：`viewportY` 是可见区首行的绝对行号（0 = 回滚缓冲顶部，`baseY` = 底部）。
 * 滑块往下拖 = 往历史后面走 = `viewportY` 变大、`scrolledBack` 变小 = `scrollLines`
 * 传正数，所以这里 `deltaY` 直接同号返回。
 */
export function scrollLinesFromPointer(input: {
  deltaY: number;
  trackHeight: number;
  baseY: number;
  thumbHeight: number;
}): number {
  const free = input.trackHeight - SCROLLBAR_TRACK_PADDING * 2 - input.thumbHeight;
  if (free <= 0 || input.baseY <= 0) return 0;
  return input.deltaY * (input.baseY / free);
}
