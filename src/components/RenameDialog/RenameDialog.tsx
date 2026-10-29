import React, { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Modal } from "@/components/ui/Modal";
import "./rename-dialog.css";

export interface RenameDialogProps {
  open: boolean;
  title: string;
  description?: string;
  /** 输入框的无障碍标签，也当占位文案之外的语义说明。 */
  label: string;
  initialValue: string;
  confirmLabel: string;
  busy?: boolean;
  error?: string;
  onCancel: () => void;
  onConfirm: (value: string) => void;
}

/**
 * 重命名用的单输入弹窗。
 *
 * 之前是行内输入框：一改名整行就从「标题 + 时间 + 三个按钮」塌成一根输入框，
 * 行高和右侧动作一起抖，而且光标进不去就要先点两次。改成弹窗后行本身不动，
 * 输入框独占焦点、内容默认全选，回车即提交。
 */
export const RenameDialog: React.FC<RenameDialogProps> = ({
  open,
  title,
  description,
  label,
  initialValue,
  confirmLabel,
  busy = false,
  error,
  onCancel,
  onConfirm,
}) => {
  const [draft, setDraft] = useState(initialValue);
  // 中文输入法敲回车是「选字」，不是「提交」——合成期间一律不响应回车。
  const composingRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setDraft(initialValue);
    composingRef.current = false;
    // 全选要等草稿落进 DOM 再做：Modal 挂载时已经聚焦过输入框，这里若只挂 onFocus，
    // 值一写进去选区就被顶到末尾，「打开即全选」等于白做。
    const frame = window.requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open, initialValue]);

  const trimmed = draft.trim();
  // 允许和原名一样：用户可能只是想点开确认一下，没必要拦着。
  const blocked = busy || trimmed === "";
  const submit = () => { if (!blocked) onConfirm(trimmed); };

  return <Modal
    open={open}
    size="sm"
    title={title}
    description={description}
    busy={busy}
    onClose={onCancel}
    footer={<>
      <Button size="sm" variant="secondary" disabled={busy} onClick={onCancel}>取消</Button>
      <Button size="sm" variant="primary" disabled={blocked} loading={busy} onClick={submit}>{confirmLabel}</Button>
    </>}
  >
    <input
      ref={inputRef}
      className="rename-dialog-input"
      value={draft}
      aria-label={label}
      disabled={busy}
      spellCheck={false}
      onChange={(event) => setDraft(event.target.value)}
      onCompositionStart={() => { composingRef.current = true; }}
      onCompositionEnd={() => { composingRef.current = false; }}
      onKeyDown={(event) => {
        if (event.key !== "Enter") return;
        event.preventDefault();
        if (composingRef.current) return;
        submit();
      }}
    />
    {error && <p className="rename-dialog-error" role="alert">{error}</p>}
  </Modal>;
};

export default RenameDialog;
