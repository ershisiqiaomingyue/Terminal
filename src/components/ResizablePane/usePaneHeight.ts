import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { clampPaneWidth, paneHeightBounds, type PaneLimits } from "./paneWidth";

/** 只读的 ref 形状，兼容 `useRef<T>(null)` 与 `useRef<T | null>(null)` 两种写法。 */
type ElementRef = RefObject<HTMLElement | null> | { current: HTMLElement | null };

/**
 * 纵向版 `usePaneWidth`：把用户拖出来的高度记在 localStorage。
 *
 * `budget` 是可用空间的来源容器（测它的 `clientHeight`）。当被拖动的元素本身也是
 * flex 子项、高度就是拖动结果时，必须传外层容器，否则会自己量自己形成循环。
 */
export function usePaneHeight(budget: ElementRef, enabled: boolean, key: string, limits: PaneLimits) {
  const [preferredHeight, setPreferredHeight] = useState(() => {
    try {
      const stored = Number(localStorage.getItem(key));
      return Number.isFinite(stored) && stored >= limits.min && stored <= limits.max ? stored : limits.initial;
    } catch { return limits.initial; }
  });
  const [availableHeight, setAvailableHeight] = useState(0);
  const preferredRef = useRef(preferredHeight);
  useLayoutEffect(() => {
    if (!enabled) return;
    let observer: ResizeObserver | null = null;
    let frame = 0;
    let attempts = 0;
    // 祖先节点的 ref 可能比子组件的 layout effect 更晚挂上，首帧读不到就等下一帧重试，
    // 否则 availableHeight 会一直停在 0、上限退化成 limits.max，能拖到把会话区压没。
    const attach = () => {
      const element = budget.current;
      if (!element) {
        if (attempts++ < 120) frame = requestAnimationFrame(attach);
        return;
      }
      const read = () => setAvailableHeight(element.clientHeight);
      read();
      observer = new ResizeObserver(read);
      observer.observe(element);
    };
    attach();
    return () => {
      if (frame) cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [budget, enabled]);
  const bounds = paneHeightBounds(availableHeight, limits);
  const resize = useCallback((height: number) => { preferredRef.current = height; setPreferredHeight(height); }, []);
  const commit = useCallback(() => {
    try { localStorage.setItem(key, String(Math.round(preferredRef.current))); } catch { /* Height preference is optional. */ }
  }, [key]);
  return { height: clampPaneWidth(preferredHeight, bounds), bounds, resize, commit };
}
