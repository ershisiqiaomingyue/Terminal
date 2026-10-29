import { readToolWindowState } from "@/platform/toolWindowState";
import { useTerminalWindowState, type TerminalWindowSnapshot } from "@/platform/useTerminalWindowState";
import type { TerminalOutputEvent } from "@/platform/terminalHandoff";
import React, { useEffect, useMemo, useRef, useState } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { PaneHeightResizeHandle } from "@/components/ResizablePane/PaneHeightResizeHandle";
import { usePaneHeight } from "@/components/ResizablePane/usePaneHeight";
import { TERMINAL_PANE } from "@/components/ResizablePane/paneWidth";
import { ContextMenu } from "@/components/ui/ContextMenu";
import { SessionTabs } from "@/components/ui/SessionTabs";
import "@xterm/xterm/css/xterm.css";
import { ChevronDown, Minus, Monitor, Plus, Server, TerminalSquare, X } from "lucide-react";
import { formatInvokeError, invoke, isTauri, listen } from "@/platform/runtime";
import { notify } from "@/platform/notifications";
import { MAX_CHAT_TERMINALS } from "@/platform/scrollbackBudget";
import "./chat-terminal.css";
// 与终端页同一套面板组件与样式（迁移定稿）：对话页终端不再自建内核宿主，
// 主题 / 四角覆盖 / 首键焦点 / 尺寸自适应等修复从此全端一致。
import { TerminalPane, type TerminalPaneHandle } from "../Terminal/TerminalPane";
import type { TerminalTab } from "../Terminal/types";
import "../Terminal/terminal-view.css";
import "../Terminal/terminal-workspace.css";
import {
  appendTerminalText,
  bufferEarlyTerminalOutput,
  getTerminalTextBuffer,
  isImmediateTerminalInput,
  readTerminalText,
  takeTerminalText,
  type TerminalTextBuffer,
} from "@/platform/terminalBuffer";
import type { SshConnection, WslProfile } from "@/platform/api";

export type TerminalTarget = { kind: "local" | "ssh" | "wsl"; id?: string; label: string; root?: string; shell?: "pwsh" | "powershell" | "cmd" };
type Tab = { id: string; title: string; target: TerminalTarget; sessionId?: string };

const INPUT_CHARS_PER_BATCH = 32 * 1024;

function defaultTerminalTarget(target: TerminalTarget, workspacePath?: string): TerminalTarget {
  if (target.kind === "local") return { kind: "local", label: "PowerShell 7", shell: "pwsh", root: workspacePath || target.root };
  return { ...target, root: target.root || workspacePath };
}

/** 对话页的页签模型 → 终端页的 TerminalTab（TerminalPane 的契约）。 */
function toPaneTab(tab: Tab): TerminalTab {
  return {
    id: tab.id,
    title: tab.title,
    type: tab.target.kind,
    shell: tab.target.shell,
    profileId: tab.target.id,
    root: tab.target.root,
    sessionId: tab.sessionId,
  };
}

