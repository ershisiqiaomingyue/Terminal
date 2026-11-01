import React, { useCallback, useState } from "react";
import { Button } from "rain-ui";
import { Terminal, PanelTop, SquareCode, ExternalLink } from "lucide-react";
import { type SupportedToolKey } from "@/views/ToolWindows";
import { ContextMenu } from "rain-ui";
import { ToolIcon } from "rain-ui";
export interface HomeTool {
  id: string;
  title: string;
  subtitle: string;
  desc: string;
  tag: string;
  icon: React.ReactNode;
  iconClass: string;
  toolKey?: SupportedToolKey;
  status?: "ready" | "unavailable" | "planned";
}

export type HomeToolStatus = "ready" | "unavailable" | "planned";

export function homeToolStatus(tool: HomeTool): HomeToolStatus {
  return tool.status || (tool.toolKey ? "ready" : "planned");
}

export function homeToolStatusLabel(status: HomeToolStatus) {
  return status === "ready" ? "可用" : status === "unavailable" ? "不可用" : "待开发";
}

export function homeToolStatusVariant(status: HomeToolStatus): "success" | "danger" | "neutral" {
  return status === "ready" ? "success" : status === "unavailable" ? "danger" : "neutral";
}

interface HomeViewProps {
  onNavigateToTable?: () => void;
  onOpenTool?: (toolId: SupportedToolKey) => void;
  onOpenToolWindow?: (toolId: SupportedToolKey) => void;
}

// 终端应用的工具清单，同时作为全局快捷工具面板的数据源。
export const HOME_TOOLS: HomeTool[] = [
  {
    id: "code-viewer", title: "代码查看", subtitle: "本地 / SSH 目录树",
    desc: "按需展开本地工程或 SSH 目录树，查看源码和 Markdown，可在新标签或独立窗口打开。",
    tag: "已集成", icon: <SquareCode size={18} />, iconClass: "icon-indigo", toolKey: "code-viewer",
  },
  {
    id: "terminal", title: "终端", subtitle: "本地 / WSL / SSH 多源终端",
    desc: "在一个界面里管理本机、WSL 与 SSH 终端会话，左侧切换、右侧查看主机状态与文件。",
    tag: "可用",
    icon: <Terminal size={18} />,
    iconClass: "icon-emerald",
    toolKey: "shell",
  },
];

export const HOME_TOOL_GROUPS = [
  // 终端不在这里：它常驻左上导航栏，右键还能在新窗口打开，主页再列一项只是重复入口。
  { title: "工具", ids: ["code-viewer"] },
].map((group) => ({ ...group, tools: group.ids.map((id) => HOME_TOOLS.find((tool) => tool.id === id)!) }));
export const HomeView: React.FC<HomeViewProps> = ({ onOpenTool, onOpenToolWindow }) => {
  const [menu, setMenu] = useState<{ tool: HomeTool; x: number; y: number; trigger: HTMLButtonElement } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  return (
  <div className="home-view">
    <h1 className="home-title">工具</h1>
    {HOME_TOOL_GROUPS.map((group) => (
      <section className="home-tool-group" key={group.title} aria-label={group.title}>
        <h2>{group.title}</h2>
        <div className="home-tool-list">
          {group.tools.map((tool) => {
            const status = homeToolStatus(tool);
            return (
              <Button
                type="button" variant="bare"
                key={tool.id}
                className={`home-tool-row is-${status}`}
                disabled={status !== "ready"}
                title={`${status === "ready" ? "已实现 · 可用" : status === "unavailable" ? "不可用" : "禁用 · 待开发"}：${tool.desc}`}
                onClick={() => tool.toolKey && onOpenTool?.(tool.toolKey)}
                onContextMenu={(event) => {
                  if (!tool.toolKey || status !== "ready") return;
                  event.preventDefault();
                  const rect = event.currentTarget.getBoundingClientRect();
                  setMenu({ tool, x: event.clientX || rect.left + 16, y: event.clientY || rect.bottom, trigger: event.currentTarget });
                }}
              >
                <ToolIcon className="home-tool-icon">{tool.icon}</ToolIcon>
                <span className="home-tool-copy">
                  <span className="home-tool-title">{tool.title}</span>
                  <span className="home-tool-sub">{tool.subtitle}</span>
                </span>
                <span className={`home-tool-status is-${status}`} role="img" aria-label={status === "planned" ? "禁用 · 待开发" : homeToolStatusLabel(status)} title={status === "ready" ? "已实现 · 可用" : status === "unavailable" ? "不可用" : "禁用 · 待开发"} />
              </Button>
            );
          })}
        </div>
      </section>
    ))}
    {menu && <ContextMenu x={menu.x} y={menu.y} label={`${menu.tool.title}操作`} trigger={menu.trigger} onClose={closeMenu} items={[
      { label: "在当前窗口中打开", icon: <PanelTop />, onSelect: () => onOpenTool?.(menu.tool.toolKey!) },
      { label: "在新窗口中打开", icon: <ExternalLink />, disabled: !onOpenToolWindow, onSelect: () => onOpenToolWindow?.(menu.tool.toolKey!) },
    ]} />}
  </div>
  );
};

export default HomeView;
