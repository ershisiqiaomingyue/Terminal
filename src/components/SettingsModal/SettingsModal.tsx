import React, { useState, useEffect } from "react";
import {
  X,
  SlidersHorizontal,
  ShieldCheck,
  Server,
  Terminal,
  Wrench,
  Sparkles,
  Info,
  Sliders,
  Boxes,
  Network,
  Puzzle,
  Code2,
  SquareTerminal,
} from "lucide-react";
import { subscribeAppTheme } from "@/platform/theme";
import { Button } from "@/components/ui/Button";
import { subscribeLogcatBuffer } from "@/platform/logcatBuffer";
import { GeneralPanel } from "./GeneralPanel";
import { AgentPanel } from "./AgentPanel";
import { SshPanel } from "./SshPanel";
import { WslPanel } from "./WslPanel";
import { TerminalPanel } from "./TerminalPanel";
import { EnvironmentToolsPanel } from "./EnvironmentToolsPanel";
import { NetworkPanel } from "./NetworkPanel";
import { SkillsPanel } from "./SkillsPanel";
import { AboutPanel } from "./AboutPanel";
import { ToolsPanel } from "./ToolsPanel";
import { ProjectAutomationPanel } from "./ProjectAutomationPanel";
import { LspPanel } from "./LspPanel";
import { SettingsNotificationProvider } from "./SettingsNotification";
const PluginsPanel = React.lazy(() => import("./PluginsPanel").then((module) => ({ default: module.PluginsPanel })));

interface SettingsModalProps {
  isOpen: boolean;
  section?: SettingsSection;
  onClose: () => void;
}

export type SettingsSection = "general" | "tools" | "terminal" | "agent" | "ssh" | "wsl" | "network" | "environment" | "lsp" | "skills" | "plugins" | "project-automation" | "about";

const NAVIGATION_ITEMS = [
  { id: "general" as const, label: "常规", icon: SlidersHorizontal, tint: "tint-blue" },
  { id: "tools" as const, label: "工具", icon: Sliders, tint: "tint-green" },
  { id: "terminal" as const, label: "终端", icon: SquareTerminal, tint: "tint-cyan" },
  { id: "agent" as const, label: "Agent 接入", icon: ShieldCheck, tint: "tint-cyan" },
  { id: "ssh" as const, label: "SSH 远程", icon: Server, tint: "tint-green" },
  { id: "wsl" as const, label: "WSL", icon: Terminal, tint: "tint-purple" },
  { id: "network" as const, label: "网络", icon: Network, tint: "tint-cyan" },
  { id: "environment" as const, label: "环境与工具", icon: Wrench, tint: "tint-blue" },
  { id: "lsp" as const, label: "语言服务器", icon: Code2, tint: "tint-cyan" },
  { id: "skills" as const, label: "Skills", icon: Sparkles, tint: "tint-amber" },
  { id: "plugins" as const, label: "插件", icon: Puzzle, tint: "tint-purple" },
  { id: "project-automation" as const, label: "项目与自动化", icon: Boxes, tint: "tint-blue" },
  { id: "about" as const, label: "关于", icon: Info, tint: "tint-rose" },
] as const;

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, section, onClose }) => {
  const [activeSection, setActiveSection] = useState<SettingsSection>(section || "general");

  useEffect(() => {
    if (isOpen && section) setActiveSection(section);
  }, [isOpen, section]);

  useEffect(() => {
    if (!isOpen) return;
    // Inner dialogs and selects consume Escape before this bubbling listener.
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener("keydown", handleEscape);
    return () => document.removeEventListener("keydown", handleEscape);
  }, [isOpen, onClose]);

  // Subscribe to global settings changes
  useEffect(() => {
    const unsubscribeTheme = subscribeAppTheme(() => {});
    const unsubscribeBuffer = subscribeLogcatBuffer(() => {});
    return () => {
      unsubscribeTheme();
      unsubscribeBuffer();
    };
  }, []);

  if (!isOpen) return null;

  const renderActivePanel = () => {
    switch (activeSection) {
      case "general":
        return <GeneralPanel />;
      case "agent":
        return <AgentPanel />;
      case "tools":
        return <ToolsPanel />;
      case "terminal":
        return <TerminalPanel />;
      case "ssh":
        return <SshPanel />;
      case "wsl":
        return <WslPanel />;
      case "network":
        return <NetworkPanel />;
      case "environment":
        return <EnvironmentToolsPanel />;
      case "lsp":
        return <LspPanel />;
      case "skills":
        return <SkillsPanel />;
      case "plugins":
        return <React.Suspense fallback={<p className="setting-desc">正在加载插件…</p>}><PluginsPanel /></React.Suspense>;
      case "project-automation":
        return <ProjectAutomationPanel />;
      case "about":
        return <AboutPanel />;
      default:
        return <GeneralPanel />;
    }
  };

  return (
    <div
      className="settings-modal-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="settings-modal-dialog" role="dialog" aria-modal="true">
        <SettingsNotificationProvider>
          {/* 顶部标题栏 */}
          <header className="settings-modal-header">
            <div className="settings-modal-header-title">
              <SlidersHorizontal size={16} />
              <span>设置</span>
            </div>
            <Button
              type="button"
              variant="subtle"
              size="sm"
              icon={<X size={15} />}
              onClick={onClose}
              aria-label="关闭设置"
              title="关闭设置 (Esc)"
            />
          </header>

          {/* 主体左右双栏结构 */}
          <div className="settings-dialog-body">
            {/* 左侧导航栏 */}
            <nav className="settings-dialog-nav" aria-label="设置分类">
              <div className="settings-nav-caption">应用设置</div>
              {NAVIGATION_ITEMS.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={`settings-nav-item ${activeSection === item.id ? "active" : ""}`}
                  onClick={() => setActiveSection(item.id)}
                >
                  <div className={`icon-tinted ${item.tint}`}>
                    <item.icon size={15} />
                  </div>
                  <span className="nav-item-label">{item.label}</span>
                </button>
              ))}
            </nav>

            {/* 右侧面板内容主视口 */}
            <main className="settings-dialog-content">
              {renderActivePanel()}
            </main>
          </div>
        </SettingsNotificationProvider>
      </div>
    </div>
  );
};

export default SettingsModal;
