import type { MouseEvent, ReactNode } from "react";
import { X } from "lucide-react";
import { Button } from "./Button";
import "../SessionTabs/session-tabs.css";

interface SessionTabsProps {
  tabs: { id: string; label: ReactNode; closeLabel?: string }[];
  activeId: string;
  onSelect: (id: string) => void;
  onClose?: (id: string, event: MouseEvent<HTMLButtonElement>) => void;
  label: string;
  className?: string;
}

/** Shared session tabs keep selection and close actions separately keyboard accessible. */
export function SessionTabs({ tabs, activeId, onSelect, onClose, label, className = "" }: SessionTabsProps) {
  return <div className={`session-tabs ${className}`} role="group" aria-label={label}>
    {tabs.map((tab) => <div key={tab.id} className={`session-tab${tab.id === activeId ? " active" : ""}`}>
      <Button type="button" variant="bare" className="session-tab-select" aria-pressed={tab.id === activeId} onClick={() => onSelect(tab.id)}>{tab.label}</Button>
      {onClose && <Button type="button" variant="bare" className="session-tab-close" icon={<X size={12} />} title={tab.closeLabel || "关闭标签"} aria-label={tab.closeLabel || "关闭标签"} onClick={(event) => onClose(tab.id, event)} />}
    </div>)}
  </div>;
}
