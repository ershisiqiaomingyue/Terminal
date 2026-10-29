import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export function ToolbarPopover({ anchor, label, className, children, onClose }: { anchor: HTMLElement; label: string; className?: string; children: ReactNode; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 8, top: 8 });
  useLayoutEffect(() => {
    const panel = ref.current;
    if (!panel) return;
    const positionPanel = () => {
      const rect = anchor.getBoundingClientRect(), popup = panel.getBoundingClientRect();
      // 锚点落在右半屏（工具条右侧按钮、悬浮窗头部按钮）→ 弹层右缘对齐锚点
      // 右缘、整体朝左展开；左半屏锚点保持朝右（v3 §22 十六轮：用户要求
      // 稳定朝左，避免贴边时弹层被裁）。
      const alignRight = rect.left > window.innerWidth / 2;
      const desiredLeft = alignRight ? rect.right - popup.width : rect.left;
      const next = { left: Math.max(8, Math.min(desiredLeft, window.innerWidth - popup.width - 8)), top: Math.max(8, Math.min(rect.bottom + 7, window.innerHeight - popup.height - 8)) };
      setPosition((current) => current.left === next.left && current.top === next.top ? current : next);
    };
    positionPanel();
    const observer = new ResizeObserver(positionPanel); observer.observe(panel);
    // 弹层默认焦点放在容器上：直接聚焦首个小按钮会在标题右侧画出一圈很重的焦点环。
    const input = panel.querySelector<HTMLInputElement>("input");
    if (input) input.focus({ preventScroll: true });
    else panel.focus({ preventScroll: true });
    return () => observer.disconnect();
  }, [anchor]);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      const element = event.target as HTMLElement;
      if (!ref.current?.contains(element) && !anchor.contains(element) && !element.closest(".context-menu, .confirm-dialog-overlay, .settings-edit-overlay")) onClose();
    };
    const key = (event: KeyboardEvent) => { if (event.key === "Escape" && !document.querySelector('[aria-modal="true"], .context-menu')) {
      event.preventDefault(); onClose();
      (anchor.matches("button") ? anchor : anchor.querySelector<HTMLElement>("button"))?.focus({ preventScroll: true });
    } };
    window.addEventListener("pointerdown", outside, true); window.addEventListener("keydown", key); window.addEventListener("resize", onClose);
    return () => { window.removeEventListener("pointerdown", outside, true); window.removeEventListener("keydown", key); window.removeEventListener("resize", onClose); };
  }, [anchor, onClose]);
  return createPortal(<div ref={ref} className={`code-toolbar-popover${className ? ` ${className}` : ""}`} role="dialog" aria-label={label} style={position}>{children}</div>, document.body);
}
