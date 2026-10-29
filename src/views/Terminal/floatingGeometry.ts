/**
 * 悬浮窗几何层（文件预览卡片的拖拽与缩放）。
 *
 * 单独成文件的原因：拖拽/缩放的边界夹取是最容易出「窗口被拖出屏幕再也抓不回来」
 * 这类事故的地方，而它又是纯算术。放在 tsx 里就只能靠手点验证，这里可以
 * 用 `node --experimental-strip-types --test` 直接把八向缩放和四边夹取跑一遍。
 *
 * 坐标一律用**视口坐标**（与 `position: fixed` 的 left/top/width/height 同空间）：
 * 卡片是 createPortal 到 body 上的，祖先带 backdrop-filter 时 absolute 会退化成
 * 相对该祖先定位，只有 fixed 不受影响。
 */

/** 卡片矩形，单位 px，视口坐标。 */
export interface FloatingRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** 可用区域（视口）尺寸。 */
export interface FloatingBounds {
  width: number;
  height: number;
}

/** 与设计稿 `.floating-file-preview-card` 的 min-width / min-height 一致。 */
export const FLOATING_MIN_WIDTH = 320;
export const FLOATING_MIN_HEIGHT = 220;

/**
 * 拖拽/缩放后必须留在视口内的最小可见量。
 * 等于标题栏高度量级：只要露出一条标题栏，用户总能抓回来。
 */
export const FLOATING_KEEP_VISIBLE = 48;

/** 八个缩放方向。 */
export type ResizeEdge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export const RESIZE_EDGES: readonly ResizeEdge[] = [
  "n",
  "s",
  "e",
  "w",
  "ne",
  "nw",
  "se",
  "sw",
];

const clamp = (value: number, min: number, max: number): number =>
  Math.min(Math.max(value, min), Math.max(min, max));

/**
 * 把矩形夹回可视范围：保证左边不超出右边界、顶边不高于视口顶。
 *
 * 允许左侧/上方出现负值（卡片可以探出视口一部分，用户常这么摆），但至少
 * 保留 `FLOATING_KEEP_VISIBLE` 的可见量，避免"窗口还在但完全摸不到"。
 */
export function clampFloatingRect(rect: FloatingRect, bounds: FloatingBounds): FloatingRect {
  const width = Math.max(FLOATING_MIN_WIDTH, rect.width);
  const height = Math.max(FLOATING_MIN_HEIGHT, rect.height);
  // 视口比最小尺寸还小时的退路：贴住左上角，宽度不强行撑破视口。
  const usableWidth = Math.max(0, bounds.width);
  const usableHeight = Math.max(0, bounds.height);
  const finalWidth = Math.min(width, Math.max(usableWidth, FLOATING_MIN_WIDTH));
  const finalHeight = Math.min(height, Math.max(usableHeight, FLOATING_MIN_HEIGHT));
  return {
    left: clamp(rect.left, -finalWidth + FLOATING_KEEP_VISIBLE, usableWidth - FLOATING_KEEP_VISIBLE),
    // 顶边不允许到 0 以上：标题栏是唯一的抓手，必须始终可点。
    top: clamp(rect.top, 0, Math.max(0, usableHeight - FLOATING_KEEP_VISIBLE)),
    width: finalWidth,
    height: finalHeight,
  };
}

/**
 * 拖动标题栏：`dx/dy` 是本次手势相对按下点的累计位移。
 * 用「起始矩形 + 累计位移」而不是「上一帧 + 增量」，避免夹取后累积误差。
 */
export function moveFloatingRect(
  start: FloatingRect,
  dx: number,
  dy: number,
  bounds: FloatingBounds,
): FloatingRect {
  return clampFloatingRect({ ...start, left: start.left + dx, top: start.top + dy }, bounds);
}

/**
 * 八向缩放。对边固定、动的那一侧移动；任一侧被夹住后另一侧不再跟着漂。
 *
 * `dx/dy` 同样是相对按下点的累计位移。
 */
export function resizeFloatingRect(
  start: FloatingRect,
  edge: ResizeEdge,
  dx: number,
  dy: number,
  bounds: FloatingBounds,
): FloatingRect {
  const right = start.left + start.width;
  const bottom = start.top + start.height;
  let { left, top, width, height } = start;

  if (edge.includes("e")) {
    // 右边跟随指针，但不许越过视口右边缘。
    const maxWidth = Math.max(FLOATING_MIN_WIDTH, bounds.width - start.left);
    width = clamp(start.width + dx, FLOATING_MIN_WIDTH, maxWidth);
  }
  if (edge.includes("s")) {
    const maxHeight = Math.max(FLOATING_MIN_HEIGHT, bounds.height - start.top);
    height = clamp(start.height + dy, FLOATING_MIN_HEIGHT, maxHeight);
  }
  if (edge.includes("w")) {
    // 左边跟随指针、右边固定：宽度由 newLeft 反推，不能直接加 dx。
    left = clamp(start.left + dx, 0, right - FLOATING_MIN_WIDTH);
    width = right - left;
  }
  if (edge.includes("n")) {
    top = clamp(start.top + dy, 0, bottom - FLOATING_MIN_HEIGHT);
    height = bottom - top;
  }

  return { left, top, width, height };
}

/** 初次打开时的落位：贴着锚点容器（终端工作区）的右下角，并向内缩 18px。 */
export function initialFloatingRect(
  anchor: { right: number; bottom: number; width: number; height: number } | null,
  bounds: FloatingBounds,
  preferred: { width: number; height: number } = { width: 520, height: 380 },
): FloatingRect {
  const rightEdge = anchor ? anchor.right : bounds.width;
  const bottomEdge = anchor ? anchor.bottom : bounds.height;
  const width = clamp(preferred.width, FLOATING_MIN_WIDTH, Math.max(FLOATING_MIN_WIDTH, rightEdge - 20));
  const height = clamp(preferred.height, FLOATING_MIN_HEIGHT, Math.max(FLOATING_MIN_HEIGHT, bottomEdge - 20));
  return clampFloatingRect(
    {
      left: Math.max(0, rightEdge - width - 18),
      top: Math.max(0, bottomEdge - height - 18),
      width,
      height,
    },
    bounds,
  );
}
