import React, { useMemo, useState, useEffect } from "react";
import {
  Settings,
  SquareTerminal,
  Palette,
  Bot,
  Puzzle,  Braces,
  Wrench,
  Search,
  type LucideIcon,
} from "lucide-react";
import { Button, Input, PortalProvider, cn } from "rain-ui";
import { subscribeAppTheme } from "@/platform/theme";
import { GeneralPanel } from "./GeneralPanel";
import { AgentPanel } from "./AgentPanel";
import { TerminalPanel } from "./TerminalPanel";
import { EnvironmentToolsPanel } from "./EnvironmentToolsPanel";
import { LspPanel } from "./LspPanel";
import { PersonalizationPanel } from "./PersonalizationPanel";
const PluginsPanel = React.lazy(() => import("./PluginsPanel").then((module) => ({ default: module.PluginsPanel })));

/** 新设置壳的七个分区 id（导航与面板渲染都只用这份列表）。 */
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

/**
 * 左侧导航按功能层级分组：一级是分组标题（个人 / AI / 集成），二级才是可点的
 * 分区。分区 id 保持稳定（历史事件仍用旧值），显示名与右侧面板的 pane-title
 * 一一对应，避免「导航叫外观、内容其实是终端」这类错位。
 */
const NAV_GROUPS: { label: string; items: { id: SettingsSectionId; label: string; icon: LucideIcon }[] }[] = [
  {
    label: "个人",
    items: [
      { id: "general", label: "常规", icon: Settings },
      { id: "appearance", label: "终端", icon: SquareTerminal },
      { id: "personalization", label: "个性化", icon: Palette },
    ],
  },
  {
    label: "AI",
    items: [
      { id: "engine", label: "Agent 与模型", icon: Bot },
    ],
  },
  {
    label: "集成",
    items: [
      { id: "plugins", label: "插件", icon: Puzzle },
      { id: "lsp", label: "语言服务器", icon: Braces },
      { id: "environment", label: "环境", icon: Wrench },
    ],
  },
];

interface SettingsModalProps {
  isOpen: boolean;
  section?: SettingsSection;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, section, onClose }) => {
  const [activeSection, setActiveSection] = useState<SettingsSectionId>(normalizeSettingsSection(section));
  const [query, setQuery] = useState("");

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

  const visibleGroups = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return NAV_GROUPS;
    return NAV_GROUPS
      .map((group) => ({
        ...group,
        items: group.items.filter(
          (item) => item.label.toLowerCase().includes(keyword) || group.label.toLowerCase().includes(keyword),
        ),
      }))
      .filter((group) => group.items.length > 0);
  }, [query]);

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
        return (
          <React.Suspense fallback={<p className="px-5 py-4 text-[13px] text-muted-foreground">正在加载插件…</p>}>
            <PluginsPanel />
          </React.Suspense>
        );
      case "lsp":
        return <LspPanel />;
      case "environment":
        return <EnvironmentToolsPanel />;
      default:
        return <GeneralPanel />;
    }
  };

  return (
    <div className="relative z-10 flex h-full min-h-0 flex-1 flex-col bg-background text-foreground">
      <PortalProvider position="bottom-center" appearance="solid">
        {/* 主体左右双栏：导航与内容同顶距对齐，返回走标题栏 ← 或 Esc */}
        <div className="flex min-h-0 flex-1">
          <nav
            aria-label="设置分类"
            className="flex w-[238px] shrink-0 flex-col overflow-y-auto border-r border-border/70 bg-background px-2 pb-6 pt-5"
          >
            <div className="px-2 pb-4 text-[19px] font-bold tracking-tight">设置</div>
            <div className="pb-5">
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                size="lg"
                prefixIcon={<Search size={15} className="text-muted-foreground" />}
                placeholder="搜索"
                aria-label="搜索设置"
              />
            </div>

            {visibleGroups.map((group) => (
              <div key={group.label}>
                <div className="px-2 pb-1.5 pt-4 text-[13px] font-medium text-muted-foreground">{group.label}</div>
                <div className="flex flex-col gap-0.5">
                  {group.items.map((item) => {
                    const Icon = item.icon;
                    const active = activeSection === item.id;
                    return (
                      <Button
                        key={item.id}
                        type="button"
                        variant="ghost"
                        size="sm"
                        className={cn(
                          "h-[32px] w-full justify-start rounded-[8px] px-2.5 text-[13px] font-normal leading-5 active:translate-y-0 active:scale-100 active:bg-foreground/[0.05]",
                          active && "bg-foreground/[0.06] hover:bg-foreground/[0.06]",
                        )}
                        onClick={() => setActiveSection(item.id)}
                      >
                        <span className="flex min-w-0 items-center gap-2.5 leading-5">
                          <Icon className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.8} aria-hidden />
                          <span className="truncate">{item.label}</span>
                        </span>
                      </Button>
                    );
                  })}
                </div>
              </div>
            ))}
            {visibleGroups.length === 0 && (
              <p className="px-2.5 py-3 text-[13px] text-muted-foreground">没有匹配「{query.trim()}」的设置项</p>
            )}
          </nav>

          <main className="min-w-0 flex-1 overflow-y-scroll bg-background px-8 pt-5 [scrollbar-gutter:stable]">
            <div className="mx-auto w-full max-w-[728px]">{renderActivePanel()}</div>
          </main>
        </div>
      </PortalProvider>
    </div>
  );
};

export default SettingsModal;


