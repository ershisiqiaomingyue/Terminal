import { Check } from "lucide-react";
import { useEffect, useRef, type RefObject } from "react";
import "./access-mode-menu.css";

export type AccessMode = "ask" | "risk" | "full";

const modes = [
  { value: "ask", label: "请求批准", description: "执行命令、shell、写入文件和 MCP 操作时始终询问" },
  { value: "risk", label: "替我审批", description: "只读检查自动执行，写入、命令和 shell 请求批准" },
  // G3-D：语义诚实——这是权限模式不是沙盒。full = 完全信任（无隔离）：
  // 模型生成的命令与你本人的权限完全相同，没有任何强制边界兜底。
  { value: "full", label: "完全信任（无隔离）", description: "不再逐次询问；无沙盒隔离——模型以你的完整权限执行，请仅在可信环境使用" },
] as const;

/** Selection only: the caller retains approval and full-access confirmation. */
export function AccessModeMenu({ value, onChange, inline = false, onClose, triggerRef }: {
  value: AccessMode;
  onChange: (value: AccessMode) => void;
  inline?: boolean;
  onClose?: () => void;
  triggerRef?: RefObject<HTMLElement>;
}) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (inline || !onClose) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!root.current?.contains(target) && !triggerRef?.current?.contains(target)) onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onClose();
      triggerRef?.current?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
    };
  }, [inline, onClose, triggerRef]);
  return <div ref={root} className={`ui-access-menu${inline ? " is-inline" : ""}`} role="group" aria-label="Agent 操作审批方式">
    <div className="ui-access-menu-heading">应如何批准 Agent 操作？</div>
    {modes.map((mode) => <button key={mode.value} type="button" className={`ui-access-menu-row${value === mode.value ? " is-selected" : ""}${mode.value === "full" ? " is-full" : ""}`}
      aria-pressed={value === mode.value} onClick={() => onChange(mode.value)}>
      <span><b>{mode.label}</b><small>{mode.description}</small></span>
      {value === mode.value && <Check size={16} aria-hidden="true" />}
    </button>)}
  </div>;
}
