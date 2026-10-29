import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { clampPaneWidth, paneBounds, type PaneLimits } from "./paneWidth";

export function usePaneWidth(container: RefObject<HTMLElement>, enabled: boolean, key: string, limits: PaneLimits) {
  const [preferredWidth, setPreferredWidth] = useState(() => {
    try {
      const stored = Number(localStorage.getItem(key));
      return Number.isFinite(stored) && stored >= limits.min && stored <= limits.max ? stored : limits.initial;
    } catch { return limits.initial; }
  });
  const [containerWidth, setContainerWidth] = useState(0);
  const preferredRef = useRef(preferredWidth);
  useLayoutEffect(() => {
    if (!enabled || !container.current) return;
    const element = container.current;
    const measure = () => setContainerWidth(element.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [container, enabled]);
  const bounds = paneBounds(containerWidth, limits);
  const resize = useCallback((width: number) => { preferredRef.current = width; setPreferredWidth(width); }, []);
  const commit = useCallback(() => {
    try { localStorage.setItem(key, String(preferredRef.current)); } catch { /* Width preference is optional. */ }
  }, [key]);
  return { width: clampPaneWidth(preferredWidth, bounds), bounds, resize, commit };
}
