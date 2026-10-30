import { readToolWindowState, restoreToolWindowState, useToolWindowState } from "@/platform/toolWindowState";
import { saveTerminalPageState, takeTerminalPageState } from "./pageCache";
import {
  useTerminalWindowState,
  type TerminalWindowSnapshot,
} from "@/platform/useTerminalWindowState";
import type { TerminalOutputEvent } from "@/platform/terminalHandoff";
import { isTerminalQueryResponse } from "@/platform/terminalReplay";
import React, { useState, useRef, useEffect, useMemo, useCallback } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { ContextMenu } from "@/components/ui/ContextMenu";
import "@xterm/xterm/css/xterm.css";
import {
  AlertTriangle,
  X,
  Terminal as TerminalIcon,
  Square,
  Columns2,
  Grid2x2,
  Rows2,
  AlertCircle,
  Info,
  PanelLeft,
} from "lucide-react";
import { Button } from "@/components/ui";
import type { TerminalEngine } from "@/platform/terminalAdapter";
import { MAX_TERMINALS } from "@/platform/scrollbackBudget";
import { BlockTracker } from "@/platform/terminal/blocks/blockTracker";
import {
  isTerminalStatus,
  isFailableStatus,
  type CommandBlock,
} from "@/platform/terminal/blocks/types";
import { isBlockAiEligible } from "@/platform/terminal/ai/terminalAiService";
import type { TerminalAiMode } from "@/platform/terminal/ai/prompts";
import { BlockAiPanel } from "./BlockAiPanel";
import { CommandComposer } from "./CommandComposer";
import { TerminalCommandBar } from "./TerminalCommandBar";
import { RepairCard } from "./RepairCard";
import {
  canStartRepair,
  initialRepairState,
  isTerminalPhase,
  repairConfirmToken,
  repairExecutable,
  repairReducer,
  type RepairState,
} from "./repairSession";
import { proposeRepair } from "@/platform/terminal/ai/repairService";
import { terminalAiDeps } from "@/platform/terminal/ai/terminalAiDeps";
import { composerBlockedReason } from "@/platform/terminal/ai/commandComposer";
import { readTerminalSettings, subscribeTerminalSettings } from "@/platform/terminalSettings";
import {
  formatInvokeError,
  invoke,
  isTauri,
  listen,
} from "@/platform/runtime";
import {
  readSshWorkingDirectory,
  listSavedServers,
  readWorkspaceGit,
  type SavedServerEntry,
} from "@/platform/terminalWorkspace";
import { readHostStatus } from "@/platform/hostStatus";
import {
  countActiveSshTerminals, listSshConnections } from "@/platform/api";
