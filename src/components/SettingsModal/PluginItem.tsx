import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { Badge, type BadgeProps } from "@/components/ui/Badge";

interface PluginItemProps {
  name: string;
  icon: ReactNode;
  version?: string | null;
  status: string;
  variant?: BadgeProps["variant"];
  children: ReactNode;
  onToggle?: (open: boolean) => void;
}

export function PluginItem({ name, icon, version, status, variant = "neutral", children, onToggle }: PluginItemProps) {
  return <details className="plugin-entry" onToggle={event => { if (event.target === event.currentTarget) onToggle?.(event.currentTarget.open); }}>
    <summary className="plugin-summary">
      <span className="plugin-item-icon" aria-hidden="true">{icon}</span>
      <span className="plugin-item-name">{name}</span>
      {version && <span className="plugin-version">v{version}</span>}
      <span className="plugin-state"><Badge variant={variant}>{status}</Badge></span>
      <ChevronRight size={15} className="plugin-chevron" aria-hidden="true" />
    </summary>
    <div className="plugin-content">{children}</div>
  </details>;
}
