import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { Button } from "./Button";
import { formatInvokeError } from "@/platform/runtime";

/**
 * Toast / Notification
 * ---------------------------------------------------------------------------
 * One stack for the whole app. Before this existed each page carried its own
 * copy (adb-dev-toast-pill, aosp-toast-pill, archive-toast-pill,
 * terminal-toast, settings-notification) with the same tokens but different
 * timings and dismissal rules, so the same event looked different depending on
 * where it happened.
 *
 * Tone is carried by the icon only; the surface stays neutral so a queue of
 * mixed messages does not turn into a row of coloured slabs. Errors live
 * longer than confirmations because they carry something to act on.
 *
 * The action is a library Button (`secondary` / `xs`): quiet accent glass, the
 * only variant that reads as an action on a white toast plate. The close glyph
 * is the plain icon-button treatment — nothing at rest, red on hover — the same
 * one the attachment remove button uses, so the two never compete.
 */

export type ToastTone = "success" | "error" | "info";

export type ToastOptions = {
  message: unknown;
  tone?: ToastTone;
  /** Milliseconds on screen. Defaults to 3000, or 4800 for errors. */
  duration?: number;
  /** Optional single action, e.g. undo. Not used for confirmation dialogs. */
  action?: { label: string; onClick: () => void };
};

type ToastEntry = Required<Pick<ToastOptions, "tone">> &
  Pick<ToastOptions, "action"> & { id: string; message: string };

type ToastApi = {
  push: (options: ToastOptions) => string;
  dismiss: (id: string) => void;
  success: (message: unknown) => string;
  error: (message: unknown) => string;
  info: (message: unknown) => string;
};

const ToastContext = createContext<ToastApi | null>(null);

const TONE_ICON: Record<ToastTone, React.ReactNode> = {
  success: <CheckCircle2 size={15} />,
  error: <AlertCircle size={15} />,
  info: <Info size={15} />,
};

/** Visible cards at once. Older ones are dropped, not queued forever. */
const MAX_VISIBLE = 3;

export const ToastProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const timers = useRef<Record<string, number>>({});

  const dismiss = useCallback((id: string) => {
    const timer = timers.current[id];
    if (timer) {
      window.clearTimeout(timer);
      delete timers.current[id];
    }
    setToasts((previous) => previous.filter((toast) => toast.id !== id));
  }, []);

  const push = useCallback<ToastApi["push"]>(
    ({ message, tone = "info", duration, action }) => {
      const id = `toast-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
      // 后端结构化错误是普通对象，直接渲染会变成「[object Object]」，统一归一化。
      const text = typeof message === "string" ? message : formatInvokeError(message);
      setToasts((previous) => [...previous, { id, message: text, tone, action }].slice(-MAX_VISIBLE));
      timers.current[id] = window.setTimeout(
        () => dismiss(id),
        duration ?? (tone === "error" ? 4800 : 3000),
      );
      return id;
    },
    [dismiss],
  );

  useEffect(
    () => () => {
      Object.values(timers.current).forEach((timer) => window.clearTimeout(timer));
    },
    [],
  );

  const api = useMemo<ToastApi>(
    () => ({
      push,
      dismiss,
      success: (message) => push({ message, tone: "success" }),
      error: (message) => push({ message, tone: "error" }),
      info: (message) => push({ message, tone: "info" }),
    }),
    [push, dismiss],
  );

  return (
    <ToastContext.Provider value={api}>
      {children}
      {createPortal(
        <div className="ui-toast-stack" aria-live="polite" aria-atomic="false">
          {toasts.map((toast) => (
            <div key={toast.id} className={`ui-toast is-${toast.tone}`} role="status">
              <span className="ui-toast-icon">{TONE_ICON[toast.tone]}</span>
              <span className="ui-toast-text">{toast.message}</span>
              {toast.action && (
                <Button
                  type="button"
                  variant="secondary"
                  size="xs"
                  className="ui-toast-action"
                  onClick={() => {
                    toast.action?.onClick();
                    dismiss(toast.id);
                  }}
                >
                  {toast.action.label}
                </Button>
              )}
              <Button
                type="button"
                variant="subtle"
                size="xs"
                className="ui-toast-close"
                icon={<X size={13} />}
                aria-label="关闭提示"
                onClick={() => dismiss(toast.id)}
              />
            </div>
          ))}
        </div>,
        document.body,
      )}
    </ToastContext.Provider>
  );
};

/**
 * Reads the toast API. Throws when no provider is mounted so a missing
 * provider surfaces at the call site instead of silently swallowing messages.
 */
export function useToast(): ToastApi {
  const api = useContext(ToastContext);
  if (!api) throw new Error("useToast must be used inside <ToastProvider>");
  return api;
}

/** Presentational card, for the component gallery and for static previews. */
export const ToastCard: React.FC<{
  tone?: ToastTone;
  message: string;
  actionLabel?: string;
  onClose?: () => void;
}> = ({ tone = "info", message, actionLabel, onClose }) => (
  <div className={`ui-toast is-${tone} is-static`} role="status">
    <span className="ui-toast-icon">{TONE_ICON[tone]}</span>
    <span className="ui-toast-text">{message}</span>
    {actionLabel && (
      <Button type="button" variant="secondary" size="xs" className="ui-toast-action">
        {actionLabel}
      </Button>
    )}
    <Button
      type="button"
      variant="subtle"
      size="xs"
      className="ui-toast-close"
      icon={<X size={13} />}
      aria-label="关闭提示"
      onClick={onClose}
    />
  </div>
);
