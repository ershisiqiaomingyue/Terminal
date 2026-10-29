import { useCallback, useEffect, useRef } from "react";
import type React from "react";
import type { Window as TauriWindow } from "@tauri-apps/api/window";

/**
 * 顶栏里不允许触发窗口拖动 / 双击最大化的交互元素。
 *
 * 这些元素要么自身有点击语义（按钮、输入框），要么有独立的拖拽手势
 * （标签页靠 `useTabDetachDrag` 拖出成独立窗口），必须把鼠标事件留给它们。
 */
export const WINDOW_DRAG_BLOCKED_SELECTOR = [
  "button",
  "a",
  "input",
  "textarea",
  "select",
  "[role='button']",
  ".topbar-tab",
  ".topbar-search",
  ".topbar-window-controls",
  ".topbar-tabs-overflow",
].join(", ");

export function isWindowDragBlocked(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(WINDOW_DRAG_BLOCKED_SELECTOR) !== null;
}

/** 按下后指针移动超过该距离，才真正发起系统拖动（逻辑 px）。 */
const DRAG_START_THRESHOLD_PX = 3;
/** 两次「纯点击」间隔小于该值才算双击。 */
const DOUBLE_CLICK_MS = 500;
/** 两次「纯点击」位置偏移小于该值才算双击。 */
const DOUBLE_CLICK_SLOP_PX = 6;

export interface WindowDragHandlers {
  onMouseDown: (event: React.MouseEvent) => void;
}

/**
 * 无边框窗口顶栏的拖动处理器。
 *
 * ## 为什么不能「按下就 startDragging」
 *
 * `startDragging()` 不是同步拖动，它是一条
 * 「JS → IPC → Rust 主线程 → `PostMessage(WM_NCLBUTTONDOWN, HTCAPTION)` → 系统 modal move loop」
 * 的异步链路（见 tao `platform_impl/windows/window.rs` 的 `handle_os_dragging`）。
 * 真正接管鼠标发生在系统消息队列里，而不是本次 mousedown 的调用栈内，于是有两个后果：
 *
 * 1. 用户在系统进入 move loop 之前松手（轻点、手抖、极短促的拖动），这次拖动就"没发生"，
 *    窗口纹丝不动 —— 用户自然会再按一次重试。
 * 2. 每次按下都会投递一条 `WM_NCLBUTTONDOWN / HTCAPTION`，等于告诉系统"用户点了标题栏"。
 *    重试那次若距离上一次不足系统双击时间（`GetDoubleClickTime()`，默认 500ms），
 *    系统就按**标题栏双击**处理，把窗口最大化/还原。实测：按下-微动 2px-松开，
 *    隔 80/150/250/350ms 再按住拖 → 4 次全部变成最大化；隔 450ms 以上才正常拖动。
 *    这正好卡在 500ms 双击窗口的边界上，和"要多拖两次才成功"的观感完全吻合。
 *
 * ## 现在的做法
 *
 * 拖动改为「移动确认后再发起」：mousedown 只记录起点，等指针真正移动超过
 * `DRAG_START_THRESHOLD_PX` 才调用 `startDragging()`。这样
 *
 * - 单纯的点击/连点**永远不会**投递 `WM_NCLBUTTONDOWN`，系统不会再把重试误判成
 *   标题栏双击，窗口也不会突然最大化；
 * - 发起拖动时手指/鼠标仍然按着并在移动中，系统 move loop 能立即接管，不会出现
 *   "按了没反应"；
 * - 双击最大化改由这里自己的计时判断（两次纯点击、间隔 < 500ms、位置接近），
 *   行为与系统标题栏一致，但不受那条异步链路影响。
 */
export function useWindowDrag(
  getHandle: () => TauriWindow | null,
  onError?: (error: unknown) => void
): WindowDragHandlers {
  const lastClick = useRef<{ time: number; x: number; y: number } | null>(null);
  const detach = useRef<(() => void) | null>(null);

  const cleanup = useCallback(() => {
    detach.current?.();
    detach.current = null;
  }, []);

  // 组件卸载时清掉挂在 window 上的监听
  useEffect(() => cleanup, [cleanup]);

  const onMouseDown = useCallback(
    (event: React.MouseEvent) => {
      if (event.button !== 0) return;
      if (isWindowDragBlocked(event.target)) return;
      const handle = getHandle();
      if (!handle) return;
      // 避免顶栏文字被选中、焦点被抢走；不会阻止 click / dblclick 派发。
      event.preventDefault();

      cleanup();

      const startX = event.clientX;
      const startY = event.clientY;
      let dragging = false;

      const finish = () => {
        cleanup();
        if (dragging) return;
        // 到这里说明按下后指针没有移动过：是一次纯点击，判断是否构成双击。
        const now = Date.now();
        const previous = lastClick.current;
        if (
          previous &&
          now - previous.time <= DOUBLE_CLICK_MS &&
          Math.hypot(startX - previous.x, startY - previous.y) <= DOUBLE_CLICK_SLOP_PX
        ) {
          lastClick.current = null;
          void handle.toggleMaximize().catch((error) => onError?.(error));
          return;
        }
        lastClick.current = { time: now, x: startX, y: startY };
      };

      const onMove = (moveEvent: MouseEvent) => {
        if (dragging) return;
        if (Math.hypot(moveEvent.clientX - startX, moveEvent.clientY - startY) < DRAG_START_THRESHOLD_PX) return;
        dragging = true;
        // 拖动一旦发起就不再参与双击判定，也不需要在 window 上继续跟踪。
        lastClick.current = null;
        cleanup();
        void handle.startDragging().catch((error) => onError?.(error));
      };

      window.addEventListener("mousemove", onMove, true);
      window.addEventListener("mouseup", finish, true);
      detach.current = () => {
        window.removeEventListener("mousemove", onMove, true);
        window.removeEventListener("mouseup", finish, true);
      };
    },
    [cleanup, getHandle, onError]
  );

  return { onMouseDown };
}
