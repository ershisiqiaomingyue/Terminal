export interface PaneLimits {
  min: number;
  max: number;
  initial: number;
  contentMin: number;
}

export const PROJECT_PANE: PaneLimits = { min: 180, max: 420, initial: 240, contentMin: 480 };
export const OUTLINE_PANE: PaneLimits = { min: 140, max: 320, initial: 180, contentMin: 320 };
export const WORKSPACE_PANE: PaneLimits = { min: 280, max: 560, initial: 316, contentMin: 420 };
export const PANE_HANDLE_WIDTH = 6;

/**
 * 终端页右栏（主机状态 / 版本控制 / 文件浏览）。原来硬编码 262px，文件树
 * 和代码预览在里面挤得看不清路径，所以给它和代码查看器一样的拖拽改宽能力。
 *
 * `contentMin` 量的是 `.terminal-workspace-body` 这一层：左栏 246px + 终端区
 * 至少 360px 才够看一行命令，所以留 606。
 */
export const TERMINAL_DRAWER_PANE: PaneLimits = { min: 240, max: 720, initial: 300, contentMin: 606 };

/**
 * 对话页终端面板的高度范围。`contentMin` 预留给输入框和会话区，
 * 保证拖动到上限时消息不会被压缩到看不见。
 */
export const TERMINAL_PANE: PaneLimits = { min: 120, max: 900, initial: 250, contentMin: 300 };

export function paneBounds(containerWidth: number, limits: PaneLimits) {
  const available = containerWidth > 0 ? containerWidth - limits.contentMin - PANE_HANDLE_WIDTH : limits.max;
  return { min: limits.min, max: Math.max(limits.min, Math.min(limits.max, available)) };
}

export function clampPaneWidth(value: number, bounds: { min: number; max: number }) {
  return Math.max(bounds.min, Math.min(bounds.max, value));
}

/**
 * 纵向空间的可用范围：在总高度里让出 `contentMin` 和手柄厚度。
 * 容器高度未知（0）时退回上限，避免首帧把面板压到最小。
 */
export function paneHeightBounds(containerHeight: number, limits: PaneLimits) {
  return paneBounds(containerHeight, limits);
}
