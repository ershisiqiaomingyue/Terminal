import { useEffect, useRef, useState } from "react";
import { formatInvokeError } from "@/platform/runtime";
import { ConfirmDialog } from "./ConfirmDialog";

interface ConfirmAction {
  title: string;
  message: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  /** 取消时的回调（如抢占确认取消后恢复拖拽前的比例）。 */
  onCancel?: () => void;
}

export function useConfirmAction() {
  const [pending, setPending] = useState<ConfirmAction | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const executing = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  const confirmAction = (action: ConfirmAction) => {
    if (executing.current) return;
    setError("");
    setPending(action);
  };
  const execute = async () => {
    if (!pending || executing.current) return;
    executing.current = true;
    setBusy(true);
    setError("");
    try {
      await pending.onConfirm();
      if (mounted.current) setPending(null);
    } catch (reason) {
      if (mounted.current) setError(formatInvokeError(reason));
    } finally {
      executing.current = false;
      if (mounted.current) setBusy(false);
    }
  };

  return { confirmAction, confirmationDialog: <ConfirmDialog
    open={Boolean(pending)} title={pending?.title || "确认操作"}
    message={pending?.message || ""} confirmLabel={pending?.confirmLabel}
    busy={busy} error={error} onConfirm={() => { void execute(); }}
    onCancel={() => { if (!executing.current) { pending?.onCancel?.(); setPending(null); } }}
  /> };
}
