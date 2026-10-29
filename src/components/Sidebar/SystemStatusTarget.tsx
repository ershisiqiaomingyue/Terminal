import type { ReactNode } from "react";
import { ChevronRight, Square, SquareCheck } from "lucide-react";
import { Button } from "@/components/ui/Button";

const statusLabels: Record<string, string> = {
  connected: "已连接", connecting: "连接中", failed: "连接失败", disconnected: "已断开", idle: "未连接",
};

/** Keep each environment's identity, connection state and details in one item. */
export function SystemStatusTarget({ title, subtitle, description, icon, selected, status, latency, disabled, bound, onSelect, onToggleBind, children }: {
  title: string; subtitle?: string; description?: string; icon: ReactNode; selected: boolean; status?: string;
  latency?: string | null;
  disabled: boolean; onSelect: () => void; children?: ReactNode;
  /** 是否已绑定到当前对话；只有传入 `onToggleBind` 时才渲染绑定勾。 */
  bound?: boolean; onToggleBind?: () => void;
}) {
  const secondary = subtitle?.trim() !== title.trim() ? subtitle : undefined;
  const statusLabel = status ? statusLabels[status] : undefined;
  const bindable = Boolean(onToggleBind);
  const bindLabel = bound ? "已绑定到当前对话，点击解除绑定" : "绑定到当前对话";
  return <section className={`system-environment-target${selected ? " is-selected" : ""}${bindable ? " is-bindable" : ""}`}>
    <Button variant="subtle" size="xs" className="system-environment-target-button" aria-pressed={selected}
      aria-label={description || (secondary ? `${title}，${secondary}` : title)} disabled={disabled} onClick={onSelect}>
      <span className="system-environment-target-icon" aria-hidden="true">{icon}</span>
      <span className="system-environment-target-copy"><b>{title}</b>{secondary && <small>{secondary}</small>}</span>
      {statusLabel ? <span className="system-environment-target-status">
        <span className={`system-environment-target-state is-${status}`}><i aria-hidden="true" />{statusLabel}</span>
        {status === "connected" && latency && <small className="system-environment-latency" title={`连接延迟 ${latency}`}>{latency}</small>}
      </span> : <ChevronRight size={13} />}
    </Button>
    {bindable && <Button variant="subtle" size="xs" className={`system-environment-target-bind${bound ? " is-bound" : ""}`}
      aria-pressed={bound} aria-label={bindLabel} title={bindLabel} disabled={disabled} onClick={() => onToggleBind?.()}>
      {bound ? <SquareCheck size={13} /> : <Square size={13} />}
    </Button>}
    {children && <div className="system-environment-target-detail">{children}</div>}
  </section>;
}

