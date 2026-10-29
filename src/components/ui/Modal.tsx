import React, { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import "./modal.css";

export interface ModalProps {
  open: boolean;
  title: string;
  /** 标题下面那行小字：说明这个弹窗要干什么，或者当前有多少条。 */
  description?: string;
  /** 标题左边的图标（放在一个淡色方块里）。 */
  icon?: React.ReactNode;
  iconTone?: "neutral" | "danger";
  size?: "sm" | "md";
  /** 忙碌时禁止关闭：遮罩点击、Esc、右上角 X 都会失效，避免中途丢状态。 */
  busy?: boolean;
  /** 底部按钮区；不给就只渲染内容（比如纯展示的弹窗）。 */
  footer?: React.ReactNode;
  className?: string;
  onClose: () => void;
  children: React.ReactNode;
}

/**
 * 通用弹窗壳。
 *
 * 原先只有 ConfirmDialog 一个模态（专做二次确认），归档面板借用的是设置页的
 * 编辑弹窗壳——从侧栏点进去会先跳到设置，观感也不对。这里把「遮罩 + 卡片 +
 * 标题栏 + 底部按钮」抽出来，给归档、重命名这类自己的弹窗共用。
 */
export const Modal: React.FC<ModalProps> = ({
  open,
  title,
  description,
  icon,
  iconTone = "neutral",
  size = "md",
  busy = false,
  footer,
  className,
  onClose,
  children,
}) => {
  const modalRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement as HTMLElement | null;
    const root = document.getElementById("root");
    const previousInert = root?.inert;
    if (root) root.inert = true;
    // 弹窗里第一个可聚焦的东西通常就是主角（输入框 / 第一个按钮）。
    const first = modalRef.current?.querySelector<HTMLElement>('input:not(:disabled), textarea:not(:disabled), button:not(:disabled), [tabindex]:not([tabindex="-1"])');
    (first || modalRef.current)?.focus();
    return () => {
      if (root) root.inert = Boolean(previousInert);
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (!busy) onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = Array.from(modalRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])',
      ) || []);
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (!first) { event.preventDefault(); modalRef.current?.focus(); return; }
      if (event.shiftKey && (document.activeElement === first || !modalRef.current?.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !modalRef.current?.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [open, busy, onClose]);

  if (!open || typeof document === "undefined") return null;

  return createPortal(
    <div className={`ui-modal-overlay${className ? ` ${className}` : ""}`} onClick={() => { if (!busy) onClose(); }}>
      <div
        ref={modalRef}
        className={`ui-modal ui-modal-${size}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        aria-busy={busy}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="ui-modal-header">
          {icon && <span className={`ui-modal-icon${iconTone === "danger" ? " is-danger" : ""}`} aria-hidden="true">{icon}</span>}
          <div className="ui-modal-headings">
            <h3 className="ui-modal-title" id={titleId}>{title}</h3>
            {description && <p className="ui-modal-description" id={descriptionId}>{description}</p>}
          </div>
          <button type="button" className="ui-modal-close" title="关闭" aria-label="关闭" disabled={busy} onClick={onClose}>
            <X size={15} />
          </button>
        </div>
        <div className="ui-modal-content">{children}</div>
        {footer && <div className="ui-modal-footer">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
};

export default Modal;
