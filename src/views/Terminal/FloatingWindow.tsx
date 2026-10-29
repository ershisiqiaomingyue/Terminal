import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import {
  FLOATING_MIN_HEIGHT,
  FLOATING_MIN_WIDTH,
  RESIZE_EDGES,
  clampFloatingRect,
  initialFloatingRect,
  moveFloatingRect,
  resizeFloatingRect,
} from "./floatingGeometry";
import type { FloatingBounds, FloatingRect, ResizeEdge } from "./floatingGeometry";

/** 上次窗口位置/尺寸的存储键（v3 §22 十一轮）。 */
const FLOATING_RECT_STORAGE = "android-tool.terminal-floating-rect";

export interface FloatingWindowProps {
  /** 无障碍标题，同时作为 remember 的键语义。 */
  label: string;
  /** 关闭回调（Esc 或标题栏关闭按钮由内部面板自己提供时可不传）。 */
  onClose?: () => void;
  /** 额外类名，作用在卡片本体上。 */
  className?: string;
  children: React.ReactNode;
}

/**
 * 可拖拽、可八向缩放的悬浮窗。
 *
 * 三个必须这么做的理由：
 * 1. **portal 到 body + position:fixed**：祖先若有 `backdrop-filter`，`position:
 *    absolute` 会退化成相对该祖先定位（终端的 `.terminal-panel-card` 就带
 *    backdrop-filter），卡片就会被困在父容器里。
 * 2. **标题栏整条是拖拽把手**：设计稿 `.floating-preview-header { cursor: move }`。
 *    这里不另做标题栏，直接监听卡片内的 `.ui-preview-header`，避免和
 *    FilePreviewPanel 自带的头部重复。
 * 3. **八向手柄在 frame 上而不是卡片内**：卡片自己有 `overflow: hidden` 和圆角，
 *    放内部的手柄会被裁掉一半，只剩右下角可拖。
 */
export const FloatingWindow: React.FC<FloatingWindowProps> = ({ label, onClose, className, children }) => {
  const frameRef = useRef<HTMLDivElement | null>(null);
  const [rect, setRect] = useState<FloatingRect>(() => {
    // 记住上次大小/位置（v3 §22 十一轮）：拖动/缩放后的尺寸跨开窗保留，
    // 失效（超出视口/损坏）则回默认（比旧默认更大）。
    try {
      const stored = JSON.parse(window.localStorage.getItem(FLOATING_RECT_STORAGE) ?? "null");
      if (stored && Number.isFinite(stored.width) && Number.isFinite(stored.height)) {
        return clampFloatingRect(stored, viewportBounds());
      }
    } catch {
      /* 损坏的存储直接回默认 */
    }
    return initialFloatingRect(null, viewportBounds(), { width: 640, height: 480 });
  });
  // 尺寸/位置变化即持久化（含八向缩放的结果），下次打开按上次使用。
  useEffect(() => {
    try {
      window.localStorage.setItem(FLOATING_RECT_STORAGE, JSON.stringify(rect));
    } catch {
      /* 存储满时保持会话内记忆即可 */
    }
  }, [rect]);
  const [dragging, setDragging] = useState(false);

  // 手势期间的所有中间量都放 ref：state 每次 pointermove 都会重渲染，
  // 累计位移会读到上一帧还没提交的值，导致拖动"跟手但越拖越慢"。
  const gestureRef = useRef<{
    mode: "move" | ResizeEdge;
    startX: number;
    startY: number;
    startRect: FloatingRect;
  } | null>(null);

  const beginGesture = useCallback((mode: "move" | ResizeEdge, event: React.PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    gestureRef.current = {
      mode,
      startX: event.clientX,
      startY: event.clientY,
      startRect: rect,
    };
    setDragging(true);
  }, [rect]);

  useEffect(() => {
    if (!dragging) return;

    const onMove = (event: PointerEvent) => {
      const gesture = gestureRef.current;
      if (!gesture) return;
      const dx = event.clientX - gesture.startX;
      const dy = event.clientY - gesture.startY;
      const bounds = viewportBounds();
      setRect(
        gesture.mode === "move"
          ? moveFloatingRect(gesture.startRect, dx, dy, bounds)
          : resizeFloatingRect(gesture.startRect, gesture.mode, dx, dy, bounds),
      );
    };

    const onUp = () => {
      gestureRef.current = null;
      setDragging(false);
    };

    // 手势期间锁掉全局 user-select，否则拖到终端画布上会顺手选中一片文字。
    const previousSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    document.body.style.cursor = gestureRef.current && gestureRef.current.mode !== "move" ? "nwse-resize" : "grabbing";

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      document.body.style.userSelect = previousSelect;
      document.body.style.cursor = "";
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
  }, [dragging]);

  // 窗口尺寸变化时把卡片夹回可见区，否则缩小窗口后卡片会飘到视口外。
  useEffect(() => {
    const onResize = () => setRect((current) => clampFloatingRect(current, viewportBounds()));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Esc 关闭：面板内部的 Esc 会被 FilePreviewPanel 自己消费（先关搜索再关闭）。
  useEffect(() => {
    if (!onClose) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const frame = frameRef.current;
      if (!frame) return;
      // 只响应当前焦点在卡片内的情况，避免抢终端里的 Esc。
      if (!frame.contains(document.activeElement)) return;
      onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const style = useMemo<React.CSSProperties>(
    () => ({ left: rect.left, top: rect.top, width: rect.width, height: rect.height }),
    [rect.left, rect.top, rect.width, rect.height],
  );

  return createPortal(
    <div
      ref={frameRef}
      className="floating-window"
      data-dragging={dragging ? "" : undefined}
      style={style}
      role="dialog"
      aria-label={label}
      onPointerDown={(event) => {
        // 标题栏整条可拖，但标题栏里的按钮（搜索/复制/重试/关闭）要留给按钮自己。
        const target = event.target as HTMLElement;
        // 把手：旧外壳的标题栏 + 文件页形态的标签条（v3 §22 十一轮）。
        if (!target.closest(".ui-preview-header, .code-viewer-tab-bar")) return;
        if (target.closest("button, input, a")) return;
        beginGesture("move", event);
      }}
    >
      <div className={`floating-window-card ${className ?? ""}`.trim()}>{children}</div>
      {RESIZE_EDGES.map((edge) => (
        <span
          key={edge}
          className="floating-window-grip"
          data-edge={edge}
          aria-hidden="true"
          onPointerDown={(event) => beginGesture(edge, event)}
        />
      ))}
    </div>,
    document.body,
  );
};

/** 视口尺寸，保证不低于最小可拖拽尺寸，避免 0 尺寸时算出荒谬坐标。 */
function viewportBounds(): FloatingBounds {
  return {
    width: Math.max(window.innerWidth, FLOATING_MIN_WIDTH),
    height: Math.max(window.innerHeight, FLOATING_MIN_HEIGHT),
  };
}

export default FloatingWindow;
