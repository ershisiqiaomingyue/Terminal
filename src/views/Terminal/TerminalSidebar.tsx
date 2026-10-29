import React, { useEffect, useRef, useState } from "react";
import { ChevronDown, Plus, RefreshCw, SquareTerminal, X } from "lucide-react";
import type { SavedServerEntry } from "@/platform/terminalWorkspace";
import { sshServerDotStatus, useGlobalSshConnection } from "@/platform/globalSsh";
import { Badge, Button, ListItem } from "@/components/ui";
import { TerminalGlyph, type TerminalGlyphKind } from "./TerminalGlyphs";
import { startPointerDrag } from "./pointerDrag";
import type { TerminalTab } from "./types";
import "./sidebar-drag.css";

interface TerminalSidebarProps {
  tabs: TerminalTab[];
  activeTabId: string;
  savedServers: SavedServerEntry[];
  savedServersLoading: boolean;
  /** 是否有在线 ADB 设备（决定「新建 ADB 会话」是否可点） */
  hasAdbDevice: boolean;
  onSelectTab: (id: string) => void;
  onCloseTab: (event: React.MouseEvent, id: string) => void;
  onAddLocal: (shell: "pwsh" | "cmd") => void;
  onAddWsl: () => void;
  onAddSsh: () => void;
  onAddAdb: () => void;
  onUseSavedServer: (server: SavedServerEntry) => void;
  onRefreshServers: () => void;
  /** 终端卡片右键菜单（需求：左侧终端项可右键）。 */
  onTabContextMenu?: (event: React.MouseEvent, tabId: string) => void;
  /**
   * 指针拖拽（v3）：把页签卡片拖到中栏某块面板上松手。
   *
   * 之所以放弃 HTML5 DnD：Tauri 在 Windows WebView2 上默认接管拖放
   * （`dragDropEnabled` 默认开，供聊天/代码查看器接收文件拖入），原生处理器会
   * 吞掉页面内 dragover/drop——左栏拖到面板在真机上完全无响应。改为指针事件
   * 自绘拖拽（阈值启动 + 命中测试 + dataset 高亮），与内核/WebView 行为解耦。
   */
  onDropTabAt?: (slot: number, tabId: string, xRatio: number, yRatio: number) => void;
  /** 指针拖拽：把已保存服务器拖到某块面板上，在该槽位新开连接。 */
  onDropSavedServerAt?: (slot: number, kind: "ssh" | "wsl", id: string) => void;
  /**
   * 已保存服务器右键菜单（需求 ⑧：在新终端打开 / 复制地址 / 重命名 / 删除）。
   *
   * 菜单本身由调用方渲染（`index.tsx` 已有 `ContextMenu` 组件与定位逻辑），
   * 这里只负责把「在哪个条目上按了右键」和事件原样递出去 —— 侧栏不知道
   * 菜单长什么样，也不持有菜单开合状态，避免常驻组件多一份 UI 状态。
   * 不传则右键走浏览器默认行为（不 preventDefault）。
   */
  onServerContextMenu?: (event: React.MouseEvent, server: SavedServerEntry) => void;
}

const TYPE_LABEL: Record<TerminalTab["type"], string> = {
  local: "本机",
  adb: "ADB",
  wsl: "WSL",
  ssh: "SSH",
};

/**
 * `+` 按钮打开的默认类型 = 用户上一次新建的类型。
 * 434 行的一次性交互里，连续开三个 cmd 是常态；每次都回到 PowerShell 要手点两次。
 */
const LAST_KIND_KEY = "android-tool.terminal-last-kind";
type LocalKind = "pwsh" | "cmd" | "wsl";

const KIND_LABEL: Record<LocalKind, string> = {
  pwsh: "PowerShell 7",
  cmd: "命令提示符",
  wsl: "WSL 环境",
};

function readLastKind(): LocalKind {
  try {
    const stored = localStorage.getItem(LAST_KIND_KEY);
    if (stored === "cmd" || stored === "wsl" || stored === "pwsh") return stored;
  } catch {
    // localStorage 在隐私模式/配额满时会抛，读不到就用默认值。
  }
  return "pwsh";
}

