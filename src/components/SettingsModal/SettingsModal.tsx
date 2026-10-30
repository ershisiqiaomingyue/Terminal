import React, { useState, useEffect } from "react";
import {
  X,
  SlidersHorizontal,
  Palette,
  Paintbrush,
  Cpu,
  Puzzle,
  Code2,
  Wrench,
  Bot,
  type LucideIcon,
} from "lucide-react";
import { Button } from "rain-ui";
import { subscribeAppTheme } from "@/platform/theme";
import { GeneralPanel } from "./GeneralPanel";
import { AgentPanel } from "./AgentPanel";
import { TerminalPanel } from "./TerminalPanel";
import { EnvironmentToolsPanel } from "./EnvironmentToolsPanel";
import { LspPanel } from "./LspPanel";
import { PersonalizationPanel } from "./PersonalizationPanel";
import { AgentAssistPanel } from "./AgentAssistPanel";
import { SettingsNotificationProvider } from "./SettingsNotification";
const PluginsPanel = React.lazy(() => import("./PluginsPanel").then((module) => ({ default: module.PluginsPanel })));

/** 新设置壳的八个分区 id（导航与面板渲染都只用这份列表）。 */
export type SettingsSectionId =
  | "general"
  | "appearance"
  | "personalization"
  | "engine"
  | "plugins"
  | "lsp"
  | "environment"
  | "agent";

/**
 * 对外兼容的分区类型：历史事件 payload 与旧调用方（AppMenubar、QuickToolPalette、
 * ComposerAddMenu、SystemStatusIndicator）仍会传旧值，运行时由
 * normalizeSettingsSection 归一到 SettingsSectionId，这里保留旧值让类型不破。
 */
export type SettingsSection =
  | SettingsSectionId
  | "terminal"
  | "ssh"
  | "wsl"
  | "network"
  | "skills"
  | "project-automation"
  | "about";

const SETTINGS_SECTION_IDS: readonly SettingsSectionId[] = [
  "general",
  "appearance",
  "personalization",
  "engine",
  "plugins",
  "lsp",
  "environment",
  "agent",
];

/** 旧 section 值 → 新分区。terminal 并入 appearance，ssh/wsl/skills/project-automation/about/network 并入 general，agent 改指引擎。 */
const LEGACY_SECTION_MAP: Partial<Record<string, SettingsSectionId>> = {
  terminal: "appearance",
  ssh: "general",
  wsl: "general",
  network: "general",
  skills: "general",
  "project-automation": "general",
  about: "general",
  agent: "engine",
};

/** 事件 payload / props 里的 section 归一化：旧值映射、未知值回落到 general。 */
export const normalizeSettingsSection = (section?: string | null): SettingsSectionId => {
  if (!section) return "general";
  const legacy = LEGACY_SECTION_MAP[section];
  if (legacy) return legacy;
  return (SETTINGS_SECTION_IDS as readonly string[]).includes(section)
    ? (section as SettingsSectionId)
    : "general";
};

const NAVIGATION_ITEMS: { id: SettingsSectionId; label: string; icon: LucideIcon }[] = [
  { id: "general", label: "常规", icon: SlidersHorizontal },
  { id: "appearance", label: "外观", icon: Palette },
  { id: "personalization", label: "个性化", icon: Paintbrush },
  { id: "engine", label: "引擎", icon: Cpu },
  { id: "plugins", label: "插件", icon: Puzzle },
  { id: "lsp", label: "LSP", icon: Code2 },
  { id: "environment", label: "环境", icon: Wrench },
  { id: "agent", label: "Agent", icon: Bot },
];

interface SettingsModalProps {
  isOpen: boolean;
  section?: SettingsSection;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, section, onClose }) => {
  const [activeSection, setActiveSection] = useState<SettingsSectionId>(normalizeSettingsSection(section));

  useEffect(() => {
    if (isOpen && section) setActiveSection(normalizeSettingsSection(section));
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
    return () => {
      unsubscribeTheme();
    };
  }, []);

  if (!isOpen) return null;

  const renderActivePanel = () => {
    switch (activeSection) {
      case "general":
        return <GeneralPanel />;
      case "appearance":
        return <TerminalPanel />;
      case "personalization":
        return <PersonalizationPanel />;
      case "engine":
        return <AgentPanel />;
      case "plugins":
        return <React.Suspense fallback={<p className="setting-desc">正在加载插件…</p>}><PluginsPanel /></React.Suspense>;
      case "lsp":
        return <LspPanel />;
      case "environment":
        return <EnvironmentToolsPanel />;
      case "agent":
        return <AgentAssistPanel />;
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
                  <span className="settings-nav-icon">
                    <item.icon size={16} strokeWidth={1.8} />
                  </span>
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