export function ChatTerminalPanel({ visible, target, workspacePath, sshConnections, wslProfiles, onHide, panelClassName, showHideControl = true, showTargetControl = false, windowStateKey = "chat-terminals", resizable = false, resizeBudget }: {
  windowStateKey?: string;
  visible: boolean;
  target: TerminalTarget;
  workspacePath?: string;
  sshConnections: SshConnection[];
  wslProfiles: WslProfile[];
  onHide: () => void;
  panelClassName?: string;
  showHideControl?: boolean;
  showTargetControl?: boolean;
  /** 允许拖动上沿改高度。默认关闭：AOSP 等复用方由 CSS 自行决定高度。 */
  resizable?: boolean;
  /** 可用高度来源容器（通常是对话工作区）；不传就用面板父节点。 */
  resizeBudget?: React.RefObject<HTMLElement | null>;
}) {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const desktop = isTauri();
  const restored = useRef(readToolWindowState<TerminalWindowSnapshot<Tab>>(windowStateKey)).current;
  // 面板自身高度就是拖动结果，所以量它外层（对话工作区）的可用高度。
  const parentBudgetRef = useRef<HTMLElement | null>(null);
  const addMenuRef = useRef<HTMLDivElement>(null);
  const sessionsRef = useRef<Record<string, string>>(Object.fromEntries((restored?.tabs || []).filter((tab) => tab.sessionId).map((tab) => [tab.id, tab.sessionId!])));
  const sessionTabsRef = useRef<Record<string, string>>(Object.fromEntries(Object.entries(sessionsRef.current).map(([tabId, sessionId]) => [sessionId, tabId])));
  const pendingOutputRef = useRef<Record<string, TerminalTextBuffer>>({});
  /**
   * 会话还没建好时敲下的键。
   *
   * 建 SSH 会话要建连、拉凭证，远端不可达时能等十几秒；这段时间里如果用户已经在
   * 敲字，原来会因为拿不到 sessionId 而被直接丢掉（表现就是「打不进字」）。
   * 先存起来，会话登记后立刻补发。
   */
  const pendingInputRef = useRef<Record<string, string>>({});
  const openingTabsRef = useRef(0);
  const disposedRef = useRef(false);
  // 换内核必须换实例（实例池按内核寄存），用纪元值驱动 TerminalPane 重建。
  const [engineEpoch, setEngineEpoch] = useState(0);
  const buffersRef = useRef<Record<string, TerminalTextBuffer>>({});
  const restoredBuffers = useRef(false);
  if (!restoredBuffers.current) {
    restoredBuffers.current = true;
    for (const [id, data] of Object.entries(restored?.buffers || {})) appendTerminalText(getTerminalTextBuffer(buffersRef.current, id), data, 240_000);
  }
  const inputQueueRef = useRef<Record<string, TerminalTextBuffer>>({});
  const inputWritesRef = useRef<Record<string, Promise<void>>>({});
  const inputFrameRef = useRef<number | null>(null);
  const activeIdRef = useRef("");
  /** 面板句柄表（TerminalPane 登记）：输出投递与右键菜单都走它，不直接碰内核。 */
  const paneHandlesRef = useRef<Record<string, TerminalPaneHandle>>({});
  const firstTabRequestedRef = useRef(false);
  const tabSequenceRef = useRef(0);
  const [tabs, setTabs] = useState<Tab[]>(restored?.tabs || []);
  const [activeId, setActiveId] = useState(restored?.activeId || "");
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [terminalMenu, setTerminalMenu] = useState<{ x: number; y: number; trigger: HTMLElement } | null>(null);
  const [outputReady, setOutputReady] = useState(false);
  const handoff = useTerminalWindowState(windowStateKey, tabs, activeId, buffersRef.current, () => openingTabsRef.current > 0, () => Object.values(sessionsRef.current));
  const budgetRef = resizeBudget || parentBudgetRef;
  const terminalHeight = usePaneHeight(budgetRef, resizable && visible, `android-tool.chat.terminal-height.${windowStateKey}`, TERMINAL_PANE);
  const active = tabs.find((item) => item.id === activeId);
  activeIdRef.current = activeId;
  const addTargets = useMemo<TerminalTarget[]>(() => [
    { kind: "local", label: "PowerShell 7", shell: "pwsh", root: workspacePath },
    { kind: "local", label: "Windows PowerShell", shell: "powershell", root: workspacePath },
    { kind: "local", label: "命令提示符", shell: "cmd", root: workspacePath },
    ...sshConnections.map((connection) => ({ kind: "ssh" as const, id: connection.id, label: `全局 SSH · ${connection.name}`, root: connection.defaultRoot || "~" })),
    ...wslProfiles.map((profile) => ({ kind: "wsl" as const, id: profile.id, label: `WSL · ${profile.name}`, root: profile.defaultRoot || workspacePath })),
  ], [sshConnections, workspacePath, wslProfiles]);

  const flushInputQueue = () => {
    for (const sessionId of Object.keys(inputQueueRef.current)) {
      const input = takeTerminalText(inputQueueRef.current[sessionId], INPUT_CHARS_PER_BATCH);
      if (!input) continue;
      const prior = inputWritesRef.current[sessionId] || Promise.resolve();
      const write = prior.catch(() => undefined).then(() => invoke<void>("write_terminal_input", { sessionId, input }));
      inputWritesRef.current[sessionId] = write.catch(() => undefined);
      void inputWritesRef.current[sessionId].finally(() => {
        if (inputQueueRef.current[sessionId]?.length) scheduleInputFlush();
      });
    }
  };

  const scheduleInputFlush = () => {
    if (inputFrameRef.current !== null) return;
    inputFrameRef.current = window.requestAnimationFrame(() => {
      inputFrameRef.current = null;
      flushInputQueue();
    });
  };

  const queueTerminalInput = (sessionId: string, input: string) => {
    appendTerminalText(getTerminalTextBuffer(inputQueueRef.current, sessionId), input);
    if (isImmediateTerminalInput(input)) {
      if (inputFrameRef.current !== null) {
        window.cancelAnimationFrame(inputFrameRef.current);
        inputFrameRef.current = null;
      }
      flushInputQueue();
      return;
    }
    scheduleInputFlush();
  };

  /** 会话输出 → 回滚缓冲 + 投给面板（面板自带帧预算泵）。 */
  const deliverOutput = (tabId: string, data: string) => {
    appendTerminalText(getTerminalTextBuffer(buffersRef.current, tabId), data, 240_000);
    paneHandlesRef.current[tabId]?.write(data);
  };

  /**
   * 已按会话发出的 PTY 尺寸（`${cols}x${rows}`）。面板实例的去重是实例级的：
   * 切换页签会按 key 重挂载面板，新实例首次 fit 必定上报一次尺寸——尺寸没变
   * 也发 resize 的话，Windows 远端（ConPTY/PSReadLine）会重画一行提示符，
   * 表现为「每切一次页签多一行 >」。按会话去重后同尺寸不再发。
   */
  const lastResizeRef = useRef<Record<string, string>>({});
  const resizeSession = (sessionId: string, cols: number, rows: number) => {
    const size = `${cols}x${rows}`;
    if (lastResizeRef.current[sessionId] === size) return;
    lastResizeRef.current[sessionId] = size;
    void invoke("resize_terminal_session", { sessionId, cols, rows }).catch(() => undefined);
  };

  const openTab = async (nextTarget: TerminalTarget) => {
    // 每个终端实例常驻一份 WASM 内存（只增不减），页签数必须设上限（开发方案 6.7.2）。
    if (tabs.length >= MAX_CHAT_TERMINALS) {
      notify(`最多同时打开 ${MAX_CHAT_TERMINALS} 个终端，请先关闭不用的页签。`, "error");
      return;
    }
    const id = `terminal-${Date.now()}-${tabSequenceRef.current++}`;
    const tab: Tab = { id, title: nextTarget.label, target: nextTarget };
    setTabs((previous) => [...previous, tab]);
    activeIdRef.current = id;
    setActiveId(id);
    if (!desktop) {
      const text = "\r\n交互式终端仅在桌面应用中运行。\r\n";
      appendTerminalText(getTerminalTextBuffer(buffersRef.current, id), text, 240_000);
      return;
    }
    const terminalTarget = nextTarget.kind === "local"
      ? { kind: "local", shell: nextTarget.shell || "pwsh", cwd: nextTarget.root || workspacePath || null }
      : nextTarget.kind === "ssh"
        ? { kind: "ssh", connectionId: nextTarget.id, cwd: nextTarget.root || null }
        : { kind: "wsl", profileId: nextTarget.id, cwd: nextTarget.root || null };
    openingTabsRef.current++;
    try {
      // PTY 先按默认尺寸开（面板此刻多半还没挂载完），登记会话后再按面板真实
      // 尺寸补一次——尺寸确实变了才补发，避免无谓的 SIGWINCH 让 shell 重画提示符。
      const info = await invoke<{ sessionId: string }>("open_terminal_session", { target: terminalTarget, cols: 120, rows: 28 });
      if (disposedRef.current) {
        await invoke("close_terminal_session", { sessionId: info.sessionId }).catch(() => undefined);
        return;
      }
      sessionsRef.current[id] = info.sessionId;
      sessionTabsRef.current[info.sessionId] = id;
      lastResizeRef.current[info.sessionId] = "120x28";
      void (async () => {
        const deadline = performance.now() + 400;
        for (;;) {
          const settled = paneHandlesRef.current[id]?.fit();
          if (settled && settled.cols > 0 && settled.rows > 0) {
            resizeSession(info.sessionId, settled.cols, settled.rows);
            return;
          }
          if (performance.now() >= deadline) return;
          await new Promise((resolve) => window.setTimeout(resolve, 25));
        }
      })();
      const pendingOutput = readTerminalText(pendingOutputRef.current[info.sessionId]);
      delete pendingOutputRef.current[info.sessionId];
      if (pendingOutput) deliverOutput(id, pendingOutput);
      const queuedInput = pendingInputRef.current[id];
      delete pendingInputRef.current[id];
      if (queuedInput) queueTerminalInput(info.sessionId, queuedInput);
      setTabs((previous) => previous.map((item) => item.id === id ? { ...item, sessionId: info.sessionId } : item));
    } catch (error) {
      delete pendingInputRef.current[id];
      const text = `\r\n${formatInvokeError(error)}\r\n`;
      appendTerminalText(getTerminalTextBuffer(buffersRef.current, id), text, 240_000);
      paneHandlesRef.current[id]?.write(text);
    } finally {
      openingTabsRef.current--;
      if (!openingTabsRef.current) { pendingOutputRef.current = {}; handoff.retainSessions(Object.values(sessionsRef.current)); }
    }
  };

  useEffect(() => {
    if (!addMenuOpen) return;
    const closeOnOutsidePointerDown = (event: PointerEvent) => {
      if (!addMenuRef.current?.contains(event.target as Node)) setAddMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsidePointerDown);
    return () => document.removeEventListener("pointerdown", closeOnOutsidePointerDown);
  }, [addMenuOpen]);

  useEffect(() => {
    if (!visible || (desktop && !outputReady) || tabs.length || firstTabRequestedRef.current) return;
    firstTabRequestedRef.current = true;
    void openTab(defaultTerminalTarget(target, workspacePath));
  }, [desktop, outputReady, target, tabs.length, visible, workspacePath]);

  useEffect(() => {
    if (!desktop) return;
    let stopped = false;
    let remove: () => void = () => undefined;
    const deliver = (event: TerminalOutputEvent) => {
      const tabId = sessionTabsRef.current[event.sessionId];
      if (!event.data) return;
      if (!tabId) {
        if (openingTabsRef.current) bufferEarlyTerminalOutput(pendingOutputRef.current, event.sessionId, event.data);
        return;
      }
      deliverOutput(tabId, event.data);
    };
    void listen<TerminalOutputEvent>("terminal:output", (event) => handoff.accept(event, deliver)).then((unlisten) => {
      if (stopped) unlisten();
      else {
        remove = unlisten;
        void handoff.recover(deliver).catch((error) => console.error("终端输出恢复失败", error)).finally(() => { if (!stopped) setOutputReady(true); });
      }
    });
    return () => { stopped = true; remove(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop]);

  const closeTimerRef = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    disposedRef.current = false;
    clearTimeout(closeTimerRef.current);
    return () => { closeTimerRef.current = setTimeout(() => {
    disposedRef.current = true;
    if (inputFrameRef.current !== null) window.cancelAnimationFrame(inputFrameRef.current);
    Object.values(sessionsRef.current).forEach((sessionId) => void invoke("close_terminal_session", { sessionId }));
    }, 0); };
  }, []);

  const closeTab = async (id: string) => {
    const sessionId = sessionsRef.current[id];
    if (sessionId) await invoke("close_terminal_session", { sessionId });
    delete sessionsRef.current[id];
    if (sessionId) delete lastResizeRef.current[sessionId];
    handoff.retainSessions(Object.values(sessionsRef.current));
    if (sessionId) {
      delete sessionTabsRef.current[sessionId];
      delete inputQueueRef.current[sessionId];
      delete inputWritesRef.current[sessionId];
    }
    delete buffersRef.current[id];
    delete paneHandlesRef.current[id];
    setTabs((previous) => {
      const index = previous.findIndex((item) => item.id === id);
      const remaining = previous.filter((item) => item.id !== id);
      if (activeIdRef.current === id) {
        const nextId = remaining[Math.max(0, index - 1)]?.id || "";
        activeIdRef.current = nextId;
        setActiveId(nextId);
      }
      return remaining;
    });
  };

  const disposeAll = async () => {
    for (const tab of tabs) await closeTab(tab.id);
    sessionTabsRef.current = {};
    buffersRef.current = {};
    inputQueueRef.current = {};
    inputWritesRef.current = {};
    firstTabRequestedRef.current = false;
    activeIdRef.current = "";
    setTabs([]);
    setActiveId("");
    setAddMenuOpen(false);
    onHide();
  };

  const hideTerminal = () => {
    setAddMenuOpen(false);
    onHide();
  };

  const activeHandle = () => paneHandlesRef.current[activeIdRef.current];
  const copyTerminalSelection = async () => {
    const selection = activeHandle()?.getSelection();
    if (!selection) return;
    try { await navigator.clipboard.writeText(selection); }
    catch (error) { notify(`复制失败：${formatInvokeError(error)}`, "error"); }
  };
  const pasteTerminalClipboard = async () => {
    try { activeHandle()?.paste(await navigator.clipboard.readText()); }
    catch (error) { notify(`粘贴失败：${formatInvokeError(error)}`, "error"); }
  };

  return <>
    {resizable && visible && <PaneHeightResizeHandle label="调整终端高度（双击恢复默认）" controls={`${windowStateKey}-panel`} height={terminalHeight.height} min={terminalHeight.bounds.min} max={terminalHeight.bounds.max} initial={TERMINAL_PANE.initial} onResize={terminalHeight.resize} onCommit={terminalHeight.commit} />}
  <section ref={(node: HTMLElement | null) => { parentBudgetRef.current = node?.parentElement || null; }} className={`chat-terminal-panel${visible ? "" : " is-hidden"}${panelClassName ? ` ${panelClassName}` : ""}`} aria-hidden={!visible} style={resizable ? { "--chat-terminal-height": `${terminalHeight.height}px` } as React.CSSProperties : undefined}>
    {confirmationDialog}
    {terminalMenu && <ContextMenu x={terminalMenu.x} y={terminalMenu.y} label="终端操作" trigger={terminalMenu.trigger} onClose={() => setTerminalMenu(null)} items={[
      { label: "复制选中内容", disabled: !activeHandle()?.hasSelection(), onSelect: () => void copyTerminalSelection() },
      { label: "粘贴到终端", onSelect: () => void pasteTerminalClipboard() },
      { label: "全选", onSelect: () => activeHandle()?.selectAll() },
      { label: "清屏", onSelect: () => activeHandle()?.clear() },
    ]} />}
    <header>
      {showTargetControl && <button type="button" title={`在当前环境新建终端：${target.label} · ${workspacePath || target.root || "~"}`} aria-label="在当前环境新建终端" onClick={() => void openTab(defaultTerminalTarget(target, workspacePath))}><TerminalSquare size={15} /></button>}
      <SessionTabs className="chat-terminal-tabs" label="终端会话" tabs={tabs.map((tab) => ({ id: tab.id, label: tab.title, closeLabel: `关闭 ${tab.title}` }))} activeId={activeId} onSelect={(id) => { activeIdRef.current = id; setActiveId(id); }} onClose={(id) => { const tab = tabs.find((item) => item.id === id); if (tab) confirmAction({ title: "结束终端", message: `确认关闭「${tab.title}」？运行中的进程会被中断，输出记录将清除。`, confirmLabel: "确认结束", onConfirm: () => closeTab(id) }); }} />
      <div ref={addMenuRef} className="chat-terminal-add-anchor">
        <button type="button" title="新建终端" aria-label="新建终端" aria-expanded={addMenuOpen} onClick={() => setAddMenuOpen((value) => !value)}><Plus size={15} /><ChevronDown size={12} /></button>
        {addMenuOpen && <div className="chat-terminal-add-menu" role="menu">
          <span>本机终端</span>
          {addTargets.filter((item) => item.kind === "local").map((item) => <button key={`${item.kind}-${item.shell}`} type="button" role="menuitem" onClick={() => { setAddMenuOpen(false); void openTab(item); }}><TerminalSquare size={14} />{item.label}</button>)}
          {addTargets.some((item) => item.kind === "ssh") && <><span>全局 SSH</span>{addTargets.filter((item) => item.kind === "ssh").map((item) => <button key={`${item.kind}-${item.id}`} type="button" role="menuitem" onClick={() => { setAddMenuOpen(false); void openTab(item); }}><Server size={14} />{item.label}</button>)}</>}
          {addTargets.some((item) => item.kind === "wsl") && <><span>WSL</span>{addTargets.filter((item) => item.kind === "wsl").map((item) => <button key={`${item.kind}-${item.id}`} type="button" role="menuitem" onClick={() => { setAddMenuOpen(false); void openTab(item); }}><Monitor size={14} />{item.label}</button>)}</>}
        </div>}
      </div>
      {showHideControl && <button type="button" title="隐藏终端，保留会话和历史" aria-label="隐藏终端，保留会话和历史" onClick={hideTerminal}><Minus size={15} /></button>}
      <button type="button" title="关闭并结束全部终端" aria-label="关闭并结束全部终端" onClick={() => confirmAction({ title: "结束全部终端", message: "确认关闭全部终端会话？运行中的进程会被中断，终端输出记录将清除。", confirmLabel: "确认结束", onConfirm: disposeAll })}><X size={15} /></button>
    </header>
    <div className="chat-terminal-host">
      <TerminalPane
        key={`chat-${active?.id ?? "empty"}`}
        tab={active ? toPaneTab(active) : null}
        focused={visible}
        paneTotal={tabs.length || 1}
        engineEpoch={engineEpoch}
        getReplayText={(tabId) => readTerminalText(buffersRef.current[tabId])}
        getSessionId={(tabId) => sessionsRef.current[tabId]}
        onInput={(tabId, data) => {
          const sessionId = sessionsRef.current[tabId];
          if (sessionId) queueTerminalInput(sessionId, data);
          else {
            // 会话还在建立：先攒着，登记后补发（远端卡住时也不会吞键）。
            pendingInputRef.current[tabId] = `${pendingInputRef.current[tabId] || ""}${data}`.slice(-8192);
          }
        }}
        onResize={(tabId, cols, rows) => {
          const sessionId = sessionsRef.current[tabId];
          if (sessionId) resizeSession(sessionId, cols, rows);
        }}
        onFocus={(tabId) => {
          activeIdRef.current = tabId;
          setActiveId(tabId);
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          setTerminalMenu({ x: event.clientX, y: event.clientY, trigger: event.currentTarget as HTMLElement });
        }}
        onEngineSwitchRequest={() => setEngineEpoch((value) => value + 1)}
        onEngine={() => undefined}
        onRegister={(tabId, handle) => {
          if (handle) paneHandlesRef.current[tabId] = handle;
          else delete paneHandlesRef.current[tabId];
        }}
        onDetach={(tabId) => {
          const tab = tabs.find((item) => item.id === tabId);
          if (tab) confirmAction({ title: "结束终端", message: `确认关闭「${tab.title}」？运行中的进程会被中断，输出记录将清除。`, confirmLabel: "确认结束", onConfirm: () => void closeTab(tab.id) });
        }}
        slot={0}
        chromeless
        emptyContent={
          <button type="button" className="chat-terminal-empty" onClick={() => { void openTab(defaultTerminalTarget(target, workspacePath)); }}>
            <TerminalSquare size={16} />新建终端
          </button>
        }
      />
    </div>
  </section>
  </>;
}
