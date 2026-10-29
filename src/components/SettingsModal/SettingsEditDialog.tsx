import React, { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

export function SettingsEditDialog({ title, busy = false, onClose, children, className = "" }: {
  title: string;
  busy?: boolean;
  onClose: () => void;
  children: React.ReactNode;
  className?: string;
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

  return createPortal(<div ref={overlayRef} className="provider-editor-overlay" onClick={(event) => {
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
    <div ref={dialogRef} className={`provider-editor-dialog ${className}`} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
      <header className="provider-editor-header"><h3 id={titleId}>{title}</h3>
        <button type="button" className="icon-action-btn" aria-label="关闭弹窗" title="关闭" disabled={busy} onClick={onClose}><X size={15} /></button>
      </header>
      <div className="provider-editor-content">{children}</div>
    </div>
  </div>, document.body);
}
