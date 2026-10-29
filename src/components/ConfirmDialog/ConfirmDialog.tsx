import React, { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, X } from "lucide-react";
import { Button } from "@/components/ui";
import { Checkbox } from "@/components/ui/Checkbox";
import "./confirm-dialog.css";

export interface ConfirmDialogProps {
  open: boolean;
  className?: string;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  variant?: "danger" | "primary";
  acknowledgeLabel?: string;
  busy?: boolean;
  confirmDisabled?: boolean;
  acknowledgementKey?: string;
  error?: string;
  children?: React.ReactNode;
  additionalAction?: React.ReactNode;
  onConfirm: () => void;
  onCancel: () => void;
}

export const ConfirmDialog: React.FC<ConfirmDialogProps> = ({
  open,
  className,
  title,
  message,
  confirmLabel = "确认删除",
  cancelLabel = "取消",
  variant = "danger",
  acknowledgeLabel,
  busy = false,
  confirmDisabled = false,
  acknowledgementKey,
  error,
  children,
  additionalAction,
  onConfirm,
  onCancel,
}) => {
  const [acknowledged, setAcknowledged] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const messageId = useId();

  useEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement as HTMLElement | null;
    const root = document.getElementById("root");
    const previousInert = root?.inert;
    if (root) root.inert = true;
    modalRef.current?.querySelector<HTMLButtonElement>('.confirm-dialog-actions button')?.focus();
    return () => {
      if (root) root.inert = Boolean(previousInert);
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  }, [open]);

  useEffect(() => {
    if (open) setAcknowledged(false);
  }, [open, acknowledgementKey]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (!busy) onCancel();
      } else if (e.key === "Tab") {
        const controls = Array.from(modalRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') || []);
        const first = controls[0];
        const last = controls[controls.length - 1];
        if (!first) { e.preventDefault(); modalRef.current?.focus(); }
        else if (e.shiftKey && (document.activeElement === first || !modalRef.current?.contains(document.activeElement))) { e.preventDefault(); last.focus(); }
        else if (!e.shiftKey && (document.activeElement === last || !modalRef.current?.contains(document.activeElement))) { e.preventDefault(); first.focus(); }
      }
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [open, onCancel, busy]);

  if (!open || typeof document === "undefined") return null;

  return createPortal(
    <div className={`confirm-dialog-overlay${className ? ` ${className}` : ""}`} onClick={() => { if (!busy) onCancel(); }}>
      <div
        ref={modalRef}
        className="confirm-dialog-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={messageId}
        aria-busy={busy}
        tabIndex={-1}
        onKeyDown={(event) => event.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="confirm-dialog-icon-wrapper" aria-hidden="true">
          <AlertTriangle size={20} className="confirm-dialog-icon" />
        </div>

        <div className="confirm-dialog-body">
          <div className="confirm-dialog-header">
            <h3 className="confirm-dialog-title" id={titleId}>{title}</h3>
            <button
              type="button"
              className="confirm-dialog-close-btn"
              onClick={onCancel}
              disabled={busy}
              title="关闭"
            >
              <X size={15} />
            </button>
          </div>

          <p className="confirm-dialog-message" id={messageId}>{message}</p>
          {error && <p className="confirm-dialog-error" role="alert">{error}</p>}
          {children}

          {acknowledgeLabel && (
            <label className="confirm-dialog-acknowledge">
              <Checkbox
                checked={acknowledged}
                disabled={busy}
                onChange={setAcknowledged}
              />
              <span>{acknowledgeLabel}</span>
            </label>
          )}

          <div className="confirm-dialog-actions">
            <Button size="sm" variant="secondary" disabled={busy} onClick={onCancel}>
              {cancelLabel}
            </Button>
            {additionalAction}
            <Button
              size="sm"
              variant={variant === "danger" ? "danger" : "primary"}
              disabled={busy || confirmDisabled || (Boolean(acknowledgeLabel) && !acknowledged)}
              loading={busy}
              onClick={onConfirm}
            >
              {confirmLabel}
            </Button>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
};

export default ConfirmDialog;
