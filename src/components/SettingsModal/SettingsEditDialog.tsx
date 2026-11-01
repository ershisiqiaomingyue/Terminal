import React, { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { Button } from "rain-ui";

/**
 * 设置页通用编辑弹窗 chrome —— 零自定义 CSS，全部 Tailwind 工具类。
 * `provider-editor-overlay` / `provider-editor-dialog` / `provider-editor-content`
 * 是 inert 栈查询与外部样式的锚点类（code-viewer-toolbar.css 等），不可随意删改。
 */
export function SettingsEditDialog({ title, busy = false, onClose, children, className = "", overlayClassName = "z-[12000]" }: {
  title: string;
  busy?: boolean;
  onClose: () => void;
  children: React.ReactNode;
  className?: string;
  overlayClassName?: string;
}) {
  const titleId = useId();
  const overlayRef = useRef<HTMLDivElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    // A discovery dialog can sit above a provider editor portaled outside root.
    const backgrounds = [document.getElementById("root"), ...document.querySelectorAll<HTMLElement>(".provider-editor-overlay")]
      .filter((element): element is HTMLElement => Boolean(element) && element !== overlayRef.current);
    const states = backgrounds.map((element) => ({ element, inert: element.inert }));
    backgrounds.forEach((element) => { element.inert = true; });
    dialogRef.current?.querySelector<HTMLElement>('input:not(:disabled), textarea:not(:disabled), button:not(:disabled)')?.focus();
    return () => {
      states.forEach(({ element, inert }) => { element.inert = inert; });
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    };
  }, []);

  return createPortal(<div ref={overlayRef} className={`provider-editor-overlay fixed inset-0 ${overlayClassName} flex items-center justify-center bg-black/40 p-5`} onClick={(event) => {
    event.stopPropagation();
    if (!busy && event.target === event.currentTarget) onClose();
  }} onKeyDown={(event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (!busy) onClose();
    }
    if (event.key !== "Tab") return;
    const selector = 'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex="0"]';
    const popup = document.querySelector(".select-dropdown.is-portaled");
    const controls = [...Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(selector) || []),
      ...Array.from(popup?.querySelectorAll<HTMLElement>(selector) || [])]
      .filter((element) => element.getClientRects().length > 0);
    const first = controls[0];
    const last = controls.at(-1);
    if (!first) { event.preventDefault(); dialogRef.current?.focus(); }
    else if (event.shiftKey && (document.activeElement === first || document.activeElement === dialogRef.current)) {
      event.preventDefault(); last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  }}>
    <div ref={dialogRef} className={`provider-editor-dialog flex w-full min-w-0 max-w-[720px] flex-col overflow-hidden rounded-2xl border border-border/80 bg-card text-card-foreground shadow-2xl max-h-[min(720px,calc(100dvh-40px))] ${className}`} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
      <header className="flex flex-none items-center justify-between gap-3 border-b border-border/70 px-5 py-3.5">
        <h3 className="text-sm font-semibold text-foreground">{title}</h3>
        <Button type="button" variant="ghost" size="icon-xs" icon={<X size={15} />} aria-label="关闭弹窗" title="关闭" disabled={busy} onClick={onClose} />
      </header>
      <div className="provider-editor-content min-h-0 flex-1 overflow-y-auto p-5 [overscroll-behavior:contain] [scrollbar-gutter:stable]">{children}</div>
    </div>
  </div>, document.body);
}
