import { ToolEmptyState } from "rain-ui";
import React from "react";
import { isTauri } from "@/platform/runtime";
import "./tool-window-action.css";

const CodeViewerView = React.lazy(() => import("@/views/CodeViewer").then((module) => ({ default: module.CodeViewerView })));
const TerminalView = React.lazy(() => import("@/views/Terminal").then((module) => ({ default: module.TerminalView })));

export type SupportedToolKey =
  | "code-viewer"
  | "shell";

interface WindowProps {
  type: SupportedToolKey;
  active?: boolean;
  onOpenTool?: (toolKey: SupportedToolKey) => void;
  onBack?: () => void;
}

export const ToolWindowView: React.FC<WindowProps> = ({ type, active = true, onBack }) => {
  let page: React.ReactNode;
  if (type === "code-viewer") {
    page = <CodeViewerView active={active} />;
  } else {
    page = <TerminalView onBack={onBack} />;
  }

  return (
    <div className="tool-window-action-frame">
      {!isTauri() && <ToolEmptyState compact title="等待桌面连接" description="连接桌面应用后显示真实数据。" />}
      <React.Suspense fallback={<div className="app-view-loading">正在载入工具...</div>}>{page}</React.Suspense>
    </div>
  );
};