import { awaitSshConnected, connectGlobalSsh, reportTerminalSshSession } from "@/platform/globalSsh";
import { TerminalSidebar } from "./TerminalSidebar";
import { EmptyPanePicker } from "./EmptyPanePicker";
import { NewSshDialog, type EditSshTarget } from "./NewSshDialog";
import { TerminalDrawer, TerminalDrawerRail } from "./TerminalDrawer";
import { TerminalPane, readPaneReplayText, type TerminalPaneHandle } from "./TerminalPane";
import { PaneSplitter } from "./PaneSplitter";
import { normalizeTerminalPwd } from "./terminalPwd";
import { PaneResizeHandle } from "@/components/ResizablePane/PaneResizeHandle";
import { usePaneWidth } from "@/components/ResizablePane/usePaneWidth";
import { TERMINAL_DRAWER_PANE } from "@/components/ResizablePane/paneWidth";
import {
  DEFAULT_EDGE_RATIO,
  canAcceptDrop,
  paneCoverSide,
  planDropOnPane,
  type ResizePlan,
  emptyBindings,
} from "./paneLayout";
import {
  promoteTabToFirstSlot,
  shouldReconnectOnInput,
  SSH_RETRY_INTERVAL_MS,
  SSH_RETRY_MAX_ATTEMPTS,
  type SshRetryState,
} from "./paneRearrange";
import { PaneBanner, type PaneNotice, type PaneNoticeKind } from "./PaneBanner";
import {
  compactBindings,
  normalizeBindings,
  paneCount,
  readTerminalLayout,
  upgradeLayoutFor,
  visibleLayout,
  dropsEmptySlotIntent,
  type DrawerPane,
  type PaneBindings,
  type TerminalLayout,
  type TerminalTab,
} from "./types";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { TERMINAL_MENU_EVENT, type TerminalMenuDetail } from "@/platform/terminalMenu";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import {
  appendTerminalText,
  bufferEarlyTerminalOutput,
  clearTerminalTextBuffer,
  getTerminalTextBuffer,
  isImmediateTerminalInput,
  readTerminalText,
  takeTerminalText,
  type TerminalTextBuffer,
} from "@/platform/terminalBuffer";
import "./terminal-view.css";
import "./terminal-workspace.css";
interface TerminalViewProps {
  onBack?: () => void;
}
/** 页面级 UI 状态快照：窗口重开/切页回来后恢复布局。 */
interface ShellUiSnapshot {
  layout: TerminalLayout;
  /** 兼容旧快照：早期版本只有单面板的输入框状态。 */
  paneBindings: PaneBindings;
  inputVal?: string;
  cmdHistory?: string[];
  /** 切页回来还要原样恢复：焦点面板、分栏比例（覆盖态极值 0/1 也在内）。 */
  focusedPane?: number;
  colRatios?: number[];
  rowRatios?: number[];
  /** 旧快照字段：`showEmptySlots` 已改成一次性意图、不再持久化，仅为了能读旧快照。 */
  showEmptySlots?: boolean;
  /** 左栏（终端列表）是否收起，与对话页的导航折叠同一套交互（m18576）。 */
  sidebarCollapsed?: boolean;
}
const INPUT_CHARS_PER_BATCH = 32 * 1024;
/** 比例数组恢复：夹到 0..1（覆盖态极值 0/1 合法），长度按当前布局补齐。 */
const sanitizeRatioList = (value: unknown, length: number, fallback: number[]): number[] => {
  if (!Array.isArray(value) || value.length === 0) return fallback;
  return Array.from({ length }, (_, index) => {
    const raw = value[index] ?? fallback[index];
    return Number.isFinite(raw) ? Math.min(1, Math.max(0, raw)) : fallback[index];
  });
};
export const TerminalView: React.FC<TerminalViewProps> = ({ onBack }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const desktop = isTauri();
  // 切页缓存优先于悬浮窗快照：同一窗口内去别的工具页再回来，工作区原样恢复
  //（含离开期间的输出 journal 补投——与悬浮窗拆离同一条恢复链路）。
  const pageState = useRef(takeTerminalPageState()).current;
  if (pageState) restoreToolWindowState(pageState);
  const restored = useRef(
    readToolWindowState<TerminalWindowSnapshot<TerminalTab>>("shell"),
  ).current;
  const restoredUi = useRef(readToolWindowState<ShellUiSnapshot>("shell-ui")).current;
  const sessionsRef = useRef<Record<string, string>>(
    Object.fromEntries(
      (restored?.tabs || []).filter((tab) => tab.sessionId).map((tab) => [tab.id, tab.sessionId!]),
    ),
  );
  const openingPtyRef = useRef(0);
  const disposedRef = useRef(false);
  const sessionTabsRef = useRef<Record<string, string>>(
    Object.fromEntries(
      Object.entries(sessionsRef.current).map(([tabId, sessionId]) => [sessionId, tabId]),
    ),
  );
  /**
   * 页签 → SSH 连接 id（需求②）。
   * 终端里的 SSH 是 PTY 直起 `ssh.exe` 子进程，后端 `SshRuntimeState` 里没有这条
   * 连接，右上角全局状态永远停在 disconnected。这里记录哪些页签是 SSH 终端，
   * 在开/关时通过 `reportTerminalSshSession` 通知全局状态，**不新增轮询**。
   */
  const sshTabRef = useRef<Record<string, string>>({});
  const pendingOutputRef = useRef<Record<string, TerminalTextBuffer>>({});
  const outputBuffers = useRef<Record<string, TerminalTextBuffer>>({});
  const restoredBuffers = useRef(false);
  if (!restoredBuffers.current) {
    restoredBuffers.current = true;
    for (const [id, data] of Object.entries(restored?.buffers || {}))
      appendTerminalText(getTerminalTextBuffer(outputBuffers.current, id), data, 240000);
  }
  const inputQueueRef = useRef<Record<string, TerminalTextBuffer>>({});
  const inputWritesRef = useRef<Record<string, Promise<void>>>({});
  const inputFrameRef = useRef<number | null>(
    null,
  );
  /**
   * 面板句柄表：key 是页签 id。每个 `TerminalPane` 各自持有一份内核实例，
   * 父级不再直接碰内核，只按页签 id 把输出投给对应面板（设计文档 §3.5）。
   */
  const paneHandlesRef = useRef<Record<string, TerminalPaneHandle>>(
    {},
  );
  /**
   * 当前活跃页签。
   * 默认是空串：用户明确要求「进入界面默认是空闲面板」。空串表示没有任何页签
   * 活跃，中栏四个槽位全是空闲面板。
   */
  const activeTabIdRef = useRef(restored?.activeId || "");
  const [terminalEngine, setTerminalEngine] = useState<TerminalEngine | null>(null);
  // 换内核必须换实例（池按内核寄存），所以用纪元值触发构造 effect 重建。
  const [engineEpoch, setEngineEpoch] =
    useState(
      0,
    );
  /**
   * 命令块层（开发方案第八章）：切成"一次命令执行"的结构化记录。
   * 放 ref 而不是 state——它跑在输出热路径上，重建或触发渲染都会拖慢终端。
   */
  const blockTrackerRef = useRef<BlockTracker | null>(null);
  if (!blockTrackerRef.current) blockTrackerRef.current = new BlockTracker();
  const blockTracker =
    blockTrackerRef.current;
  /**
   * 命令块 AI 面板（开发方案 9.2.4 / T3-4）。
   * 终端输出是内核 canvas，块卡片插不到行间，所以做成底部停靠面板：
   * 同一时刻只显示一个块，`blockAiEpoch` 在每次打开时自增，保证在同一个块上
   * 再点一次"诊断"也能重新发起请求（改了 mode 但 block.id 没变）。
   */
  const [blockAi, setBlockAi] = useState<{
    blockId: string;
    mode: TerminalAiMode;
    epoch: number;
  } | null>(null);
  const blockAiEpochRef =
    useRef(
      0,
    );
  /**
   * 输入侧 AI 面板（开发方案 9.3 / T4-1）：记的是"哪个页签开着面板"。
   * 与 `BlockAiPanel` 互斥——两个面板都停靠在终端下方，同时开会把画布挤没。
   * 页面关闭、切页签、直通都要收起，否则面板会停在一个已经不存在的会话上。
   */
  const [composerTabId, setComposerTabId] = useState<string | null>(
    null,
  );
  /**
   * 从底部指令栏带进输入侧 AI 的自然语言。
   * 单独存一份而不是塞进 `composerTabId` 里：值可能是空串以外任意文本，
   * 和"哪个页签开着面板"是两件事；面板消费后由自身 `initialNl` 只读一次。
   */
  const [composerNl, setComposerNl] = useState<string | null>(null);
  const composerTabRef = useRef<string | null>(null);
  composerTabRef.current =
    composerTabId;
  /** 直通模式（8.5 / F54）：全屏程序里按键必须直进 PTY，输入侧 AI 要禁用。 */
  const [passthroughTabs, setPassthroughTabs] = useState<Record<string, boolean>>(
    {},
  );
  /**
   * 自动修复闭环（开发方案 9.4 / T4-7）。
   * 卡片同样停靠在终端下方，与 `BlockAiPanel` / `CommandComposer` 三者互斥。
   * `repairWaitingRef` 记"已经写进 PTY、还在等结果"的那条命令，轮询时靠它比对
   * 新的 block，避免把别的命令的成功算成这次修复的成功。
   */
  const [repair, setRepair] = useState<RepairState | null>(null);
  const [repairBlock, setRepairBlock] = useState<CommandBlock | null>(null);
  const [repairBusy, setRepairBusy] = useState(false);
  const repairAbortRef = useRef<AbortController | null>(null);
  const repairWaitingRef = useRef<string | null>(null);
  const repairTimerRef = useRef<number | null>(
    null,
  );
  /**
   * 最近一个可解释的块（F41：不自动刷屏）。
   * 面板**不会**自动弹出——只在顶栏亮出一个入口。`BlockTracker` 的 `onChange`
   * 回调在构造时就固定，拿不到 props，所以这里由输出流驱动一个惰性刷新：
   * 只在会话登记表里已有块、且"最新块"与上次比对不一致时才 setState，
   * 命令执行期间不会每个 chunk 都触发一次渲染。
   */
  const [latestBlockTick, setLatestBlockTick] = useState(0);
  const latestBlockKeyRef = useRef("");
  const refreshLatestBlock = useCallback(() => {
    const sessionId = sessionsRef.current[activeTabIdRef.current];
    if (!sessionId) return;
    const latest = blockTracker.latest(sessionId);
    const key = latest ? `${latest.id}:${latest.status}` : "";
    if (key === latestBlockKeyRef.current) return;
    latestBlockKeyRef.current = key;
    setLatestBlockTick((value) => value + 1);
  }, [
    blockTracker,
  ]);
  /**
   * 只取 AI 相关的那一个设置位（F46 全局开关）。
   * 整份 `TerminalSettings` 订阅过来会让字号/主题这些改动也触发本视图重渲染，
   * 而终端视图重渲染要重建画布引用，代价远大于收益。所以这里只投影一个 boolean，
   * 值没变时 `setState` 直接返回同一个值，React 会跳过重渲染。
   */
  const [aiFailureHint, setAiFailureHint] = useState(
    () => readTerminalSettings().aiFailureHint,
  );
  /**
   * 渲染后端（设计 4.4「内核 + 渲染后端」胶囊）：只投影 renderer 一个字段。
   * 与 aiFailureHint 共用同一份订阅，避免两次 subscribe 各自触发一次 setState。
   */
  const [terminalRenderer, setTerminalRenderer] = useState(() => readTerminalSettings().renderer);
  useEffect(
    () =>
      subscribeTerminalSettings((settings) => {
        setAiFailureHint(settings.aiFailureHint);
        setTerminalRenderer(settings.renderer);
      }),
    [],
  );
  // 多终端标签会话管理 (满足多终端需求)
  /**
 * 页签列表。
 * 首次进入不预置任何页签：用户要求进终端默认是一屏空闲面板，点空闲面板
 * 才通过悬浮选择器（或左栏）开终端。恢复会话时照旧沿用上次的页签。
 */ const [
    tabs,
    setTabs,
  ] = useState<TerminalTab[]>(restored?.tabs || []);
  const [activeTabId, setActiveTabId] = useState<string>(restored?.activeId || "");
  /** 页签表的镜像：定时器 / 事件回调里要读最新值，不能依赖渲染闭包。 */
  const tabsRef = useRef<TerminalTab[]>(tabs);
  tabsRef.current = tabs;
  activeTabIdRef.current =
    activeTabId;
  /**
   * 中栏布局与面板绑定（设计文档 §3.2 / §3.3）。
   * `paneBindings[i]` 是第 i 块面板绑定的页签 id，null 表示空面板。
   * 布局从大到小时多余的绑定被裁掉；从小变大时补 null，不自动填满。
   */
  // 首次进入 / 应用重启仍从 1x1 空面板开始（用户定稿）；但**切页回来**要按快照
  // 原样恢复布局与绑定（含覆盖态比例），缓存只在窗口内存里、重启自然清空。
  const [layout, setLayout] = useState<TerminalLayout>(restoredUi?.layout ?? "1x1");
  /** 显式点布局按钮选择的布局：空槽渲染占位板（可拖入/新建），§22 十八轮。
   * 自动路径（抢占收敛/页签关闭前移等）不改此标记，维持 §20.1 的铺满行为。
   *
   * **不写进快照、也不从快照恢复**：`layout` + `paneBindings` 恢复后如果还带着很久
   * 以前那次的「开空槽」意图，重启/切页回来就会看到 2 个终端铺在 4 格里（m18580）。
   * 这是一次性意图，重开时按实际占用收敛，想再摆空槽就再点一次布局按钮。 */
  const [showEmptySlots, setShowEmptySlots] = useState(false);
  /** 左栏折叠：只影响终端页自己的侧栏，不动 App 里聊天/组件页共用的那套状态。 */
  const [sidebarCollapsed, setSidebarCollapsed] = useState(Boolean(restoredUi?.sidebarCollapsed));
  const [paneBindings, setPaneBindings] = useState<PaneBindings>(() =>
    restoredUi?.paneBindings
      ? normalizeBindings(restoredUi.paneBindings, restoredUi?.layout ?? "1x1")
      : emptyBindings("1x1"),
  );
  /** 焦点面板下标：键盘输入与设备条/常用命令条都挂在它上面。 */
  const [focusedPane, setFocusedPane] = useState(() => {
    if (typeof restoredUi?.focusedPane === "number" && restoredUi.focusedPane >= 0) {
      return Math.min(restoredUi.focusedPane, paneCount(restoredUi.layout ?? "1x1") - 1);
    }
    const restoredIndex = (restoredUi?.paneBindings ?? []).indexOf(restored?.activeId || "");
    // 没有恢复的活跃页签时焦点落在第 0 块（空闲面板），不要把 -1 传下去。
    return restoredIndex >= 0 ? restoredIndex : 0;
  });
  /**
   * 拖边线缩放的分栏比例（需求 ⑥）。
   * 只记两个数，不记像素：左右比例与上下比例都是 0..1，`0.5` 是均分。
   * *拖动过程中这两个值不变**——`PaneSplitter` 只往网格元素写 CSS 变量，
   * 松手才回到这里。终端是内容最重的视图，每帧 setState 会让四个面板连同
   * 内核 canvas 一起重渲染，那是必须避免的开销。
   * 切页回来按快照恢复（覆盖态极值 0/1 也原样保留，被盖住的一侧继续隐藏）。
   */
  const [colRatios, setColRatios] = useState<number[]>(() => sanitizeRatioList(restoredUi?.colRatios, 2, [0.5, 0.5]));
  const [rowRatios, setRowRatios] = useState<number[]>(() => sanitizeRatioList(restoredUi?.rowRatios, 1, [0.5]));
  /**
   * 覆盖态记忆：某行/整网格被压到比例极值（0/1）时，记下**覆盖前的边线位置**，
   * 即被隐藏一侧的拖前原宽。回拖必须超过这条原宽的一半才恢复（用户定稿），
   * 否则一碰到边线就弹回来，覆盖态根本维持不住。恢复 / 换布局时清掉。
   */
  const collapseOriginRef = useRef<{ col: (number | null)[]; row: number | null }>({
    col: [null, null],
    row: null,
  });
  /** 网格元素引用：拖动中的预览比例直接写在这个节点的 style 上。 */
  const paneGridRef = useRef<HTMLDivElement | null>(
    null
  );
  /** 拖拽预览的 rAF 句柄（快速拖拽时每帧最多写一次 DOM）。 */
  const previewFrameRef = useRef<number | null>(null);
  /** 槽位正被覆盖（比例极值）隐藏时恢复对半显示。只看**这个槽位自己**是否
      被压扁——2v 里往可见行放页签不能顺手把另一行的覆盖态解开。 */
  const revealPaneRatio = (slot: number) => {
    const perRow = layout === "1x1" || layout === "2v" ? 1 : 2;
    const rowIndex = Math.floor(slot / perRow);
    const colIndex = slot % perRow;
    if (layout === "2v" || layout === "2x2") {
      const rowHidden =
        (rowIndex === 0 && rowRatios[0] <= 0.01) || (rowIndex === 1 && rowRatios[0] >= 0.99);
      if (rowHidden) {
        setRowRatios([0.5]);
        collapseOriginRef.current.row = null;
      }
    }
    if (layout === "1x2" || layout === "2x2") {
      const value = colRatios[rowIndex] ?? 0.5;
      const colHidden = (colIndex === 0 && value <= 0.01) || (colIndex === 1 && value >= 0.99);
      if (colHidden) {
        setColRatios((prev) => prev.map((v, index) => (index === rowIndex ? 0.5 : v)));
        collapseOriginRef.current.col[rowIndex] = null;
      }
    }
  };
  /**
   * 把页签放到面板上（设计文档 §3.3）。
   * **关键修正**：原实现"全占满就顶掉焦点面板"会在 1x1 布局下把上一个页签
   * 从唯一槽位挤出去——那个面板随即卸载、`lease.release()` 归还实例并清空容器，
   * 用户看到的就是"切到第 2 个，第 1 个数据全没了"。
   * 现在的规则：
   * 1. 已绑在某个面板上 → 只把焦点移过去；
   * 2. 有空面板 → 放到第一个空面板；
   * 3. 全占满但还有更多页签要显示 → **自动升到能容纳的布局**（1x1→1x2→2x2），
   * 绝不顶掉已有面板；只有布局已经到 2x2 且四块都占满时才替换焦点面板。
   *
   * 覆盖态定稿（§22 二十六轮）后这里必须是**直线逻辑**（不走 setState updater）：
   * 被覆盖槽位的绑定/空槽都还在，"找空槽 / 替换槽 0"都可能落进**被压到 0 宽的
   * 隐藏格子**——终端正常建连正常输出，但整个渲染在看不见的格子里，用户看到
   * 的就是"新开的终端什么也没有"。因此每条落位路径都要对目标槽位调
   * `revealPaneRatio`；顺带消灭 updater 里嵌套 setState（StrictMode 双调用）。
   */
  const bindTabToPane = (tabId: string, preferSlot?: number, extraCount = 0) => {
    const prev = paneBindings;
    // 已绑在某个面板上 → 只把焦点移过去（若那格正被覆盖隐藏，先亮出来）。
    const existing = prev.indexOf(tabId);
    if (existing >= 0) {
      revealPaneRatio(existing);
      setFocusedPane(existing);
      return;
    }
    let target = -1;
    let next: PaneBindings = prev;
    let nextLayout: TerminalLayout | null = null;
    if (preferSlot !== undefined && preferSlot >= 0 && preferSlot < prev.length) {
      // 显式指定了槽位（拖拽落点 / 从服务器拖进来）：就放这个槽位。
      // 被挤出来的页签不销毁，页签仍在左栏；能塞进别的空槽就顺手塞进去，
      // 这样"拖到第 2 块"不会把原来第 2 块的终端变成只能从侧栏点回来。
      next = [...prev];
      const displaced = next[preferSlot];
      next[preferSlot] = tabId;
      if (displaced) {
        const spare = next.indexOf(null);
        if (spare >= 0) next[spare] = displaced;
      }
      target = preferSlot;
    } else {
      const empty = prev.indexOf(null);
      if (empty >= 0) {
        next = [...prev];
        next[empty] = tabId;
        target = empty;
      } else {
        // 槽位全占满：先尝试升布局，给新页签腾一个空槽。
        // `tabs` 是本次渲染的闭包值，新建页签时它还没包含新页签——差这一步
        // 会导致 1x1 开第 2 个终端时不升级、直接顶掉焦点面板（终端1消失的根因）。
        const upgraded = upgradeLayoutFor(layout, tabs.length + (extraCount ?? 0));
        if (upgraded !== layout) {
          const grown = normalizeBindings(prev, upgraded);
          const slot = grown.indexOf(null);
          if (slot >= 0) {
            grown[slot] = tabId;
            next = grown;
            target = slot;
            nextLayout = upgraded;
          }
        }
        if (target < 0) {
          // 真的没地方了（2x2 满 / 没升上去）：替换「终端1」所在的槽 0（用户
          // 定稿：新的进终端1的位置，原本那块回左栏后台），而不是顶掉焦点面板。
          next = [...prev];
          next[0] = tabId;
          target = 0;
        }
      }
    }
    if (nextLayout) {
      setLayout(nextLayout);
      setShowEmptySlots(false);
    }
    // 目标槽位若正被框线覆盖隐藏（比例极值），落进去就恢复显示。
    revealPaneRatio(target);
    setPaneBindings(next);
    setFocusedPane(target);
  };
  /** 切换布局：先按新面板数裁/补绑定，再把焦点夹回范围内。
      显式点布局按钮 = 用户要重新摆格子：覆盖态（比例极值）连同记忆的原宽一起清掉。
      **激活的终端必须在切换后仍然看得见**：2 格切 1 格时如果用户看的是第二块，
      单纯裁剪绑定会把那一块裁掉、屏幕上反而留下后台那个——用户点的是「单格」，
      拿到的却是另一个终端。所以先把激活页签搬到槽 0 再裁（纯函数、有单测）。 */
  const handleLayoutChange = (next: TerminalLayout) => {
    setLayout(next);
    setShowEmptySlots(next !== "1x1");
    const arranged = promoteTabToFirstSlot(paneBindings, activeTabIdRef.current, next);
    setPaneBindings(arranged);
    // 焦点跟着激活页签落到它现在所在的槽位，键盘输入不会打到旁边那格去。
    const focusSlot = activeTabIdRef.current
      ? arranged.indexOf(activeTabIdRef.current)
      : focusedPane;
    setFocusedPane(Math.max(0, Math.min(focusSlot < 0 ? 0 : focusSlot, paneCount(next) - 1)));
    setColRatios([0.5, 0.5]);
    setRowRatios([0.5]);
    collapseOriginRef.current = { col: [null, null], row: null };
  };
  /** 选中左栏某个终端：优先复用已绑定的面板，否则占用空面板。 */
  const handleSelectTab = (tabId: string) => {
    activeTabIdRef.current = tabId;
    setActiveTabId(tabId);
    bindTabToPane(tabId);
  };
  /**
   * 面板叉删（需求 8）：只把该槽位解绑，页签仍在左栏。
   * *后面的面板依次前移补位**，不留空洞——否则 2x2 里删掉第 1 块会留一块空白。
   */
  const handleDetachPane = (tabId: string) => {
    // 面板已经不显示这块终端了：重试与提示都不该继续挂在后台跑。
    stopSshRetry(tabId);
    dismissPaneNotice(tabId);
    setPaneBindings((prev) => {
      const index = prev.indexOf(tabId);
      if (index < 0) return prev;
      const next = [...prev];
      // 从该槽位起，后面的内容整体左移一位，末尾补 null。
      for (let cursor = index; cursor < next.length - 1; cursor += 1)
        next[cursor] = next[cursor + 1];
      next[next.length - 1] = null;
      return next;
    });
  };
  /**
   * 拖放换位（需求 ⑤）：把页签放到目标面板槽位。
   * 判定全部交给 `planDropOnPane`（纯函数、有单测）：指针必须真的落在目标面板
   * 内容盒内（`xRatio`/`yRatio` 在 0..1 开区间），否则一律原样返回——跨面板拖拽
   * 时鼠标很容易在松手瞬间滑到边缘外，光凭 slot 会把"擦边"当成"换位"。
   * 与旧实现的区别：目标槽位若**已有终端**且用户把窗口拖过去，语义是两者互换
   * （用户拖 A 压到 B 身上，期望的是换位，不是把 B 顶掉）。
   */
  const handleDropTab = (slotIndex: number, tabId: string, xRatio: number, yRatio: number) => {
    const from = paneBindings.indexOf(tabId);
    // 拖动源不是面板里的窗口（比如从左栏直接拖来），就没有"换位"这回事，
    // 退回原来的语义：占用目标空槽。
    if (from < 0) {
      if (!canAcceptDrop(paneBindings, slotIndex)) {
        // 目标面板被占：若还有被抢占后隐藏的空槽，放回空槽而不是拒绝。
        const spare = paneBindings.indexOf(null);
        if (spare >= 0) {
          setPaneBindings((prev) => {
            const next = [...prev];
            if (spare < 0 || spare >= next.length) return prev;
            next[spare] = tabId;
            return next;
          });
          setFocusedPane(spare);
          handleSelectTab(tabId);
          showToast("已放回空闲面板。", "info");
          return;
        }
        showToast("这个面板已经有终端在用了，请拖到空闲面板。", "info");
        return;
      }
      setPaneBindings((prev) => {
        const next = [...prev];
        if (slotIndex < 0 || slotIndex >= next.length) return prev;
        next[slotIndex] = tabId;
        return next;
      });
      setFocusedPane(slotIndex);
      handleSelectTab(tabId);
      return;
    }
    const plan = planDropOnPane(paneBindings, from, slotIndex, xRatio, yRatio);
    if (plan.bindings === paneBindings) return;
    setPaneBindings(plan.bindings);
    setLayout(plan.layout);
    setFocusedPane(slotIndex);
    handleSelectTab(tabId);
  };
  /**
   * 拖动中的实时预览：**只写 DOM 变量，不 setState**。
   * 终端视图是全局最重的一屏（四个面板各持一个内核实例），每帧重渲染会把
   * canvas 一起重排。写 CSS 变量让浏览器自己做布局，React 一次都不渲染。
   */
  /** 边框拖拽（v3 定稿）：由专门的边框热区层触发（盖在面板内容之上，
      空面板的选项按钮不会再挡住拖拽）。
      §22 二十六轮定稿：拖过**邻块拖前宽度的一半** → 邻块当场隐藏（预览直接给
      比例极值）、本侧占满，框线贴在被覆盖块的外侧框线上；反着拖回超过被隐藏块
      原宽的一半 → 当场恢复。空面板与真终端同一条规则，松手才落 state。 */
  const beginEdgeResize = (event: React.PointerEvent<HTMLDivElement>, axis: "col" | "row", rowIndex: number) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const startRatio = axis === "col" ? colRatios[rowIndex] ?? 0.5 : rowRatios[0] ?? 0.5;
    // 起点已是覆盖态（极值）：回拖的恢复线用记忆的原宽，而不是起点比例本身。
    const wasCollapsed = startRatio >= 0.99 || startRatio <= 0.01;
    const origin = wasCollapsed
      ? (axis === "col" ? collapseOriginRef.current.col[rowIndex] : collapseOriginRef.current.row) ?? 0.5
      : null;
    const rowRect = (event.currentTarget.closest(".terminal-pane-row") as HTMLElement | null)?.getBoundingClientRect();
    const gridRect = paneGridRef.current?.getBoundingClientRect();
    // 拖动中允许逼近到 2%：判定线最高到 (1+0.92)/2≈0.96，夹太窄会够不到线。
    const clampRatio = (value: number) => Math.min(0.98, Math.max(0.02, value));
    const pointerRatio = (point: PointerEvent): number => {
      if (axis === "col") {
        return rowRect && rowRect.width > 0 ? clampRatio((point.clientX - rowRect.left) / rowRect.width) : startRatio;
      }
      return gridRect && gridRect.height > 0 ? clampRatio((point.clientY - gridRect.top) / gridRect.height) : startRatio;
    };
    const onMove = (move: PointerEvent) => {
      const raw = pointerRatio(move);
      // 覆盖态回拖**全程跟手**：边线贴着指针走、被隐藏侧随拖动长大，松手才按
      // 「是否拖过原宽一半」决定恢复还是弹回覆盖。普通拖拽保持磁吸：越线即把
      // 被吃一侧压到 0 预览（用户已定稿的前向反馈），退回线内立刻还原。
      if (wasCollapsed) {
        previewPaneRatio(axis, rowIndex, raw);
        return;
      }
      const side = paneCoverSide(raw, startRatio, null);
      previewPaneRatio(axis, rowIndex, side === 0 ? 1 : side === 1 ? 0 : raw);
    };
    const onUp = (up: PointerEvent) => {
      detach();
      const side = paneCoverSide(pointerRatio(up), startRatio, origin);
      commitPaneRatio(axis, rowIndex, startRatio, side === 0 ? 1 : side === 1 ? 0 : pointerRatio(up));
    };
    const onCancel = () => {
      detach();
      commitPaneRatio(axis, rowIndex, startRatio, startRatio);
    };
    const detach = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
  };

  const previewPaneRatio = (axis: "col" | "row", rowIndex: number, ratio: number) => {
    // rAF 节流：快速拖拽时每帧最多写一次 DOM（写多了布局跟不上就是用户说的"轻微卡顿"）。
    if (previewFrameRef.current !== null) window.cancelAnimationFrame(previewFrameRef.current);
    previewFrameRef.current = window.requestAnimationFrame(() => {
      previewFrameRef.current = null;
      const grid = paneGridRef.current;
      if (!grid) return;
      if (axis === "col") {
        const row = grid.querySelectorAll<HTMLDivElement>(".terminal-pane-row")[rowIndex];
        const cells = row?.querySelectorAll<HTMLDivElement>(".terminal-pane-cell");
        // 两侧必须同帧同写（ratio / 1-ratio）：只写一侧时另一侧 grow 还是旧值，
        // 总和不为 1，分割线位置 = r/(r+旧值)，看起来像整块在移动而不是缩放。
        if (cells && cells[0]) cells[0].style.flex = `${ratio} 1 0%`;
        if (cells && cells[1]) cells[1].style.flex = `${1 - ratio} 1 0%`;
      } else {
        const rows = grid.querySelectorAll<HTMLDivElement>(".terminal-pane-row");
        if (rows[0]) rows[0].style.flex = `${ratio} 1 0%`;
        if (rows[1]) rows[1].style.flex = `${1 - ratio} 1 0%`;
      }
    });
  };;
  /**
   * 拖动结束：把最终比例落到 state，并按「半宽覆盖」定稿落位（§22 二十六轮）。
   * `beginEdgeResize` 已在线判定过覆盖方向，这里再按同样的规则复核一次（取消 /
   * 键盘等旁路调用也走这里，必须自洽）：越过邻块拖前半宽 → 被吃一侧比例压到
   * 极值（框线贴它的外侧框线、视觉消失、本侧占满），**槽位与绑定全保留**——
   * 终端只是隐藏后台存活，回拖超过它原宽的一半即恢复，因此不再弹确认、
   * 也不再把关掉的终端退回左栏。中间区域是普通缩放（或覆盖态恢复落点）。
   */
  const commitPaneRatio = (axis: "col" | "row", rowIndex: number, startRatio: number, ratio: number) => {
    const grid = paneGridRef.current;
    // 清掉预览的行内样式，比例改由 React state 接管。
    const rows = grid?.querySelectorAll<HTMLDivElement>(".terminal-pane-row") ?? [];
    rows.forEach((row) => {
      row.style.flex = "";
      row.querySelectorAll<HTMLDivElement>(".terminal-pane-cell").forEach((cell) => {
        cell.style.flex = "";
      });
    });
    const setRatio = (value: number) => {
      if (axis === "col") setColRatios((prev) => prev.map((v, index) => (index === rowIndex ? value : v)));
      else setRowRatios([value]);
    };
    const current = axis === "col" ? colRatios[rowIndex] ?? 0.5 : rowRatios[0] ?? 0.5;
    const wasCollapsed = current >= 0.99 || current <= 0.01;
    const origin = wasCollapsed
      ? (axis === "col" ? collapseOriginRef.current.col[rowIndex] : collapseOriginRef.current.row) ?? 0.5
      : null;
    const side = paneCoverSide(ratio, current, origin);
    // side=null → 普通缩放（或覆盖态回拖过半宽的恢复）；否则输家压到极值
    // （side=0 → 右/下隐藏、前一块独占；side=1 反之）。
    const next = side === null ? ratio : side === 0 ? 1 : 0;
    const sameSideCollapsed = side !== null && wasCollapsed && (side === 0 ? current >= 0.99 : current <= 0.01);
    if (side !== null && !sameSideCollapsed) {
      // 新一轮覆盖（含从覆盖态反向拖过可见侧半宽的「翻边」）：记忆覆盖前
      // 边线位置，它就是输家的拖前原宽，回拖恢复线 = 这条原宽的一半。
      if (axis === "col") collapseOriginRef.current.col[rowIndex] = startRatio;
      else collapseOriginRef.current.row = startRatio;
    }
    if (side === null && wasCollapsed) {
      // 从覆盖态回拖过半宽：恢复显示，清掉原宽记忆。
      if (axis === "col") collapseOriginRef.current.col[rowIndex] = null;
      else collapseOriginRef.current.row = null;
    }
    setRatio(next);
    if (next === current) {
      // 提交值与 state 相同（如回拖未过线弹回覆盖）时 React 会跳过重渲染，
      // 但函数开头清掉的拖动预览行内样式就没人补了——格子会回落到样式表默认
      // 的对半 flex，叠加 is-collapsed 隐藏内容 = 半屏空白。手动按 state 写回。
      previewPaneRatio(axis, rowIndex, next);
    }
    if (side === null) return; // 普通缩放 / 恢复不动焦点。
    // 焦点交给赢家：被压扁的格子不再吃键盘输入与设备条。
    const perRow = layout === "1x1" || layout === "2v" ? 1 : 2;
    if (axis === "col") {
      setFocusedPane(rowIndex * perRow + side);
    } else {
      const winnerRow = side === 0 ? 0 : 1;
      setFocusedPane(layout === "2x2" ? winnerRow * 2 : winnerRow);
    }
  };
  /**
   * 拖拽落点：把「已保存服务器」拖到某个空面板槽位上，直接在那个槽位开终端。
   * 与 `handleDropTab` 的区别：页签拖拽是换位，这个是新建。所以先开页签，再
   * 把它绑到目标槽位——`handleAddTab` 内部会调 `bindTabToPane` 选第一个空槽，
   * 未必等于用户松手的那个，因此这里开完再覆盖一次绑定。
   */
  const handleDropSavedServer = (slotIndex: number, kind: "ssh" | "wsl", id: string) => {
    const server = savedServers.find((item) => item.kind === kind && item.id === id);
    if (!server) {
      showToast("这个服务器已不在列表里，请刷新后重试。", "error");
      return;
    }
    void handleAddTab(kind, server.name, undefined, server.id, server.root, slotIndex);
  };
  /** 左栏右键菜单入口。 */
  const handleTabContextMenu = (event: React.MouseEvent, tabId: string) => {
    event.preventDefault();
    setSidebarMenu({
      x: event.clientX,
      y: event.clientY,
      trigger: event.currentTarget as HTMLElement,
      tabId,
    });
  };
  /** 重命名页签（右键菜单项）。 */
  const renameTab = async (tabId: string) => {
    const current = tabs.find((tab) => tab.id === tabId);
    if (!current) return;
    const next = window.prompt("重命名终端", current.title);
    const trimmed = next?.trim();
    if (!trimmed || trimmed === current.title) return;
    setTabs((prev) => prev.map((tab) => (tab.id === tabId ? { ...tab, title: trimmed } : tab)));
  };
  /** 关闭页签（右键菜单用，复用左栏 X 的同一条路径）。 */
  const closeTabById = async (tabId: string) => {
    const sessionId = sessionsRef.current[tabId];
    if (desktop && sessionId)
      await invoke("close_terminal_session", { sessionId }).catch(() => undefined);
    forgetTerminalSession(tabId);
    delete outputBuffers.current[tabId];
    delete paneHandlesRef.current[tabId];
    setPaneBindings((prev) => {
      const index = prev.indexOf(tabId);
      if (index < 0) return prev;
      const next = [...prev];
      for (let cursor = index; cursor < next.length - 1; cursor += 1)
        next[cursor] = next[cursor + 1];
      next[next.length - 1] = null;
      return next;
    });
    const remaining = tabs.filter((tab) => tab.id !== tabId);
    setTabs(remaining);
    if (activeTabIdRef.current === tabId) {
      const fallback = remaining[remaining.length - 1]?.id ?? "";
      activeTabIdRef.current = fallback;
      setActiveTabId(fallback);
    }
  };
  const [showNewMenu, setShowNewMenu] = useState(false);
  const [terminalMenu, setTerminalMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
  } | null>(null);
  /** 左栏终端卡片右键菜单（需求：左侧终端项可右键）。 */
  const [sidebarMenu, setSidebarMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
    tabId: string;
  } | null>(null);
  /** 左栏已保存服务器右键菜单（需求 ⑧）。 */
  const [serverMenu, setServerMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
    server: SavedServerEntry;
  } | null>(null);
  // 右栏抽屉：默认展开主机状态，点同一按钮可收起。
  const [drawerPane, setDrawerPane] = useState<DrawerPane | null>("host");
  // 右栏宽度可拖拽（原来硬编码 262px，文件树里的长路径全被截断）。
  // 容器用三栏骨架那一层测宽，跟代码查看器左侧工程栏同一套 usePaneWidth。
  const workspaceBodyRef = useRef<HTMLDivElement>(null);
  const drawerWidth = usePaneWidth(workspaceBodyRef, Boolean(drawerPane), "android-tool.terminal.drawer-width", TERMINAL_DRAWER_PANE);
  const [savedServers, setSavedServers] = useState<SavedServerEntry[]>([]);
  const [savedServersLoading, setSavedServersLoading] = useState(false);
  // 左栏「SSH 远程服务器」= 新建连接弹窗（不再只挑第一条已保存连接）。
  // 同一个弹窗也承担「编辑现有连接」：`editing` 有值就是编辑模式（需求 ⑧）。
  const [sshDialog, setSshDialog] = useState<{
    editing?: EditSshTarget;
  } | null>(null);
  // 顶栏胶囊里的 git 分支来自真实工作目录。
  const [gitBranch, setGitBranch] = useState("");
  // 本机工作目录：右栏 Git/文件面板与左栏副标题都依赖它。
  const [workspaceRoot, setWorkspaceRoot] = useState("");
  // 原 `sessionsRecords` 状态已删除：它只有 4 处 setState、全代码库没有读取点，
  // 每次写入都会触发整页重渲染（性能要求 ⑪），而输出记录实际存在 blockTracker 里。
  const handoff = useTerminalWindowState(
    "shell",
    tabs.map((tab) => ({ ...tab, sessionId: sessionsRef.current[tab.id] })),
    activeTabId,
    outputBuffers.current,
    () => openingPtyRef.current > 0,
    () => Object.values(sessionsRef.current),
  );
  // 快照里带上焦点/分栏比例/空槽模式：悬浮窗拆离与切页缓存共用这份捕获，
  // 两条恢复路径就都能原样还原工作区（含覆盖态）。
  useToolWindowState("shell-ui", () => ({
    layout,
    paneBindings,
    focusedPane,
    colRatios,
    rowRatios,
    sidebarCollapsed,
  }));
  // 切页缓存：离开终端页前把整份工作区快照（shell + shell-ui 两条）留在内存，
  // 回来时在组件顶部 restoreToolWindowState 灌回——与悬浮窗拆离同一套恢复链路。
  // 手动构建而不走 captureToolWindowState：卸载时各 effect 清理顺序不保证注册表
  // 里的捕获函数还活着（useToolWindowState 的清理可能先跑），这里直接读镜像 ref。
  const uiStateRef = useRef({ layout, paneBindings, focusedPane, colRatios, rowRatios, sidebarCollapsed });
  uiStateRef.current = { layout, paneBindings, focusedPane, colRatios, rowRatios, sidebarCollapsed };
  useEffect(
    () => () => {
      const leavingTabs = tabsRef.current;
      saveTerminalPageState({
        shell: {
          tabs: leavingTabs,
          activeId: activeTabIdRef.current,
          sequences: handoff.sequences(),
          buffers: Object.fromEntries(
            leavingTabs.map((tab) => [tab.id, readTerminalText(outputBuffers.current[tab.id])]),
          ),
        },
        "shell-ui": uiStateRef.current,
      });
    },
    // 卸载时才需要；依赖项全部经 ref 读取。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  /**
   * 顶部菜单栏的动作入口。
   *
   * 菜单栏在终端视图之外（见 App.tsx），所以动作通过窗口事件送进来；这里只把它
   * 映射到上面已有的处理函数，不引入新状态。ref 形式保证事件监听只注册一次，
   * 但每次渲染都能拿到最新的 tabs / 布局 / 面板状态。
   */
  const menuActionRef = useRef<(detail: TerminalMenuDetail) => void>(() => undefined);
  menuActionRef.current = (detail) => {
    switch (detail.action) {
      case "new":
        void handleAddTab(
          "local",
          detail.value === "cmd" ? "命令提示符" : detail.value === "pwsh" ? "PowerShell" : "Windows PowerShell",
          detail.value,
          undefined,
          workspaceRoot,
        );
        break;
      case "new-wsl":
        void handleAddTab("wsl", "WSL");
        break;
      case "new-ssh":
        setSshDialog({});
        break;
      case "close-current":
        if (activeTabIdRef.current) void closeTabById(activeTabIdRef.current);
        break;
      case "layout":
        handleLayoutChange(detail.value);
        break;
      case "pane":
        handleToggleDrawer(detail.value);
        break;
    }
  };
  useEffect(() => {
    const onMenu = (event: Event) => {
      const detail = (event as CustomEvent<TerminalMenuDetail>).detail;
      if (detail) menuActionRef.current(detail);
    };
    window.addEventListener(TERMINAL_MENU_EVENT, onMenu);
    return () => window.removeEventListener(TERMINAL_MENU_EVENT, onMenu);
  }, []);
  useApplicationPageAction("open.tool.shell", (action) => {
    if (!desktop) {
      notifyDesktopRequired(action.actionId);
      return;
    }
    const command = action.parameters.command;
    if (command) {
      // 命令预填到焦点面板的会话里，但只写进命令行（不自动回车），等用户确认。
      const tabId = activeTabIdRef.current;
      const sessionId = sessionsRef.current[tabId];
      if (sessionId) queueTerminalInput(sessionId, command);
    }
    window.requestAnimationFrame(() => {
      const tabId = activeTabIdRef.current;
      paneHandlesRef.current[tabId]?.focus();
    });
    const detail = command
      ? "命令已预填到终端；请在终端中确认内容后按 Enter 发送。"
      : "终端已就绪；请在终端中输入并确认命令。";
    notify(detail, "info");
    publishAiTaskActivity({
      running: false,
      status: "done",
      title: "终端已就绪",
      detail,
      actionId: action.actionId,
      progress: 100,
    });
  });
  // 常用命令
  const quickCommands = [
    { label: "进程", command: "ps -A" },
    { label: "存储", command: "df -h" },
    { label: "网络", command: "ip addr show" },
    { label: "CPU", command: "top -b -n 1" },
  ];
  const showToast = (text: string, kind: "success" | "error" | "info" = "info") => {
    notify(text, kind);
  };
  /**
   * 终端工作区作用域的提示（v3）：连接「正在连接 / 无法连接」这类反馈只与终端
   * 视图相关，不该用全局屏幕中心 toast 打断别的页面（聊天可能还在跑任务）。
   * **按面板（tabId）归属**：用户开了 2 块面板、只有一块在连 SSH 时，提示必须只
   * 出现在那一块里——单份中栏提示会让用户以为另一块也出了问题。
   */
  const [paneNotices, setPaneNotices] = useState<Record<string, PaneNotice>>({});
  /** 每个面板提示的自动消失计时器（按 tabId 索引，关面板时一并清掉）。 */
  const paneNoticeTimersRef = useRef<Record<string, number>>({});
  /** SSH 连接门控的单飞记录：serverId → 是否有进行中的尝试。 */
  const sshConnectingRef = useRef<Set<string>>(new Set());
  const dismissPaneNotice = (tabId: string) => {
    const timer = paneNoticeTimersRef.current[tabId];
    if (timer !== undefined) {
      window.clearTimeout(timer);
      delete paneNoticeTimersRef.current[tabId];
    }
    setPaneNotices((current) => {
      if (!(tabId in current)) return current;
      const next = { ...current };
      delete next[tabId];
      return next;
    });
  };
  /**
   * 往**某一块面板**里推一条提示。失败提示默认停留 30s 且可手动关（错误原因往往是
   * 一长串主机/端口/超时信息，4.5s 读不完）；进行中/成功 4.5s 自动消失。
   * 同文本去重只续期，多路径/连点不会叠出多条一样的提示。
   */
  const showPaneNotice = (
    tabId: string,
    text: string,
    kind: PaneNoticeKind = "info",
    durationMs = kind === "error" ? 30_000 : 4500,
  ) => {
    const timer = paneNoticeTimersRef.current[tabId];
    if (timer !== undefined) window.clearTimeout(timer);
    setPaneNotices((current) => {
      const existing = current[tabId];
      if (existing && existing.text === text && existing.kind === kind) return current;
      return { ...current, [tabId]: { id: Date.now(), text, kind } };
    });
    paneNoticeTimersRef.current[tabId] = window.setTimeout(() => {
      delete paneNoticeTimersRef.current[tabId];
      setPaneNotices((current) => {
        if (!(tabId in current)) return current;
        const next = { ...current };
        delete next[tabId];
        return next;
      });
    }, durationMs);
  };
  /**
   * SSH 断连后的自动重试（每 10s 一次、共 10 次），状态按 tabId 归属。
   * 计时器不放在 React 状态里：重试节奏必须精确，且面板卸载时要能可靠清掉。
   */
  const [sshRetries, setSshRetries] = useState<Record<string, SshRetryState>>({});
  /** 与 sshRetries 同步的 ref：键盘回调里要读最新阶段，不能再依赖闭包快照。 */
  const sshRetriesRef = useRef<Record<string, SshRetryState>>({});
  sshRetriesRef.current = sshRetries;
  const sshRetryTimersRef = useRef<Record<string, number>>({});
  const [retryTick, setRetryTick] = useState(0);
  /** 每次重试的尝试序号，用来作废"重试进行中又来了新事件"的迟到结果。 */
  const sshRetryAttemptRef = useRef<Record<string, number>>({});
  const stopSshRetry = (tabId: string, phase?: SshRetryState["phase"]) => {
    const timer = sshRetryTimersRef.current[tabId];
    if (timer !== undefined) {
      window.clearTimeout(timer);
      delete sshRetryTimersRef.current[tabId];
    }
    delete sshRetryAttemptRef.current[tabId];
    setSshRetries((current) => {
      const existing = current[tabId];
      if (!existing) return current;
      // phase 为空 = 彻底移除（连上/关面板）；否则只切阶段并停掉自动节奏。
      if (!phase) {
        const next = { ...current };
        delete next[tabId];
        return next;
      }
      return { ...current, [tabId]: { ...existing, phase, nextAt: null } };
    });
  };
  useEffect(() => {
    // 倒计时文案每秒刷新一次；没有待重试的面板就不挂定时器。
    if (!Object.values(sshRetries).some((item) => item.phase === "retrying" && item.nextAt)) return;
    const timer = window.setInterval(() => setRetryTick((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [sshRetries]);
  useEffect(() => () => {
    for (const timer of Object.values(paneNoticeTimersRef.current)) window.clearTimeout(timer);
    for (const timer of Object.values(sshRetryTimersRef.current)) window.clearTimeout(timer);
  }, []);
  const registerTerminalSession = (tabId: string, sessionId: string) => {
    const previous = sessionsRef.current[tabId];
    if (previous) delete sessionTabsRef.current[previous];
    sessionsRef.current[tabId] = sessionId;
    sessionTabsRef.current[sessionId] = tabId;
    // 命令块层按 session 索引：登记时要带上 target 类型，提示符正则按 target 区分。
    const tab = tabs.find((item) => item.id === tabId);
    blockTracker.registerSession(sessionId, {
      targetKind: tab?.type ?? "local",
      cwd: tab?.root, // 只有明确支持的 shell 才吃 OSC 133，走前端主导切块。
      osc133: tab?.type === "wsl" || tab?.type === "ssh",
    });
  };
  const forgetTerminalSession = (tabId: string) => {
    const sessionId = sessionsRef.current[tabId];
    // 终端直起的 ssh.exe 不经过后端 SshRuntimeState，需要显式撤销全局覆盖，
    // 否则关掉 SSH 终端后右上角还会一直显示"已连接"。
    const sshConnectionId = sshTabRef.current[tabId];
    if (sshConnectionId) {
      delete sshTabRef.current[tabId];
      reportTerminalSshSession(sshConnectionId, "closed");
    }
    if (!sessionId) return undefined;
    delete sessionsRef.current[tabId];
    delete sessionTabsRef.current[sessionId];
    delete inputQueueRef.current[sessionId];
    delete inputWritesRef.current[sessionId];
    delete lastResizeRef.current[sessionId];
    // 会话关闭时连同它的命令块一起丢弃，避免旧块继续被 UI 引用。
    blockTracker.forgetSession(sessionId);
    handoff.retainSessions(Object.values(sessionsRef.current));
    return sessionId;
  };
  const flushInputQueue = () => {
    for (const sessionId of Object.keys(inputQueueRef.current)) {
      const input = takeTerminalText(inputQueueRef.current[sessionId], INPUT_CHARS_PER_BATCH);
      if (!input) continue;
      const prior = inputWritesRef.current[sessionId] || Promise.resolve();
      const write = prior
        .catch(() => undefined)
        .then(() => invoke<void>("write_terminal_input", { sessionId, input }));
      inputWritesRef.current[sessionId] = write.catch((reason) => {
        const message = formatInvokeError(reason);
        // 后端已经没有这条会话（应用重启过、后端异常退出、上游把 PTY 关了）：
        // 不要把「终端会话不存在或已经关闭」当成输入失败弹给用户——页签还在、
        // 输出还在，只是底座掉了。丢掉死会话，原地重开一条，用户接着敲即可。
        const tabId = sessionTabsRef.current[sessionId];
        if (tabId && message.includes("终端会话不存在")) {
          forgetTerminalSession(tabId);
          delete inputQueueRef.current[sessionId];
          void reconnectTab(tabId).catch((error) => showToast(formatInvokeError(error), "error"));
          return;
        }
        showToast(message, "error");
      });
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
    if (isImmediateTerminalInput(input) && inputFrameRef.current !== null) {
      window.cancelAnimationFrame(inputFrameRef.current);
      inputFrameRef.current = null;
    }
    if (isImmediateTerminalInput(input)) {
      flushInputQueue();
      return;
    }
    scheduleInputFlush();
  };
  const writeShellOutput = (tabId: string, data: string) => {
    const sessionId = sessionsRef.current[tabId];
    if (sessionId) blockTracker.feedOutput(sessionId, data);
    // SSH 终端：收到第一批远端输出就算连上了（需求②）。
    // ssh.exe 自己不会通知我们，只能拿"有输出"当连接建立的信号；
    // 上报有去重（同一状态只写一次），不会在数据热路径上产生额外开销。
    const sshConnectionId = sshTabRef.current[tabId];
    if (sshConnectionId && data) {
      reportTerminalSshSession(sshConnectionId, "connected");
    } // 回滚缓冲按页签保留，跟面板绑定无关：同一页签换到别的面板后仍能重放。
    appendTerminalText(getTerminalTextBuffer(outputBuffers.current, tabId), data, 300000);
    // 面板自行做帧预算与排队（TerminalPane 内的 rAF 泵），父级只投递。
    paneHandlesRef.current[tabId]?.write(data);
    // 块状态可能刚变成终态，刷新一下"能否解释"的入口（setState 自带去重）。
    refreshLatestBlock();
  };
  /**
   * 面板键盘输入回传（设计文档 §3.5）：面板只管读内核，写 PTY 由父级统一排队。
   * 这里同时是**命令块层的输入侧接入点**（开发方案 8.4）。
   */
  const handlePaneInput = (tabId: string, data: string) => {
    const sessionId = sessionsRef.current[tabId];
    if (!sessionId) {
      // 没有会话 = 这块面板的 SSH 已经断了。自动重试耗尽后（waiting-input），
      // 用户在这里敲任意**可打印字符**即立刻触发一次重连——方向键/回车不算，
      // 那些是下意识想"唤醒"一下的按键，拿它们当重连意图太容易误触。
      const retry = sshRetriesRef.current[tabId];
      if (retry && retry.phase === "waiting-input" && shouldReconnectOnInput(data)) {
        resumeSshRetry(tabId);
      }
      return;
    }
    // 输入侧 AI 面板开着时按键不进 PTY（9.3.1：面板与内核键盘输入互斥）。
    // 正常使用下面板持有焦点，这里兜的是"面板开着但用户点了画布"的漏网情况。
    if (composerTabRef.current === tabId) return;
    blockTracker.feedInput(sessionId, data);
    queueTerminalInput(sessionId, data);
  };
  /** 面板尺寸变化 → 同步 PTY 尺寸，避免换行位置与屏幕不一致。 */
  /**
   * 已按会话发出的 PTY 尺寸（`${cols}x${rows}`）。面板实例里的 lastSize 去重是
   * **实例级**的：拖拽互换/换绑会让面板按 key 重挂载，新实例首次 fit 必定上报
   * 一次尺寸——尺寸其实没变也发 resize。Windows 远端的 ConPTY/PSReadLine 收到
   * resize 就重画一行提示符（不看尺寸变没变），表现为「每拖一次多一行 >」；
   * 去重提到父级按会话记忆后，同尺寸的 resize 一律不再发。
   */
  const lastResizeRef = useRef<Record<string, string>>({});
  const resizeSession = (sessionId: string, cols: number, rows: number) => {
    const size = `${cols}x${rows}`;
    if (lastResizeRef.current[sessionId] === size) return;
    lastResizeRef.current[sessionId] = size;
    void invoke("resize_terminal_session", { sessionId, cols, rows }).catch(() => undefined);
  };
  const handlePaneResize = (tabId: string, cols: number, rows: number) => {
    const sessionId = sessionsRef.current[tabId];
    if (!desktop || !sessionId) return;
    resizeSession(sessionId, cols, rows);
  };
  /** 面板右键菜单：复用顶栏那套终端操作。 */
  const handlePaneContextMenu = (event: React.MouseEvent, tabId: string) => {
    activeTabIdRef.current = tabId;
    setActiveTabId(tabId);
    setTerminalMenu({
      x: event.clientX,
      y: event.clientY,
      trigger: event.currentTarget as HTMLElement,
    });
  };
  /** 面板实例登记 / 注销：输出按 tabId 投递，面板卸载时清掉句柄。 */
  const handlePaneRegister = (tabId: string, handle: TerminalPaneHandle | null) => {
    if (handle) paneHandlesRef.current[tabId] = handle;
    else delete paneHandlesRef.current[tabId];
  };
  /**
   * 交互式程序（vim / top 实时流）进入直通模式（开发方案 8.5 / F54）：
   * 不切块、不记录按键，避免把全屏程序里的按键当成命令文本。
   */
  const handlePaneInteractiveChange = (tabId: string, interactive: boolean) => {
    const sessionId = sessionsRef.current[tabId];
    if (!sessionId) return;
    blockTracker.setPassthrough(sessionId, interactive, interactive ? "alt-screen" : undefined);
    // 输入侧 AI 面板要知道自己所在的页签是否在直通（9.3.4）。
    setPassthroughTabs((prev) =>
      Boolean(prev[tabId]) === interactive ? prev : { ...prev, [tabId]: interactive },
    );
  };
  /**
   * 内核报上来的工作目录（设计文档 §15 cwd 跟随）。
   *
   * 来源是 OSC 7：bash/zsh 在每条提示符前把 `file://host/path` 发给宿主，
   * 自研内核解析成 `onPwdChange`。它跟着真实 shell 状态走，比"截取命令里的 cd"
   * 可靠（多行脚本、`cd` 到不存在的目录都不会误报）。
   *
   * 只认 SSH、WSL 与本地 shell：目录没变就返回原数组，避免每次提示符
   * 刷新都触发一轮右栏目录列举。
   */
  const handlePanePwdChange = (tabId: string, path: string) => {
    setTabs((prev) => {
      const index = prev.findIndex((item) => item.id === tabId);
      if (index < 0) return prev;
      const target = prev[index];
      // OSC 7 报上来的是 `file://host/path` 这种 URI，不是路径；不归一化就会把
      // URI 当成 root 塞进右栏，目录列举必然失败（表现为「不跟随 cd」）。
      const next = normalizeTerminalPwd(path, target.type);
      if (!next || (target.root || "") === next) return prev;
      const mapped = prev.slice();
      // 首次目录变化前捕获「终端打开时的位置」：右栏跟随关闭时锚定用（§22）。
      mapped[index] = { ...target, root: next, rootAtOpen: target.rootAtOpen ?? target.root };
      return mapped;
    });
  };
  /**
   * SSH 兜底：并非所有远端 shell 都发 OSC 7（sh/dash，或用户清过 PROMPT_COMMAND），
   * 这时页签 root 会一直停在初始值。对**焦点** SSH 页签每 5 秒问一次 `pwd`；
   * 页面隐藏时整段跳过，避免后台空转占着一条 SSH 通道。
   */
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let inFlight = false;
    const poll = async () => {
      if (disposed || inFlight) return;
      if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
      const active = tabsRef.current.find((item) => item.id === activeTabIdRef.current);
      if (!active || active.type !== "ssh") return;
      const connectionId = active.profileId?.trim();
      if (!connectionId) return;
      const tabId = active.id;
      inFlight = true;
      try {
        // 带上已知 root：远端每条命令都是新进程，不先 cd 过去只会拿到登录目录。
        const cwd = normalizeTerminalPwd(
          (await readSshWorkingDirectory(connectionId, active.root || "")).trim(),
          "ssh",
        );
        if (!cwd || disposed) return;
        setTabs((prev) => {
          const index = prev.findIndex((item) => item.id === tabId);
          if (index < 0) return prev;
          const target = prev[index];
          if ((target.root || "") === cwd) return prev;
          const mapped = prev.slice();
          mapped[index] = { ...target, root: cwd, rootAtOpen: target.rootAtOpen ?? target.root };
          return mapped;
        });
      } catch {
        // 探测失败不改任何状态：连接刚断开 / 尚无通道是常态，不打扰用户。
      } finally {
        inFlight = false;
      }
    };
    // 5s 一次：远端 pwd 只读一次环境，开销很小，但太密会占住 SSH 通道。
    const timer = window.setInterval(() => void poll(), 5000);
    void poll();
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [desktop]);
  /**
   * 等承载页签的面板完成挂载并量出真实尺寸（最多 ~350ms）。
   *
   * `handleAddTab` 里 bind 之后**同一拍**就调 `openPty`，React 往往还没把新面板
   * 渲染出来、`paneHandlesRef` 里还没有句柄——直接量只能拿到 undefined，PTY
   * 只能按 120x32 默认值开，连上后再补一次 resize 拉回真实宽度。而这次必然
   * 发生的 resize 会给远端发 SIGWINCH，shell 在下一行重画提示符——用户看到的
   * 就是 SSH 连上后多出一行「>」。等挂载完成、**按真实尺寸开 PTY**，建连后的
   * 补正 resize 就无需发生（见下方「确实变了才补发」）。
   */
  const waitForPaneFit = async (tabId: string): Promise<{ cols: number; rows: number } | null> => {
    const deadline = performance.now() + 350;
    for (;;) {
      const fit = paneHandlesRef.current[tabId]?.fit();
      if (fit && fit.cols > 0 && fit.rows > 0) return fit;
      if (performance.now() >= deadline) return null;
      await new Promise((resolve) => window.setTimeout(resolve, 25));
    }
  };
  const openPty = async (tab: TerminalTab) => {
    if (!desktop) return;
    const target =
      tab.type === "local"
        ? { kind: "local", shell: tab.shell || "powershell" }
        : tab.type === "wsl"
          ? { kind: "wsl", profileId: tab.profileId }
          : { kind: "ssh", connectionId: tab.profileId };
            // 用承载该页签的面板尺寸开 PTY；等不到面板（页签不在任何面板上 /
            // 挂载超时）才按终端默认 120x32。
    const size = await waitForPaneFit(tab.id);
    // SSH 终端：登记到全局状态，右上角才会跟着变绿（需求②）。
    // 走的不是后端 SshRuntimeState，只能由前端显式上报。
    const sshConnectionId = tab.type === "ssh" ? tab.profileId?.trim() || "" : "";
    if (sshConnectionId) {
      sshTabRef.current[tab.id] = sshConnectionId;
      reportTerminalSshSession(sshConnectionId, "connecting");
    }
    openingPtyRef.current++;
    try {
      const info = await invoke<{
        sessionId: string;
      }>("open_terminal_session", { target, cols: size?.cols || 120, rows: size?.rows || 32 });
      if (disposedRef.current) {
        await invoke("close_terminal_session", { sessionId: info.sessionId }).catch(
          () => undefined,
        );
        return;
      }
      registerTerminalSession(tab.id, info.sessionId);
      // 记下 PTY 实际开台尺寸：面板首次 fit 上报同尺寸时由 lastResizeRef 直接
      // 跳过（拖拽互换重挂载不再触发无谓的远端 resize / 提示符重画）。
      const openedCols = size?.cols || 120;
      const openedRows = size?.rows || 32;
      lastResizeRef.current[info.sessionId] = `${openedCols}x${openedRows}`;
      // **PTY 尺寸补正（需求①：SSH 输出错位）**：面板挂载时就会 fit 一次并回调
      // `onResize`，但那一刻 `sessionsRef` 里还没有 sessionId，这次尺寸就丢了；
      // 建好之后用面板当前尺寸补一次（同尺寸时 resizeSession 自己去重）。
      const settled = paneHandlesRef.current[tab.id]?.fit();
      if (settled && settled.cols > 0 && settled.rows > 0) {
        resizeSession(info.sessionId, settled.cols, settled.rows);
      }
      const early = takeTerminalText(pendingOutputRef.current[info.sessionId]);
      if (early) {
        delete pendingOutputRef.current[info.sessionId];
        writeShellOutput(tab.id, early);
      }
      setTabs((prev) =>
        prev.map((item) => (item.id === tab.id ? { ...item, sessionId: info.sessionId } : item)),
      );
    } finally {
      openingPtyRef.current--;
      if (!openingPtyRef.current) {
        pendingOutputRef.current = {};
        handoff.retainSessions(Object.values(sessionsRef.current));
      }
    }
  };
  const handleAddTab = async (
    type: "local" | "wsl" | "ssh",
    title: string,
    shell?: string,
    forcedProfileId?: string,
    root?: string,
    preferSlot?: number,
  ) => {
    setShowNewMenu(false);
    // 每个终端实例常驻一份 WASM 内存（只增不减），页签数必须设上限（开发方案 6.7.2）。
    if (tabs.length >= MAX_TERMINALS) {
      showToast(`最多同时打开 ${MAX_TERMINALS} 个终端，请先关闭不用的页签。`, "error");
      return;
    }
    const newId = `tab-${type}-${Date.now()}`;
    /** SSH 门控成功后的面板内提示（此时才拿到 newId，见下方 setPaneNotice）。 */
    let sshConnectOutcome: { text: string; kind: PaneNoticeKind; durationMs: number } | null = null;
    let profileId = forcedProfileId || "";
    let resolvedTitle = title;
    if (desktop && type === "wsl" && !profileId) {
      const profiles = await invoke<
        Array<{
          id: string;
          name: string;
        }>
      >("list_wsl_profiles");
      if (!profiles.length) {
        showToast("请先在设置中保存 WSL 配置。", "error");
        return;
      }
      profileId = profiles[0].id;
      resolvedTitle = profiles[0].name;
    }
    if (desktop && type === "ssh") {
      const connections = await listSshConnections();
      const target = (forcedProfileId ? connections.find((item) => item.id === forcedProfileId) : undefined) ?? connections[0] ?? null;
      if (!target) {
        showToast("请先在设置中保存 SSH 连接。", "error");
        return;
      }
      profileId = target.id;
      resolvedTitle = target.name;
      // 连接门控（v3）：先走全局 SSH 运行时真实建连（带超时/错误）。失败就提示
      // 「无法连接」并且**不建页签、不占面板**——此前 PTY 直起 ssh.exe，收到
      // "Connection timed out" 这类错误输出也算"有输出"，呼吸灯被点亮成绿色。
      // 单飞：同一服务器的连接尝试进行中时忽略重复点击——否则等待期连点会在
      // 失败时一次性弹出 N 条相同错误（用户实测过的问题）。
      if (sshConnectingRef.current.has(target.id)) return;
      sshConnectingRef.current.add(target.id);
      // 连接反馈先记在局部变量里：此刻页签还没建出来，没有 tabId 可以挂提示。
      // 建好页签（拿到 newId）之后立刻把「正在连接」贴到这块面板上，成功/失败
      // 再原地替换——这样它只出现在用户点的那一块，不会飘到全窗口。
      let connectOutcome: { text: string; kind: PaneNoticeKind; durationMs: number } | null = null;
      try {
        const initial = await connectGlobalSsh(target);
        const progress = initial && (initial.status === "connected" || initial.status === "failed")
          ? initial
          : await awaitSshConnected(target.id);
        if (!progress || progress.status !== "connected") {
          throw new Error(progress?.error || progress?.message || "连接超时");
        }
        connectOutcome = { text: `已连接 ${target.name}`, kind: "success", durationMs: 1800 };
      } catch (reason) {
        const detail = reason instanceof Error ? reason.message : formatInvokeError(reason);
        // 后端错误已带完整信息（「SSH 连接失败：无法连接 SSH 主机 host:port：原因」），
        // 不再外层包「无法连接 name：」——否则同一件事读起来重复两遍。
        // 失败时**不建页签、不占面板**（原门控语义）：错误提示只能挂在中栏，
        // 因为此刻还没有属于它的面板。这属于"没有终端可依附"的例外，不是常规路径。
        showToast(detail || `无法连接 ${target.name}`, "error");
        return;
      } finally {
        sshConnectingRef.current.delete(target.id);
      }
      sshConnectOutcome = connectOutcome;
    }
    const newTab: TerminalTab = {
      id: newId,
      title: resolvedTitle,
      type,
      shell,
      profileId,
      root,
    };
    setTabs((prev) => [...prev, newTab]);
    bindTabToPane(newId, preferSlot, 1);
    activeTabIdRef.current = newId;
    setActiveTabId(newId);
    // 「已连接」贴在刚建好的这块面板上（不是全窗口 toast）。
    if (sshConnectOutcome) {
      showPaneNotice(newId, sshConnectOutcome.text, sshConnectOutcome.kind, sshConnectOutcome.durationMs);
    }
    if (desktop) {
      try {
        await openPty(newTab);
      } catch (reason) {
        showPaneNotice(newId, formatInvokeError(reason), "error");
      }
    }
  };
  // 关闭指定标签
  const handleCloseTab = (e: React.MouseEvent, tabId: string) => {
    e.stopPropagation();
    // 关掉最后一个页签是合法的：关完就退回"空闲面板"，用户再点面板选新终端。
    // 原实现 `if (tabs.length <= 1) return;` 会让唯一一个终端永远关不掉。
    confirmAction({
      title: "结束终端",
      message: `确认关闭「${tabs.find((tab) => tab.id === tabId)?.title || "此终端"}」？运行中的进程会被中断，输出记录将清除。`,
      confirmLabel: "确认结束",
      onConfirm: async () => {
        const sessionId = sessionsRef.current[tabId];
        if (desktop && sessionId) await invoke("close_terminal_session", { sessionId });
        forgetTerminalSession(tabId);
        // 页签没了，它的重试计时器和面板提示也要一起收掉，
        // 否则关掉终端后还会在后台每 10 秒重连一次。
        stopSshRetry(tabId);
        dismissPaneNotice(tabId);
        delete outputBuffers.current[tabId];
        delete paneHandlesRef.current[tabId];
        // 面板绑定里清掉它：面板会变回空面板，而不是继续显示已销毁的页签。
        // 同时把后面的终端往前挤：中间不空着才收得掉多出来的格子（m18580）。
        setPaneBindings((prev) => compactBindings(prev.map((id) => (id === tabId ? null : id))));
        const newTabs = tabs.filter((t) => t.id !== tabId);
        setTabs(newTabs);
        if (activeTabId === tabId) {
          const nextTabId = newTabs[newTabs.length - 1]?.id ?? "";
          activeTabIdRef.current = nextTabId;
          setActiveTabId(nextTabId);
          if (nextTabId) {
            setPaneBindings((prev) => {
              const index = prev.indexOf(nextTabId);
              if (index >= 0) setFocusedPane(index);
              return prev;
            });
          }
        }
      },
    });
  };
  /**
   * 把一条命令写到焦点面板的会话里（快捷命令、页面动作预填共用）。
   * `submit` 为真是直接把命令执行掉（追加 \r），否则只填字等用户确认。
   */
  const executeCommand = (cmd: string, submit = true) => {
    const trimmed = cmd.trim();
    if (!trimmed) return;
    if (!desktop) {
      notifyDesktopRequired();
      return;
    }
    const sessionId = sessionsRef.current[activeTabIdRef.current];
    if (!sessionId) {
      showToast("终端会话尚未建立。", "error");
      return;
    }
    queueTerminalInput(sessionId, submit ? `${trimmed}\r` : trimmed);
  };
  const handleQuickCmdClick = (cmd: string) => {
    executeCommand(cmd);
    const tabId = activeTabIdRef.current;
    paneHandlesRef.current[tabId]?.focus();
  };
  const handleClear = () => {
    const tabId = activeTabIdRef.current;
    clearTerminalTextBuffer(outputBuffers.current[tabId]);
    paneHandlesRef.current[tabId]?.clear();
    // 清屏后旧块的行号已失效，连同块一起清掉，避免 UI 仍然渲染上一屏的命令。
    const sessionId = sessionsRef.current[tabId];
    if (sessionId) blockTracker.blockStore().clearSession(sessionId);
  };
  const copyTerminalSelection = async () => {
    const selection = paneHandlesRef.current[activeTabIdRef.current]?.getSelection();
    if (!selection) return;
    try {
      await navigator.clipboard.writeText(selection);
    } catch (error) {
      notify(`复制失败：${formatInvokeError(error)}`, "error");
    }
  };
  const pasteTerminalClipboard = async () => {
    const handle = paneHandlesRef.current[activeTabIdRef.current];
    if (!handle) return;
    try {
      handle.paste(await navigator.clipboard.readText());
    } catch (error) {
      notify(`粘贴失败：${formatInvokeError(error)}`, "error");
    }
  };
  /**
   * 按 tabId 重连一块面板（原来的 `handleReconnect` 只会处理激活页签，
   * 自动重试/面板内按钮需要精确到出问题的那一格）。
   * 返回是否连上；失败时把原因抛给调用方决定怎么提示。
   */
  const reconnectTab = async (tabId: string): Promise<void> => {
    const tab = tabs.find((item) => item.id === tabId);
    if (!desktop || !tab) throw new Error("终端已在桌面应用中关闭。");
    const previous = forgetTerminalSession(tab.id);
    if (previous) {
      await invoke("close_terminal_session", { sessionId: previous }).catch(() => undefined);
    }
    clearTerminalTextBuffer(outputBuffers.current[tab.id]);
    paneHandlesRef.current[tab.id]?.replay();
    await openPty(tab);
  };
  /**
   * 开始一轮自动重试：每 SSH_RETRY_INTERVAL_MS 一次、最多 SSH_RETRY_MAX_ATTEMPTS 次。
   * 次数用尽后转 `waiting-input`——界面维持原样，等用户在该面板敲键盘再触发。
   */
  const startSshRetry = (tabId: string, reason: string) => {
    const tabType = tabs.find((item) => item.id === tabId)?.type;
    if (tabType !== "ssh") return;
    // 序号归零：新的一轮重试从头数 10 次。
    sshRetryAttemptRef.current[tabId] = 0;
    scheduleSshRetry(tabId, reason, 0);
  };
  /**
   * 排下一轮自动重试（`startSshRetry` 与「敲键后继续」共用）。
   * `attempts` 是已经失败的次数：0 表示刚断开，还没试过。
   */
  const scheduleSshRetry = (tabId: string, reason: string, attempts: number) => {
    const existing = sshRetryTimersRef.current[tabId];
    if (existing !== undefined) window.clearTimeout(existing);
    sshRetryAttemptRef.current[tabId] = attempts;
    setSshRetries((current) => ({
      ...current,
      [tabId]: {
        tabId,
        phase: "retrying",
        attempts,
        nextAt: Date.now() + SSH_RETRY_INTERVAL_MS,
        lastError: reason,
      },
    }));
    const timer = window.setTimeout(() => {
      delete sshRetryTimersRef.current[tabId];
      void attemptSshRetry(tabId, attempts + 1);
    }, SSH_RETRY_INTERVAL_MS);
    sshRetryTimersRef.current[tabId] = timer;
  };
  /** 真正发起第 `attemptNo` 次重连；失败且未用尽就排下一次。 */
  const attemptSshRetry = async (tabId: string, attemptNo: number) => {
    sshRetryAttemptRef.current[tabId] = attemptNo;
    try {
      await reconnectTab(tabId);
      // 连上了：清掉重试状态，只在这块面板里报一句成功。
      if (sshRetryAttemptRef.current[tabId] === attemptNo) {
        stopSshRetry(tabId);
        showPaneNotice(tabId, "已重新连接。", "success", 2500);
      }
    } catch (error) {
      if (sshRetryAttemptRef.current[tabId] !== attemptNo) return;
      const message = formatInvokeError(error);
      const exhausted = attemptNo >= SSH_RETRY_MAX_ATTEMPTS;
      if (exhausted) {
        // 用尽 → 停在 waiting-input：不再自动重试，界面维持原样，
        // 面板提示条换成「敲键盘即可继续」。页签留着，输出也留着。
        stopSshRetry(tabId, "waiting-input");
        setSshRetries((current) => {
          const state = current[tabId];
          if (!state) return current;
          return { ...current, [tabId]: { ...state, phase: "waiting-input", attempts: attemptNo, nextAt: null, lastError: message } };
        });
        return;
      }
      scheduleSshRetry(tabId, message, attemptNo);
    }
  };
  /**
   * 重试用尽后用户在面板里敲了字：立刻再试一次（不等 10 秒），并重新开始数次数。
   * 这是用户要的「界面就这样，但我一敲键盘就重连」。
   */
  const resumeSshRetry = (tabId: string) => {
    const state = sshRetriesRef.current[tabId];
    if (!state || state.phase !== "waiting-input") return;
    sshRetryAttemptRef.current[tabId] = 0;
    void attemptSshRetry(tabId, 1);
    setSshRetries((current) => {
      const existing = current[tabId];
      if (!existing) return current;
      return {
        ...current,
        [tabId]: { ...existing, phase: "retrying", attempts: 0, nextAt: Date.now() + SSH_RETRY_INTERVAL_MS },
      };
    });
  };
  /** 手动重连当前激活页签（工具栏/右键菜单用），失败走该面板的提示。 */
  const handleReconnect = async () => {
    const tabId = activeTabIdRef.current;
    if (!desktop || !tabId) {
      notifyDesktopRequired();
      return;
    }
    stopSshRetry(tabId);
    try {
      await reconnectTab(tabId);
      showPaneNotice(tabId, "终端已重新连接。", "success", 2500);
    } catch (reason) {
      showPaneNotice(tabId, formatInvokeError(reason), "error");
    }
  };
  // 左栏「已保存服务器」与顶栏 git 分支：都来自真实配置/工作目录，进页面读一次，
  // 之后由左栏刷新按钮触发。git 分支需要先知道工作目录，所以串行。
  const refreshSavedServers = async (announce: boolean) => {
    setSavedServersLoading(true);
    try {
      const servers = await listSavedServers();
      setSavedServers(servers);
      if (announce) {
        showToast(
          servers.length ? `已读取 ${servers.length} 个已保存环境。` : "尚未保存 SSH / WSL 配置。",
          "info",
        );
      }
    } catch (reason) {
      showToast(formatInvokeError(reason), "error");
    } finally {
      setSavedServersLoading(false);
    }
  };
  useEffect(() => {
    void refreshSavedServers(false);
    // 先解析出绝对工作目录再读 git：后端把 "." 解析成本进程目录，
    // 顶栏胶囊和右栏文件面板都会显示成 "."。
    void (async () => {
      const snapshot = await readHostStatus(1);
      const stored = window.localStorage.getItem("android-tool.terminal-root") || "";
      const root = stored || snapshot?.workingDir || snapshot?.homeDir || "";
      if (root) setWorkspaceRoot(root);
      const summary = await readWorkspaceGit(root || ".");
      if (summary.branch) setGitBranch(summary.branch);
      if (summary.path) setWorkspaceRoot(summary.path);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const stops: Array<() => void> = [];
    const deliver = (event: TerminalOutputEvent) => {
      if (!event.data) return;
      const tabId = sessionTabsRef.current[event.sessionId];
      if (!tabId) {
        if (openingPtyRef.current)
          bufferEarlyTerminalOutput(pendingOutputRef.current, event.sessionId, event.data);
        return;
      }
      writeShellOutput(tabId, event.data);
    };
    void listen<TerminalOutputEvent>("terminal:output", (event) =>
      handoff.accept(event, deliver),
    ).then((stop) => {
      if (disposed) stop();
      else {
        stops.push(stop);
        void handoff
          .recover(deliver)
          .catch((error) => showToast(formatInvokeError(error), "error"));
      }
    });
    void listen<{
      sessionId: string;
      success: boolean;
      code?: number;
      reconnectable?: boolean;
    }>("terminal:exit", (event) => {
      const tabId = sessionTabsRef.current[event.sessionId];
      if (!tabId) return;
      // v3 §21.3：russh 桥的断连可重连——先在缓冲里留横幅（页签缓冲随页签保留），
      // 再释放实例；用户从左栏重新打开该页签即走既有门控重开新会话。
      if (event.reconnectable) {
        writeShellOutput(
          tabId,
          "\r\n\x1b[38;5;203m── SSH 连接已断开，输出已保留；正在自动重连 ──\x1b[0m\r\n",
        );
      }
      forgetTerminalSession(tabId);
      // 提示只贴在这块面板里（不再飘到全窗口）：用户可能同时开着别的终端，
      // 全局 toast 会让人误以为是旁边那块断了。
      if (event.success) {
        showPaneNotice(tabId, "终端会话已结束。", "info");
        return;
      }
      if (event.reconnectable) {
        // 自动重试：每 10s 一次、共 10 次；提示条由 sshRetries 驱动（带按钮）。
        startSshRetry(tabId, "SSH 连接已断开");
        return;
      }
      showPaneNotice(
        tabId,
        `终端会话已退出${event.code === undefined ? "" : `，代码 ${event.code}`}。`,
        "error",
      );
    }).then((stop) => {
      if (disposed) stop();
      else stops.push(stop);
    });
    return () => {
      disposed = true;
      stops.forEach((stop) => stop());
    };
  }, [desktop]);
  const closeTimerRef = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    disposedRef.current = false;
    clearTimeout(closeTimerRef.current);
    return () => {
      closeTimerRef.current = setTimeout(() => {
        disposedRef.current = true;
        if (inputFrameRef.current !== null) window.cancelAnimationFrame(inputFrameRef.current);
        // **不能在这里关会话**。切到别的工具页只是把视图卸载，PTY 里的命令还在跑
        // （`pageCache` 会在卸载时存快照、回来时按 journal 补投输出）。此前这里
        // 调 `close_terminal_session` 把会话全杀了，但快照里的 sessionId 又被原样
        // 恢复回来，于是回到终端页随便敲一下回车就报「终端会话不存在或已经关闭」。
        // 会话的生命周期只由「关页签 / 应用退出」决定。
        paneHandlesRef.current = {};
      }, 0);
    };
  }, []);
  /**
   * 命令块空闲判定驱动（开发方案 8.3.2 静默兜底）。
   * `tick` 是纯判定、不碰 DOM，所以这里用一个低频定时器扫所有会话就够了：
   * 只有"输出停了且尾部像提示符"的块会被收尾，其余原样返回。
   */
  useEffect(() => {
    const timer = window.setInterval(() => {
      for (const sessionId of Object.values(sessionsRef.current)) blockTracker.tick(sessionId);
    }, 250);
    return () => window.clearInterval(timer);
  }, [blockTracker]);
  const currentTab = tabs.find((t) => t.id === activeTabId) || null;
  const shellTag =
    currentTab?.type === "local"
      ? currentTab.shell === "cmd"
        ? "cmd"
        : "pwsh"
      : currentTab?.type === "wsl"
        ? "wsl"
        : currentTab?.type === "ssh"
          ? "ssh"
          : "未建立会话";
  const handleToggleDrawer = (pane: DrawerPane) => {
    setDrawerPane((current) => (current === pane ? null : pane));
  };
  const handleUseSavedServer = (server: SavedServerEntry) => {
    if (server.kind === "ssh") {
      void handleAddTab("ssh", server.name, undefined, server.id, server.root);
    } else {
      void handleAddTab("wsl", server.name, undefined, server.id, server.root);
    }
  };
  // ---- 左栏「已保存服务器」右键菜单（需求 ⑧）------------------------------
  /**
   * 修改服务器配置（需求 ⑧）：右键菜单里打开与设置页同一个编辑弹窗。
   *
   * 左栏的 `SavedServerEntry` 只有 id/name/detail/root 四个字段，缺 host/port/username，
   * 所以这里回查一次 `list_ssh_connections` 拿到完整配置再回填。查不到说明列表已过期，
   * 退回刷新提示，避免弹出一个字段全空的编辑框把用户现有配置覆盖掉。
   */
  const editSavedServer = async (server: SavedServerEntry) => {
    if (server.kind !== "ssh") {
      void renameSavedServer(server);
      return;
    }
    try {
      const all = await listSshConnections();
      const found = all.find((item) => item.id === server.id);
      if (!found) {
        showToast("这条连接已不存在，请刷新服务器列表。", "error");
        void refreshSavedServers(true);
        return;
      }
      setSshDialog({
        editing: {
          id: found.id,
          name: found.name,
          host: found.host,
          port: found.port,
          username: found.username,
          privateKeyPath: found.privateKeyPath,
          strictHostKey: found.strictHostKey,
          defaultRoot: found.defaultRoot,
        },
      });
    } catch (reason) {
      showToast(formatInvokeError(reason), "error");
    }
  };
  /** 重命名连接：名称是唯一可原地改的字段，其余参数保持原样回写。 */ const renameSavedServer =
    async (server: SavedServerEntry) => {
      const next = window.prompt(
        `重命名${server.kind === "ssh" ? " SSH " : " WSL "}连接`,
        server.name,
      );
      const trimmed = next?.trim();
      if (!trimmed || trimmed === server.name) return;
      try {
        if (server.kind === "ssh") {
          const rows = await invoke<Array<Record<string, unknown>>>("list_ssh_connections");
          const row = rows.find((item) => item.id === server.id);
          if (!row) {
            showToast("连接已不存在，请刷新列表。", "error");
            return;
          } // 整份配置回写：save_ssh_connection 是覆盖语义，漏字段等于把配置清空。
          await invoke("save_ssh_connection", { config: { ...row, name: trimmed } });
        } else {
          const rows = await invoke<Array<Record<string, unknown>>>("list_wsl_profiles");
          const row = rows.find((item) => item.id === server.id);
          if (!row) {
            showToast("配置已不存在，请刷新列表。", "error");
            return;
          }
          await invoke("save_wsl_profile", { profile: { ...row, name: trimmed } });
        }
        await refreshSavedServers(false);
        showToast("已重命名。", "success");
      } catch (reason) {
        showToast(formatInvokeError(reason), "error");
      }
    };
  /** 删除连接：不可撤销，走统一确认弹窗（先统计受影响的活跃终端数，v3 §21.4 S5c）。 */
  const deleteSavedServer = (server: SavedServerEntry) => {
    void (async () => {
    let affectedTerminals = "";
    if (server.kind === "ssh") {
      try {
        const active = await countActiveSshTerminals(server.id);
        if (active > 0) affectedTerminals = `该服务器有 ${active} 个活跃终端会话，删除后这些终端将断开（输出缓冲保留）。`;
      } catch { /* 计数失败不阻塞删除 */ }
    }
    confirmAction({
      title: "删除连接",
      message: `确认删除「${server.name}」？该连接配置会被移除。${affectedTerminals}`,
      confirmLabel: "确认删除",
      onConfirm: async () => {
        try {
          await invoke(server.kind === "ssh" ? "delete_ssh_connection" : "delete_wsl_profile", {
            id: server.id,
          });
          await refreshSavedServers(false);
          showToast("已删除。", "success");
        } catch (reason) {
          showToast(formatInvokeError(reason), "error");
        }
      },
    });
    })();
  };
  const handleServerContextMenu = (event: React.MouseEvent, server: SavedServerEntry) => {
    event.preventDefault();
    setServerMenu({
      x: event.clientX,
      y: event.clientY,
      trigger: event.currentTarget as HTMLElement,
      server,
    });
  };
  // ---- 中栏多面板（设计文档 §3.2 / §3.3）------------------------------------
  const panes = paneBindings.slice(0, paneCount(layout));
  const paneAt = (index: number) => {
    const tabId = panes[index] ?? null;
    return tabId ? (tabs.find((tab) => tab.id === tabId) ?? null) : null;
  };
  /**
   * AI 面板要展示的块：优先用户显式打开的那个，其次是"最近一个可解释的块"。
   * `latestBlockTick` 只作为失效信号，用于在块进入终态后重新计算一次。
   */
  const aiCandidateBlock = useMemo(() => {
    void latestBlockTick;
    if (blockAi) return blockTracker.blockStore().get(blockAi.blockId) ?? null;
    return blockTracker.latest(sessionsRef.current[activeTabIdRef.current] ?? "") ?? null;
    // blockAi 与页签切换都要重算；blockTracker 是稳定实例。
  }, [blockAi, latestBlockTick, blockTracker, activeTabId]);
  const aiEligibleBlock =
    aiCandidateBlock && isBlockAiEligible(aiCandidateBlock) && !blockAi
      ? aiCandidateBlock
      : null;
  /**
   * 失败提示条（F46）：命令以 failed / interrupted 收尾时，在终端下方亮一条
   * "要诊断吗"的提示。
   * 这里刻意**不自动打开 AI 面板**——失败命令往往连着好几条，自动弹面板会把
   * 终端顶掉、还要烧一次模型调用。提示条是廉价的：一条文字 + 两个按钮，用户
   * 点"诊断"才真的发请求。`aiFailureHint` 关掉后整条都不出现，但顶栏的手动
   * 入口不受影响（关的是"提示"，不是"功能"）。
   */
  const [dismissedHintBlockId, setDismissedHintBlockId] = useState<string | null>(null);
  const failureHintBlock =
    aiFailureHint &&
    aiEligibleBlock &&
    isFailableStatus(aiEligibleBlock.status) &&
    aiEligibleBlock.id !== dismissedHintBlockId
      ? aiEligibleBlock
      : null;
  /** 打开面板；同一块上重复点"诊断"用 epoch 触发重置。 */
  const openBlockAi = (mode: TerminalAiMode, blockId?: string) => {
    const target =
      blockId ?? blockTracker.latest(sessionsRef.current[activeTabIdRef.current] ?? "")?.id;
    if (!target) return;
    setComposerTabId(null);
    closeRepair();
    blockAiEpochRef.current += 1;
    setBlockAi({ blockId: target, mode, epoch: blockAiEpochRef.current });
  };
  // 切页签时收起两个面板：面板绑定的是某个 session 里的状态，跨页签展示会张冠李戴。
  const aiPanelTabRef = useRef(activeTabId);
  useEffect(() => {
    if (aiPanelTabRef.current === activeTabId) return;
    aiPanelTabRef.current = activeTabId;
    setBlockAi(null);
    setComposerTabId(null);
  }, [
    activeTabId,
  ]);
  /**
   * 建议命令一键插入（F44）：只写进输入队列，**不追加回车**，
   * 用户必须自己确认后再按 Enter——诊断结果不能直接变成执行。
   */
  const insertSuggestedCommand = (command: string) => {
    const sessionId = sessionsRef.current[activeTabIdRef.current];
    if (!sessionId) return;
    queueTerminalInput(sessionId, command);
    paneHandlesRef.current[activeTabIdRef.current]?.focus();
    showToast("命令已插入输入框，请确认后按 Enter 执行", "info");
  };
  // ── 自动修复闭环（9.4 / T4-7）─────────────────────────────────────────────
  /** 收卡片：只中断还在路上的 AI 请求，已经写进 PTY 的命令不追回。 */ const closeRepair = () => {
    repairAbortRef.current?.abort();
    repairAbortRef.current = null;
    if (repairTimerRef.current !== null) window.clearTimeout(repairTimerRef.current);
    repairTimerRef.current = null;
    repairWaitingRef.current = null;
    setRepairBusy(false);
    setRepair(null);
    setRepairBlock(null);
  };
  /**
   * 轮询等修复命令的结果。
   * 不用 `onChange` 回调：`BlockTracker` 的回调在构造时固定，拿不到这里的状态。
   * 500 ms 一次、最多 2 分钟，命中"命令一致 + 终态"才结算，避免把用户在等待期间
   * 随手敲的别的命令当成这次修复的结果。
   */
  const watchRepairResult = (command: string) => {
    const sessionId = sessionsRef.current[activeTabIdRef.current];
    if (!sessionId) return;
    const startedAt = Date.now();
    const finish = (ok: boolean) => {
      if (repairTimerRef.current !== null) window.clearTimeout(repairTimerRef.current);
      repairTimerRef.current = null;
      repairWaitingRef.current = null;
      setRepairBusy(false);
      setRepair((prev) => (prev ? repairReducer(prev, { type: "result", ok }) : prev));
      showToast(ok ? "修复命令执行成功。" : "修复命令仍未成功。", ok ? "success" : "error");
    };
    const tick = () => {
      if (repairWaitingRef.current !== command) return;
      const latest = blockTracker.latest(sessionId);
      const matched =
        latest && latest.command.trim() === command.trim() && isTerminalStatus(latest.status);
      if (matched) {
        finish(latest.status === "done");
        return;
      }
      if (Date.now() - startedAt > 120000) {
        finish(false);
        return;
      }
      repairTimerRef.current = window.setTimeout(tick, 500);
    };
    tick();
  };
  /**
   * 开始一轮修复：诊断文本 → 一条修复命令 → 停下来等审批。
   * 只有用户点了"批准执行"才会写 PTY；生成阶段失败只改卡片文案，不弹窗。
   */
  const startRepair = (block: CommandBlock, diagnosis: string) => {
    if (repair && !canStartRepair(repair)) {
      showToast(repair.message ?? "这一轮修复已经结束，请重新诊断。", "info");
      return;
    } // 上一轮失败的历史要带上，否则模型会反复给同一条错命令。
    const carried =
      repair && !isTerminalPhase(repair.phase) ? repair.rounds : (repair?.rounds ?? []);
    setBlockAi(null);
    setComposerTabId(null);
    setRepairBlock(block);
    setRepair({ ...repairReducer(initialRepairState(), { type: "start" }), rounds: carried });
    setRepairBusy(true);
    const controller = new AbortController();
    repairAbortRef.current = controller;
    void (async () => {
      const previous = carried.map((round) => ({
        command: round.command,
        outcome: (round.outcome === "failed" ? "failed" : "cancelled") as "failed" | "cancelled",
      }));
      const outcome = await proposeRepair(block, terminalAiDeps(), {
        diagnosis,
        attempt: carried.length + 1,
        previous,
        signal: controller.signal,
      });
      if (controller.signal.aborted) return;
      repairAbortRef.current = null;
      setRepairBusy(false);
      if (!outcome.ok) {
        setRepair((prev) =>
          prev ? repairReducer(prev, { type: "proposeFailed", message: outcome.message }) : prev,
        );
        return;
      }
      if (outcome.proposal.dropped.length > 0) {
        showToast(
          `模型给了 ${outcome.proposal.dropped.length + 1} 条命令，本次只采用第一条。`,
          "info",
        );
      }
      setRepair((prev) =>
        prev
          ? repairReducer(prev, {
              type: "proposed",
              command: outcome.proposal.command,
              explanation: outcome.proposal.explanation,
            })
          : prev,
      );
    })();
  };
  /** 批准：走 AI 专用写入通道（后端再做一次闸门），随后等这一条的 block 结果。 */
  const approveRepair = () => {
    if (!repair) return;
    if (!repairExecutable(repair)) {
      showToast(`这是破坏性命令，请先手打 ${repairConfirmToken(repair)} 确认。`, "error");
      return;
    }
    const command = repair.pending?.command;
    if (!command) return;
    setRepair(repairReducer(repair, { type: "approve", at: Date.now() }));
    setRepairBusy(true);
    repairWaitingRef.current = command;
    executeComposedCommand(command);
    watchRepairResult(command);
  };
  /**
   * 输入侧 AI（F51 / T4-1）：`Ctrl+Enter` 或顶栏 Wand 按钮打开。
   * 打开前先收起输出侧面板——两个面板都停靠在终端下方，同开会把画布挤没；
   * 而且在同一个页签上"解释上一条"和"写新命令"本来就互斥。
   */
  const openComposer = () => {
    if (!sessionsRef.current[activeTabIdRef.current]) return;
    setBlockAi(null);
    closeRepair();
    // 手动入口不带预填：先放弃上一次从底部栏带进来的文本，否则会串。
    setComposerNl(null);
    setComposerTabId(activeTabId);
  };
  const closeComposer = useCallback(() => {
    setComposerTabId(null);
    setComposerNl(null);
    paneHandlesRef.current[activeTabIdRef.current]?.focus();
  }, []);
  /**
   * 底部指令栏的 AI 行：把自然语言带进输入侧面板并立刻生成草稿。
   * 与 `openComposer` 的区别只在于带 `nl`；仍然走同一套互斥与页签校验，
   * 不另开一条"直接调用模型"的旁路——草稿确认与风险闸门必须照常生效。
   */
  const openComposerWithNl = useCallback((nl: string) => {
    const tabId = activeTabIdRef.current;
    if (!sessionsRef.current[tabId]) return;
    setBlockAi(null);
    closeRepair();
    setComposerNl(nl);
    setComposerTabId(tabId);
  }, []);
  /** 提示词里带的命令历史：最近 20 条块命令，最新在前。 */
  const composerHistory = useMemo(() => {
    if (!composerTabId) return [];
    const sessionId = sessionsRef.current[composerTabId];
    if (!sessionId) return [];
    return blockTracker
      .blockStore()
      .blocksFor(sessionId)
      .slice(-20)
      .map((item) => item.command)
      .filter((item) => item.trim().length > 0)
      .reverse();
  }, [
    composerTabId,
    blockTracker,
    latestBlockTick,
  ]);
  /**
   * 执行 AI 草稿：走**专用通道** `write_ai_terminal_input`（T4-6 / 10.1 第三层）。
   * 刻意不复用 `queueTerminalInput`：
   * - 那个通道按帧切片写字符，后端看到的永远是不完整的半行，破坏性检查形同虚设；
   * - AI 命令必须整条一次写入，后端才能对完整命令行做一次闸门判定。
   * `approved: true` 是"UI 侧确认已通过"的显式声明（`executeGate` 已卡住风险），
   * 后端仍会独立再校验一次，任一侧判定为破坏性都会被拒。
   */
  const executeComposedCommand = (command: string) => {
    const tabId = activeTabIdRef.current;
    const sessionId = sessionsRef.current[tabId];
    if (!sessionId) return;
    // 块追踪也要看到这一行，否则 AI 执行的命令不会生成 block（8.4）。
    blockTracker.feedInput(sessionId, `${command}\r`);
    const prior = inputWritesRef.current[sessionId] || Promise.resolve();
    const write = prior
      .catch(() => undefined)
      .then(() =>
        invoke<void>("write_ai_terminal_input", {
          sessionId,
          input: `${command}\r`,
          approved: true,
        }),
      );
    inputWritesRef.current[sessionId] = write.catch((reason) => {
      showToast(formatInvokeError(reason), "error");
    });
    paneHandlesRef.current[tabId]?.focus();
  };
  /** 多命令"全部插入"：整段塞进输入队列不回车，逐条仍由用户按 Enter 确认。 */
  const insertComposedCommands = (multiline: string) => {
    const sessionId = sessionsRef.current[activeTabIdRef.current];
    if (!sessionId) return;
    queueTerminalInput(sessionId, multiline);
    paneHandlesRef.current[activeTabIdRef.current]?.focus();
    showToast("剩余命令已插入输入框，请逐条确认后执行", "info");
  };
  /** 只在焦点面板渲染的附加条：常用命令（设计文档 §3.6）。 */
  const paneOverlay = (
    <div className="terminal-quick-commands" aria-label="常用命令">
      {" "}
      <span>常用命令</span>{" "}
      {quickCommands.map((item) => (
        <button
          key={item.command}
          type="button"
          onClick={() => handleQuickCmdClick(item.command)}
          title={item.command}
        >
          {" "}
          {item.label}{" "}
        </button>
      ))}{" "}
    </div>
  );
  /**
   * 每块面板自己的浮层：焦点面板的常用命令条、以及**该面板自己的**提示条
   * 与断线重试条。提示按 tabId 归属，所以哪块断了就只有哪块显示——
   * 这正是用户要的「第一个面板是 ssh，就只在这个 ssh 终端里提示」。
   */
  const paneOverlayFor = (index: number, tabId: string | null) => {
    const notice = tabId ? paneNotices[tabId] ?? null : null;
    const retry = tabId ? sshRetries[tabId] ?? null : null;
    if (!notice && !retry && focusedPane !== index) return undefined;
    const retryInSeconds =
      retry && retry.nextAt ? Math.max(0, Math.ceil((retry.nextAt - Date.now()) / 1000)) : undefined;
    return (
      <>
        {focusedPane === index && tabId ? paneOverlay : null}
        {tabId ? (
          <PaneBanner
            notice={notice}
            retry={retry}
            retryInSeconds={retryInSeconds}
            onDismissNotice={() => dismissPaneNotice(tabId)}
            onRetryNow={() => {
              stopSshRetry(tabId);
              void attemptSshRetry(tabId, 1);
            }}
            onDismissRetry={() => stopSshRetry(tabId)}
          />
        ) : null}
      </>
    );
  };
  const layoutOptions = [
    { value: "1x1" as TerminalLayout, label: <Square size={13} />, title: "1x1 布局" },
    { value: "1x2" as TerminalLayout, label: <Columns2 size={13} />, title: "1x2 布局（左右）" },
    { value: "2v" as TerminalLayout, label: <Rows2 size={13} />, title: "上下两块" },
    { value: "2x2" as TerminalLayout, label: <Grid2x2 size={13} />, title: "2x2 布局" },
  ];
  /** 布局按钮激活态随实际占用的面板数同步（覆盖后自动切换；3 块等不规则数全不亮）。 */
  const occupiedSlots = paneBindings.reduce<number[]>((acc, id, index) => (id ? [...acc, index] : acc), []);
  const occupiedPanes = occupiedSlots.length;
  // 收敛的前提是「占用的槽位恰好是前 N 个」：若占用散落在后面（例如 2x2 里只用了
  // 第 1、4 格），按占用数收敛会把后面的终端藏起来，所以那种情况保持原布局不收敛。
  const compactPanes = occupiedSlots.every((slot, index) => slot === index);
  /**
   * 「显式开空槽」是一次性意图，不是长期开关。
   *
   * `handleLayoutChange` 点任意非 1x1 布局都会把 `showEmptySlots` 置真，它还会随窗口
   * 状态持久化、跨重启恢复；而它一旦为真，`visibleLayout` 就不再收敛。于是很久以前
   * 点过的 2x2 会一直留着空槽——用户把终端关/解绑到只剩 2 个，网格仍画 4 格、下面
   * 两格是「空闲面板」，正是 m18580 说的「只激活两个面板，却显示了 4 个」。
   * 只收「变少」：用户正在往空槽里塞终端（占用数变多）时要保住这个意图，否则刚点的
   * 2x2 会在拖入一个终端后立刻缩成 1x2；而关掉/解绑终端（变少）才该回到收敛。
   */
  const occupiedCountRef = useRef(occupiedPanes);
  useEffect(() => {
    const previous = occupiedCountRef.current;
    occupiedCountRef.current = occupiedPanes;
    if (dropsEmptySlotIntent(previous, occupiedPanes)) setShowEmptySlots(false);
  }, [occupiedPanes]);
  /** 真正画出来的格数（m18580：只绑 2 个终端就不要画 4 格占位板）。 */
  const renderedLayout = visibleLayout(layout, occupiedPanes, showEmptySlots || !compactPanes);
  const activeLayoutKey = (() => {
    // 没有终端时也照常亮：点了 2x2 就该看到 2x2 亮着，否则四个按钮全灰、用户
    // 不知道自己刚才点的到底生效了没有（点了 2x2 却全是灰的，很容易以为没点上）。
    if (occupiedPanes === 0) return layout;
    // 覆盖态（比例极值）：被隐藏格的绑定还在，但视觉已是单格独占，亮 1x1。
    const collapsed =
      colRatios.some((value) => value >= 0.99 || value <= 0.01) || rowRatios[0] >= 0.99 || rowRatios[0] <= 0.01;
    if (occupiedPanes === 2 && collapsed) return "1x1";
    if (occupiedPanes === 4) return "2x2";
    if (!compactPanes) return occupiedPanes === 1 && !showEmptySlots ? "1x1" : "";
    // 收敛后的格数就是当前真实形态，按钮应与网格一致（原来会亮 1x2 却画 2x2）。
    return renderedLayout;
  })();
  return (
    <div className="terminal-workspace">
      {" "}
      {confirmationDialog}{" "}
      {terminalMenu && (
        <ContextMenu
          x={terminalMenu.x}
          y={terminalMenu.y}
          label="终端操作"
          trigger={terminalMenu.trigger}
          onClose={() => setTerminalMenu(null)}
          items={[
            {
              label: "复制选中内容",
              disabled: !paneHandlesRef.current[activeTabIdRef.current]?.hasSelection(),
              onSelect: () => void copyTerminalSelection(),
            },
            { label: "粘贴到终端", onSelect: () => void pasteTerminalClipboard() },
            {
              label: "全选",
              onSelect: () => paneHandlesRef.current[activeTabIdRef.current]?.selectAll(),
            },
            {
              label: "清屏",
              onSelect: () =>
                confirmAction({
                  title: "清空终端输出",
                  message: "确认清空当前终端的输出记录？不会停止命令或删除设备文件。",
                  confirmLabel: "确认清空",
                  onConfirm: handleClear,
                }),
            },
          ]}
        />
      )}{" "}
      {serverMenu && (
        <ContextMenu
          x={serverMenu.x}
          y={serverMenu.y}
          label="服务器操作"
          trigger={serverMenu.trigger}
          onClose={() => setServerMenu(null)}
          items={[
            { label: "在新终端打开", onSelect: () => handleUseSavedServer(serverMenu.server) },
            {
              label: "复制地址",
              onSelect: () =>
                void navigator.clipboard
                  .writeText(serverMenu.server.detail)
                  .then(() => showToast("已复制连接地址。", "success"))
                  .catch(() => showToast("复制失败。", "error")),
            },
            { label: "修改服务器配置…", onSelect: () => void editSavedServer(serverMenu.server) },
            { label: "删除连接", onSelect: () => deleteSavedServer(serverMenu.server) },
          ]}
        />
      )}{" "}
      {sidebarMenu && (
        <ContextMenu
          x={sidebarMenu.x}
          y={sidebarMenu.y}
          label="终端页签"
          trigger={sidebarMenu.trigger}
          onClose={() => setSidebarMenu(null)}
          items={[
            { label: "切换到该终端", onSelect: () => handleSelectTab(sidebarMenu.tabId) },
            {
              label: "放到第 1 块面板",
              onSelect: () => handleDropTab(0, sidebarMenu.tabId, 0.5, 0.5),
            },
            {
              label: "放到第 2 块面板",
              disabled: paneCount(layout) < 2,
              onSelect: () => handleDropTab(1, sidebarMenu.tabId, 0.5, 0.5),
            },
            {
              label: "从面板移除",
              disabled: !paneBindings.includes(sidebarMenu.tabId),
              onSelect: () => handleDetachPane(sidebarMenu.tabId),
            },
            { label: "重命名…", onSelect: () => void renameTab(sidebarMenu.tabId) },
            { label: "关闭终端", onSelect: () => void closeTabById(sidebarMenu.tabId) },
          ]}
        />
      )}{" "}
      {/* 顶部头（terminal-topbar）已按需求删除：菜单栏与终端之间不再有独立头部，
          返回 Home 由左侧导航栏承担；终端自身的标题/布局条保留在 panel-header。 */}
      <div
        className="terminal-workspace-body"
        ref={workspaceBodyRef}
        style={{ "--terminal-drawer-width": `${drawerWidth.width}px` } as React.CSSProperties}
      >
        {" "}
        {/* 左栏：新建终端 / 已打开终端 / 已保存服务器 */}{" "}
        {!sidebarCollapsed && (
        <TerminalSidebar
          tabs={tabs}
          activeTabId={activeTabId}
          savedServers={savedServers}
          savedServersLoading={savedServersLoading}
          onSelectTab={handleSelectTab}
          onCloseTab={handleCloseTab}
          onAddLocal={(shell) =>
            void handleAddTab(
              "local",
              shell === "cmd" ? "命令提示符" : "PowerShell",
              shell,
              undefined,
              workspaceRoot,
            )
          }
          onAddWsl={() => void handleAddTab("wsl", "WSL", undefined, undefined, undefined)}
          onAddSsh={() => setSshDialog({})}
          onUseSavedServer={handleUseSavedServer}
          onDropTabAt={handleDropTab}
          onDropSavedServerAt={handleDropSavedServer}
          onRefreshServers={() => void refreshSavedServers(true)}
          onTabContextMenu={handleTabContextMenu}
          onServerContextMenu={handleServerContextMenu}
        />
        )}{" "}
        {/* 中栏：终端内容 */}{" "}
        <section className="terminal-center">
          <div
            className="terminal-panel-card"
            onKeyDown={(event) => {
              // 全局监听会抢走别处输入框的 Ctrl+Enter；关闭统一走 Esc。
              if (!event.ctrlKey || event.key !== "Enter") return;
              if (composerTabRef.current === activeTabId) return;
              event.preventDefault();
              openComposer();
            }}
          >
            {" "}
            <div className="terminal-panel-header">
              {" "}
              <div className="terminal-header-leading">
                {" "}
                <Button
                  type="button"
                  variant="surface"
                  size="sm"
                  className="terminal-icon-button terminal-sidebar-toggle"
                  title={sidebarCollapsed ? "展开终端列表" : "收起终端列表"}
                  aria-label={sidebarCollapsed ? "展开终端列表" : "收起终端列表"}
                  aria-expanded={!sidebarCollapsed}
                  aria-controls="terminal-sidebar"
                  icon={<PanelLeft size={14} strokeWidth={1.7} />}
                  onClick={() => setSidebarCollapsed((current) => !current)}
                />{" "}
                <div className="terminal-header-title">
                  {" "}
                  <TerminalIcon size={14} /> <span>{currentTab?.title || "终端"}</span>{" "}
                </div>{" "}
              </div>{" "}
              {/* 布局切换（1x1 / 1x2 / 2x2）：面板数是布局决定的，改布局只裁/补绑定。 */}{" "}
              <div className="terminal-layout-switch" role="group" aria-label="面板布局">
                {" "}
                {layoutOptions.map((option) => (
                  <button
                    key={option.value}
                    type="button"
                    aria-pressed={activeLayoutKey === option.value}
                    data-active={activeLayoutKey === option.value ? "true" : "false"}
                    onClick={() => handleLayoutChange(option.value)}
                    title={option.title}
                  >
                    {" "}
                    {option.label}{" "}
                  </button>
                ))}{" "}
              </div>{" "}
            </div>{" "}
            {/* 设备切换条与常用命令已下沉到 `paneOverlay`（只渲染在焦点面板上），                这里不再重复渲染，否则会出现两条一模一样的设备栏。 */}{" "}
            {/* 中栏多面板：每个面板各自持有一份内核实例（设计文档 §3.2 / §3.3）。                父级不再直接渲染终端宿主，只按 tabId 把输出投给对应面板。 */}{" "}
            <div ref={paneGridRef} className={`terminal-pane-grid layout-${renderedLayout}`}>
              {" "}
              {Array.from({ length: renderedLayout === "2x2" || renderedLayout === "2v" ? 2 : 1 }, (_, rowIndex) => {
                const perRow = renderedLayout === "1x1" || renderedLayout === "2v" ? 1 : 2;
                return (
                  <React.Fragment key={`pane-row-${rowIndex}`}>
                    <div
                      className="terminal-pane-row"
                      style={renderedLayout === "2x2" || renderedLayout === "2v" ? { flex: `${rowIndex === 0 ? (rowRatios[0] ?? 0.5) : 1 - (rowRatios[0] ?? 0.5)} 1 0%` } : undefined}
                    >
                    {Array.from({ length: perRow }, (_, colIndex) => {
                    const index = rowIndex * perRow + colIndex;
                    const tabId = paneBindings[index] ?? null;
                    // 自动路径（抢占收敛/关页签前移）：行内已有终端时空槽不渲染，
                    // 幸存面板铺满整行（§20.1 用户定稿）。显式点布局按钮展开的
                    // 布局（showEmptySlots）则渲染占位板，方便拖入/新建终端。
                    const rowOccupied = Array.from({ length: perRow }, (_, c) => paneBindings[rowIndex * perRow + c]).some(Boolean);
                    if (rowOccupied && !tabId && !showEmptySlots) return null;
                    const paneTab = paneAt(index);
                    const renderedInRow = Array.from({ length: perRow }, (_, c) => {
                        const occupied = Boolean(paneBindings[rowIndex * perRow + c]);
                        return occupied || (rowOccupied && showEmptySlots);
                    }).filter(Boolean).length;
                    const rowRatio0 = colRatios[rowIndex] ?? 0.5;
                    // 单列（无邻居）必须撑满：grow 0.5 的唯一单元只会拿到一半空间。
                    const paneFlex = renderedInRow < 2 ? '1 1 0%' : colIndex === 0 ? `${rowRatio0} 1 0%` : `${1 - rowRatio0} 1 0%`;
                    // 覆盖态（比例极值 0/1）：0 宽格隐藏内容与焦点描边（否则描边
                    // 显成一条蓝线=「多了个蓝色」），拖动条保留可回拖恢复。
                    const rowCollapsed =
                      (renderedLayout === "2v" || renderedLayout === "2x2") && (rowRatios[0] >= 0.99 || rowRatios[0] <= 0.01);
                    const colCollapsed =
                      (renderedLayout === "1x2" || renderedLayout === "2x2") && (rowRatio0 >= 0.99 || rowRatio0 <= 0.01);
                    const slotCollapsed =
                      (colCollapsed && renderedInRow === 2 && ((colIndex === 0 && rowRatio0 <= 0.01) || (colIndex === 1 && rowRatio0 >= 0.99))) ||
                      // 纵向覆盖整行压扁（2v 每行只有 1 格，不能要求 renderedInRow===2）。
                      (rowCollapsed && ((rowIndex === 0 && rowRatios[0] <= 0.01) || (rowIndex === 1 && rowRatios[0] >= 0.99)));
                    return (
                  <React.Fragment key={`pane-slot-${index}`}>
                    <div className={`terminal-pane-cell${slotCollapsed ? " is-collapsed" : ""}`} style={{ flex: paneFlex }}>
                  <TerminalPane
                    key={`${index}-${tabId ?? "empty"}`}
                    tab={paneTab}
                    focused={focusedPane === index}
                    paneTotal={panes.length}
                    engineEpoch={engineEpoch}
                    getReplayText={(id) => readPaneReplayText(outputBuffers.current, id)}
                    getSessionId={(id) => sessionsRef.current[id]}
                    onInput={handlePaneInput}
                    onInteractiveChange={handlePaneInteractiveChange}
                    onPwdChange={handlePanePwdChange}
                    onResize={handlePaneResize}
                    onFocus={(id) => {
                      setFocusedPane(index);
                      handleSelectTab(id);
                    }}
                    onContextMenu={handlePaneContextMenu}
                    onEngineSwitchRequest={() => setEngineEpoch((value) => value + 1)}
                    onEngine={setTerminalEngine}
                    onRegister={handlePaneRegister}
                    overlay={paneOverlayFor(index, tabId)}
                    emptyHint="点左栏里已打开的终端占用这个面板"
                    emptyContent={
                      <EmptyPanePicker
                        onCreate={(type) => {
                          if (type === "ssh") {
                            setSshDialog({});
                            return;
                          }
                          if (type === "wsl") {
                            void handleAddTab("wsl", "WSL", undefined, undefined, undefined);
                            return;
                          }
                          void handleAddTab(
                            "local",
                            "PowerShell",
                            "pwsh",
                            undefined,
                            workspaceRoot,
                          );
                        }}
                      />
                    }
                    onDetach={handleDetachPane}
                    onDropTab={handleDropTab}
                    onDropSavedServer={handleDropSavedServer}
                    slot={index}
                  />
                    {perRow === 2 && renderedInRow === 2 && (
                      <div
                        className="terminal-pane-edge is-col"
                        style={colIndex === 0 ? { right: 0 } : { left: 0 }}
                        onPointerDown={(event) => beginEdgeResize(event, "col", rowIndex)}
                      />
                    )}
                    {(renderedLayout === "2x2" || renderedLayout === "2v") && (
                      <div
                        className="terminal-pane-edge is-row"
                        style={rowIndex === 0 ? { bottom: 0 } : { top: 0 }}
                        onPointerDown={(event) => beginEdgeResize(event, "row", rowIndex)}
                      />
                    )}
                    </div>
                  </React.Fragment>
                    );
                    })}
                    </div>
                  </React.Fragment>
                );
              })}{" "}
            </div>{" "}
            {/* 失败提示条（F46）：默认开、可全局关闭。                刻意只提示不自动请求——点"诊断"才发模型调用。 */}{" "}
            {failureHintBlock && (
              <div className="block-ai-hint" role="status">
                {" "}
                <AlertTriangle size={13} />{" "}
                <span className="block-ai-hint-text">
                  {" "}
                  命令失败（exit {failureHintBlock.exitCode ?? "?"}），要看看可能的原因吗？{" "}
                </span>{" "}
                <code className="block-ai-hint-cmd" title={failureHintBlock.command}>
                  {" "}
                  {failureHintBlock.command.length > 32
                    ? `${failureHintBlock.command.slice(0, 31)}…`
                    : failureHintBlock.command}{" "}
                </code>{" "}
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={() => openBlockAi("diagnose", failureHintBlock.id)}
                >
                  {" "}
                  诊断{" "}
                </Button>{" "}
                <Button
                  variant="bare"
                  size="xs"
                  icon={<X size={12} />}
                  aria-label="忽略这条提示"
                  title="忽略"
                  onClick={() => setDismissedHintBlockId(failureHintBlock.id)}
                />{" "}
              </div>
            )}{" "}
            {/* 输入侧 AI 面板（开发方案 9.3 / T4-1）：与 BlockAiPanel 互斥。                key 绑页签，切页签即整块重建，避免把上一个会话的草稿带过来。 */}{" "}
            {composerTabId && composerTabId === activeTabId && (
              <CommandComposer
                key={composerTabId}
                target={
                  currentTab?.type === "ssh"
                    ? "ssh"
                    : currentTab?.type === "wsl"
                      ? "wsl"
                      : "local"
                }
                cwd={currentTab?.root}
                history={composerHistory}
                passthroughHint={
                  passthroughTabs[composerTabId]
                    ? composerBlockedReason(true, "全屏程序正在运行")
                    : null
                }
                initialNl={composerNl ?? undefined}
                onExecute={executeComposedCommand}
                onInsertOnly={insertSuggestedCommand}
                onInsertAll={insertComposedCommands}
                onClose={closeComposer}
              />
            )}{" "}
            {/* 命令块 AI 面板（开发方案 9.2.4 / T3-4）：底部停靠，一次只开一个块。                终端输出是内核 canvas，卡片插不到输出行之间，只能停在这里。                key 带 epoch：同一个块上重复点"诊断"也能重新发起请求。 */}{" "}
            {blockAi && aiCandidateBlock && (
              <BlockAiPanel
                key={`${blockAi.blockId}:${blockAi.epoch}`}
                block={aiCandidateBlock}
                initialMode={blockAi.mode}
                onClose={() => setBlockAi(null)}
                onInsertCommand={insertSuggestedCommand}
                onRepairRequest={(diagnosis) => {
                  if (aiCandidateBlock) startRepair(aiCandidateBlock, diagnosis);
                }}
              />
            )}{" "}
            {/* 修复审批卡片（开发方案 9.4 / T4-7）：诊断 → 生成 → 审批 → 执行。                「批准执行」是唯一出口，且 destructive 还要手打确认。 */}{" "}
            {repair && repairBlock && (
              <RepairCard
                state={repair}
                blockCommand={repairBlock.command}
                busy={repairBusy}
                onTyped={(value) =>
                  setRepair((prev) => (prev ? repairReducer(prev, { type: "typed", value }) : prev))
                }
                onApprove={approveRepair}
                onReject={() =>
                  setRepair((prev) => (prev ? repairReducer(prev, { type: "reject" }) : prev))
                }
                onAbort={() => {
                  repairAbortRef.current?.abort();
                  repairAbortRef.current = null;
                  setRepair((prev) => (prev ? repairReducer(prev, { type: "abort" }) : prev));
                }}
                onClose={closeRepair}
              />
            )}{" "}
            {/* 底部指令发送栏（设计稿 `.terminal-card-footer`）：常驻，不随面板开合消失。                AI 行只转交自然语言给输入侧面板，不直接执行——草稿确认与风险闸门必须照常生效。                CLI 行沿用 `executeCommand`，会话未就绪时由它统一 toast 提示。 */}{" "}
            <TerminalCommandBar
              target={
                currentTab?.type === "ssh"
                  ? "ssh"
                  : currentTab?.type === "wsl"
                    ? "wsl"
                    : "local"
              }
              aiBlockedReason={
                activeTabId && passthroughTabs[activeTabId]
                  ? composerBlockedReason(true, "全屏程序正在运行")
                  : null
              }
              onSubmitCommand={(command) => {
                executeCommand(command);
                paneHandlesRef.current[activeTabIdRef.current]?.focus();
              }}
              onAskAi={openComposerWithNl}
            />{" "}
          </div>{" "}
        </section>{" "}
        {/* 右栏：主机状态 / Git / 文件，加一条竖排入口 */}{" "}
        {drawerPane && (
          <>
            <PaneResizeHandle
              className="terminal-drawer-resizer"
              side="right"
              label="调整右侧面板宽度"
              controls="terminal-drawer-panel"
              width={drawerWidth.width}
              min={drawerWidth.bounds.min}
              max={drawerWidth.bounds.max}
              initial={TERMINAL_DRAWER_PANE.initial}
              onResize={drawerWidth.resize}
              onCommit={drawerWidth.commit}
            />
            <TerminalDrawer
              pane={drawerPane}
              activeTab={currentTab}
              fallbackRoot={workspaceRoot}
              sshConnectionId={currentTab?.type === "ssh" ? currentTab.profileId : undefined}
              onClose={() => setDrawerPane(null)}
            />
          </>
        )}{" "}
        <TerminalDrawerRail pane={drawerPane} onToggle={handleToggleDrawer} />{" "}
      </div>{" "}
      {/* 新建 SSH 连接（左栏菜单「SSH 远程服务器」）：与设置页同一个弹窗与字段。          保存成功后直接按新连接开一个终端，用户不用再去设置页配一遍。 */}{" "}
      {sshDialog && (
        <NewSshDialog
          editing={sshDialog.editing}
          onClose={() => setSshDialog(null)}
          onError={(message) => showToast(message, "error")}
          onNotice={(message) => showToast(message, "success")}
          onCreated={(connection) => {
            setSshDialog(null);
            void handleAddTab(
              "ssh",
              connection.name,
              undefined,
              connection.id,
              connection.defaultRoot || undefined,
            );
            void refreshSavedServers(false);
          }}
        />
      )}{" "}
    </div>
  );
};
