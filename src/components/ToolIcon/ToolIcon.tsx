import type { ReactNode } from "react";
import "./tool-icon.css";

interface ToolIconProps {
  children: ReactNode;
  className?: string;
}

export function ToolIcon({ children, className = "" }: ToolIconProps) {
  return <span className={`tool-icon ${className}`} aria-hidden="true">{children}</span>;
}