function writeLastKind(kind: LocalKind): void {
  try {
    localStorage.setItem(LAST_KIND_KEY, kind);
  } catch {
    // 写失败不影响本次新建，只是下次记不住。
  }
}

/** 会话行左侧图标：本地终端按 shell 区分，远端按类型。 */
function tabGlyph(tab: TerminalTab): TerminalGlyphKind {
  if (tab.type === "local") return tab.shell === "cmd" ? "cmd" : "pwsh";
  return tab.type;
}

/** 左栏：新建终端入口 + 已打开的终端 + 已保存服务器（4.1 ListItem 化）。 */
export const TerminalSidebar: React.FC<TerminalSidebarProps> = ({
  tabs,
  activeTabId,
  savedServers,
  savedServersLoading,
  hasAdbDevice,
  onSelectTab,
  onCloseTab,
  onAddLocal,
  onAddWsl,
  onAddSsh,
  onAddAdb,
  onUseSavedServer,
  onRefreshServers,
  onTabContextMenu,
  onServerContextMenu,
  onDropTabAt,
  onDropSavedServerAt,
}) => {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [lastKind, setLastKind] = useState<LocalKind>(() => readLastKind());
  // 每台已保存服务器各自的连接态（v3 R8 反向联动）：与顶栏呼吸灯读同一份
  // globalSsh 快照（含 PTY 直起 ssh 的本地覆盖），绿=已连接、黄=连接中；
  // WSL 不参与（呼吸灯只代表 SSH）。订阅挂在组件上，不新增任何轮询。
  const globalSsh = useGlobalSshConnection();

  /**
   * 拖动中的视觉反馈走**直接改 dataset**，不走 React state。
   *
   * 理由：侧栏是常驻挂载的，一次拖动会给 state 带来 dragstart / dragend 两次
   * 更新，两次都把整棵页签列表重渲一遍；而这里要改的只是「被拖那一项」自己的
   * 两个属性。直接写 dataset 零重渲，且 React 不管 `data-dragging`
   * （它没有在 render 里输出该属性），后续重渲也不会把值冲掉。
   */
  /**
   * 指针拖拽接线（内核见 pointerDrag.ts）：拖影 + 命中面板高亮 + 槽位落点。
   * 移动超过 5px 才算拖，否则仍是普通点击（点击语义由 ListItem 的 onClick 承担）。
   */
  const beginCardDrag = (
    event: React.PointerEvent<HTMLDivElement>,
    payload: { kind: "tab" | "saved"; id: string; serverKind?: "ssh" | "wsl"; label: string },
  ) => {
    startPointerDrag(event, {
      label: payload.label,
      suppressClickAfter: true,
      onDrop: (target, xRatio, yRatio) => {
        const slot = Number(target.dataset.dropSlot);
        if (!Number.isInteger(slot)) return;
        if (payload.kind === "tab") onDropTabAt?.(slot, payload.id, xRatio, yRatio);
        else onDropSavedServerAt?.(slot, payload.serverKind ?? "ssh", payload.id);
      },
    });
  };

  // 点击菜单外部或按 Esc 关闭新建菜单。
  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  /** 记一次「上次类型」并执行新建。 */
  const createLocal = (kind: LocalKind) => {
    setLastKind(kind);
    writeLastKind(kind);
    if (kind === "wsl") onAddWsl();
    else onAddLocal(kind);
  };

  const pick = (action: () => void) => {
    setMenuOpen(false);
    action();
  };

  return (
    <aside id="terminal-sidebar" className="terminal-sidebar" aria-label="终端列表">
      <div className="terminal-sidebar-header">
        <div className="terminal-sidebar-heading">
          <SquareTerminal size={15} />
          <span>终端</span>
        </div>
        <div className="terminal-add-wrap" ref={menuRef}>
          {/* 两个按钮职责分开（需求 1）：
              `+` 直接开最常用的本地终端，下箭头才展开类型菜单。
              原来是一个按钮同时承担两件事，用户点 `+` 想看菜单、点箭头又想直接开，
              两种预期互相打架。 */}
          <div className="terminal-add-group">
            <button
              type="button"
              className="terminal-add-btn is-primary"
              aria-label={`新建${KIND_LABEL[lastKind]}`}
              title={`新建${KIND_LABEL[lastKind]}（上次使用的类型）`}
              onClick={() => createLocal(lastKind)}
            >
              <Plus size={14} />
            </button>
            <button
              type="button"
              className="terminal-add-btn is-caret"
              aria-label="选择终端类型"
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              title="选择要新建的终端类型"
              onClick={() => setMenuOpen((open) => !open)}
            >
              <ChevronDown size={11} />
            </button>
          </div>
          {menuOpen && (
            // 菜单项一律单行（需求：删掉第二行的说明文字）。
            // 原本每项用「标题 + em 说明」两行，图标按 flex-start 顶对齐，
            // 视觉上图标比文字高半行。现在只留标题、改居中对齐。
            // 原本写在第二行的信息没有丢：类型/不可用原因都挪进 title 悬停提示。
            <div className="terminal-add-menu" role="menu">
              <div className="terminal-add-menu-group">本地环境</div>
              <button
                type="button"
                role="menuitem"
                title="在新终端里启动 pwsh"
                onClick={() => pick(() => createLocal("pwsh"))}
              >
                <TerminalGlyph kind="pwsh" size={16} />
                <span>终端 · PowerShell 7</span>
              </button>
              <button
                type="button"
                role="menuitem"
                title="在新终端里启动 cmd"
                onClick={() => pick(() => createLocal("cmd"))}
              >
                <TerminalGlyph kind="cmd" size={16} />
                <span>命令提示符</span>
              </button>
              <button
                type="button"
                role="menuitem"
                title="按已保存的 WSL 配置打开"
                onClick={() => pick(() => createLocal("wsl"))}
              >
                <TerminalGlyph kind="wsl" size={16} />
                <span>WSL 环境</span>
              </button>
              <div className="terminal-add-menu-divider" />
              <div className="terminal-add-menu-group">服务器与远程</div>
              <button
                type="button"
                role="menuitem"
                title="新建一条 SSH 连接并打开终端"
                onClick={() => pick(onAddSsh)}
              >
                <TerminalGlyph kind="ssh" size={16} />
                <span>SSH 远程服务器</span>
              </button>
              <button
                type="button"
                role="menuitem"
                disabled={!hasAdbDevice}
                title={hasAdbDevice ? "在当前设备上开一个 shell" : "未检测到在线设备"}
                onClick={() => pick(onAddAdb)}
              >
                <TerminalGlyph kind="adb" size={16} />
                <span>新建 ADB 会话</span>
              </button>
            </div>
          )}
        </div>
      </div>

      <section className="terminal-sidebar-section">
        <div className="terminal-sidebar-section-title">
          <span>已打开的终端</span>
          <span className="terminal-sidebar-count">{tabs.length}</span>
        </div>
        <div className="terminal-sidebar-scroll" role="list" aria-label="已打开的终端">
          {tabs.map((tab, index) => {
            const active = tab.id === activeTabId;
            return (
              // 拖拽/右键挂在行容器上：ListItem 的命中区是内部 button，
              // 把 HTML5 拖拽与 contextmenu 放在祖先节点，两者都能冒泡到这里。
              <div
                key={tab.id}
                className={"terminal-session-item" + (active ? " is-active" : "")}
                role="listitem"
                aria-label={`${tab.title}（${TYPE_LABEL[tab.type]}），可拖动到中间面板`}
                onPointerDown={(event) => beginCardDrag(event, { kind: "tab", id: tab.id, label: tab.title })}
                onContextMenu={(event) => {
                  // 没有回调时不要吞掉浏览器默认菜单（否则用户连「检查元素」都用不了）。
                  if (!onTabContextMenu) return;
                  event.preventDefault();
                  onTabContextMenu(event, tab.id);
                }}
              >
                <ListItem
                  dense
                  divider={index < tabs.length - 1}
                  selected={active}
                  interactive
                  ariaLabel={`${tab.title}（${TYPE_LABEL[tab.type]}）`}
                  leading={<TerminalGlyph kind={tabGlyph(tab)} />}
                  title={tab.title}
                  actions={
                    <span className="terminal-session-actions">
                      {/* 类型徽标紧挨关闭按钮左侧（用户要求：徽标放到 × 左边，整行只占一行）。 */}
                      {/* 类型徽标用组件库 Badge：状态点配色由变体提供（本机=中性、
                          ADB=绿、SSH=主色、WSL=橙），激活页签的圆点呼吸闪烁。 */}
                      <Badge
                        variant={tab.type === "adb" ? "success" : tab.type === "ssh" ? "primary" : tab.type === "wsl" ? "warning" : "neutral"}
                        pulse={active}
                        className="terminal-session-badge"
                      >
                        {TYPE_LABEL[tab.type]}
                      </Badge>
                      {tabs.length > 1 ? (
                        <Button
                          variant="bare"
                          size="xs"
                          icon={<X size={11} />}
                          aria-label={`关闭 ${tab.title}`}
                          title={`关闭 ${tab.title}`}
                          className="terminal-session-close"
                          onClick={(event) => {
                            event.stopPropagation();
                            onCloseTab(event, tab.id);
                          }}
                        />
                      ) : null}
                    </span>
                  }
                  onClick={() => onSelectTab(tab.id)}
                />
              </div>
            );
          })}
        </div>
      </section>

      <section className="terminal-sidebar-section terminal-sidebar-servers">
        <div className="terminal-sidebar-section-title">
          <span>已保存服务器</span>
          <button
            type="button"
            className="terminal-sidebar-mini-btn"
            aria-label="刷新已保存服务器"
            onClick={onRefreshServers}
          >
            <RefreshCw size={11} className={savedServersLoading ? "animate-spin" : undefined} />
          </button>
        </div>
        <div className="terminal-sidebar-scroll" role="list" aria-label="已保存服务器">
          {savedServers.length === 0 && (
            <div className="terminal-sidebar-empty">
              {savedServersLoading ? "正在读取…" : "暂无已保存的 SSH / WSL 配置"}
            </div>
          )}
          {savedServers.map((server, index) => (
            // 与「已打开终端」同样的做法：拖拽/右键挂在祖先容器上。
            // ListItem 的命中区是内部 button，HTML5 拖拽需要一个真实可拖的
            // DOM 元素，而 contextmenu 会从内部 button 冒泡到这里。
            <div
              key={`${server.kind}-${server.id}`}
              className="terminal-server-item"
              role="listitem"
              aria-label={`${server.name}（${server.kind.toUpperCase()}），可拖动到中间面板，右键打开菜单`}
              onPointerDown={(event) => beginCardDrag(event, { kind: "saved", id: server.id, serverKind: server.kind === "wsl" ? "wsl" : "ssh", label: server.name })}
              onContextMenu={(event) => {
                // 不传回调就保留浏览器默认菜单（侧栏不知道菜单长什么样）。
                if (!onServerContextMenu) return;
                event.preventDefault();
                onServerContextMenu(event, server);
              }}
            >
              <ListItem
                dense
                divider={index < savedServers.length - 1}
                ariaLabel={`在 ${server.name} 上打开终端`}
                leading={<TerminalGlyph kind={server.kind === "wsl" ? "wsl" : "ssh"} />}
                // 与「已打开终端」保持一致：只用一行，地址走 title 悬停提示（用户要求删掉第二行）。
                title={<span title={server.detail || server.name}>{server.name}</span>}
                meta={
                  <span className="terminal-server-meta">
                    {/* 列表里只有 SSH/WSL 两种，SSH 是主体：不再显示文字徽标，
                        只留 WSL 标识以示区分（用户要求）。 */}
                    {server.kind === "wsl" && <span className="terminal-server-kind">WSL</span>}
                  </span>
                }
                onClick={() => onUseSavedServer(server)}
              />
              {/* 连接状态点作为条目外壳的直接子元素：绝对定位锚到条目本体（ListItem
                  内部的 relative 层会截胡锚点，导致点悬在半路）。绿=已连接（呼吸）、
                  黄=连接中、红=失败、灰=未连接。 */}
              {server.kind === "ssh" && (() => {
                const dot = sshServerDotStatus(globalSsh.progressById[server.id]);
                const label = dot === "connected" ? "已连接" : dot === "connecting" ? "连接中" : dot === "failed" ? "连接失败" : "未连接";
                return (
                  <span
                    className="terminal-server-dot"
                    data-status={dot}
                    title={label}
                    aria-label={label}
                  />
                );
              })()}
            </div>
          ))}
        </div>
      </section>
    </aside>
  );
};
