import React from "react";
import { CircleDashed } from "lucide-react";
import "../ToolEmptyState/tool-empty-state.css";

export const ToolEmptyState: React.FC<{ title: string; description?: string; compact?: boolean }> = ({ title, description, compact }) => (
  <div className={`tool-empty-state${compact ? " compact" : ""}`} role="status">
    <CircleDashed size={compact ? 16 : 22} aria-hidden="true" />
    <div><span className="tool-empty-title">{title}</span>{description && <p>{description}</p>}</div>
  </div>
);
