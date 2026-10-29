import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check } from "lucide-react";
import "./context-menu.css";

export interface ContextMenuItem {
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  checked?: boolean;
  /** 破坏性动作（删除/移除）：文字与图标走危险色，和普通项区分开。 */
  danger?: boolean;
  onSelect: () => void;
}

export function ContextMenu({ x, y, label, items, trigger, onClose }: {
  x: number;
  y: number;
  label: string;
  items: readonly ContextMenuItem[];
  trigger: HTMLElement;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    const rect = menu.getBoundingClientRect();
    setPosition({ left: Math.max(8, Math.min(x, window.innerWidth - rect.width - 8)), top: Math.max(8, Math.min(y, window.innerHeight - rect.height - 8)) });
    menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
  }, [x, y]);

  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    // 打开时的锚点位置。滚动事件会从任何滚动容器冒到 window（捕获阶段也能收到），
    // 包括侧栏标题的悬停滚动——那种位移不改变锚点，菜单必须留着。只有锚点真的
    // 被滚走（或随列表消失）时才关。
    const anchor = trigger.getBoundingClientRect();
    const anchorMoved = () => {
      if (!trigger.isConnected) return true;
      const rect = trigger.getBoundingClientRect();
      return Math.abs(rect.top - anchor.top) > 1 || Math.abs(rect.left - anchor.left) > 1;
    };
    const close = () => closeRef.current();
    const outside = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) close(); };
    const scrolled = () => { if (anchorMoved()) close(); };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("blur", close);
    };
  }, [trigger]);

  return createPortal(<div ref={ref} className="context-menu" role="menu" aria-label={label} style={position} onContextMenu={(event) => event.preventDefault()} onKeyDown={(event) => {
    if (event.key === "Escape" || event.key === "Tab") {
      if (event.key === "Escape") { event.preventDefault(); trigger.focus({ preventScroll: true }); }
      event.stopPropagation(); onClose(); return;
    }
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : event.key === "ArrowDown" ? (current + 1) % buttons.length : event.key === "ArrowUp" ? (current - 1 + buttons.length) % buttons.length : undefined;
    if (next !== undefined) { event.preventDefault(); buttons[next]?.focus(); }
  }}>
    {items.map((item) => <button key={item.label} type="button" role="menuitem" disabled={item.disabled} className={item.danger ? "is-danger" : undefined} onClick={() => {
      onClose(); trigger.focus({ preventScroll: true }); item.onSelect();
    }}>{item.icon}<span>{item.label}</span>{item.checked && <Check className="context-menu-check" aria-hidden="true" size={14} />}</button>)}
  </div>, document.body);
}
