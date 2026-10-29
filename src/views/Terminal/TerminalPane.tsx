/**
 * 单个终端面板：负责一份终端实例的生命周期，以及本面板的输出分发。
 *
 * 抽出来的原因（`docs/终端工作台v2-设计文档.md` §3.5）：中栏要支持 1x1 / 1x2 / 2x2，
 * 同一时刻可能有多份终端实例。原来整个视图只有一个 `xtermRef` + 一个输出队列，
 * 多面板无法共用；改成每个面板各自持有一份 lease、自己的 rAF 泵与自己的帧预算，
 * 父级只按 tabId 把输出投递给对应面板。
 *
 * 与实例池（`src/platform/terminalPool.ts`）的契约：面板卸载时 `lease.release()`
 * 把实例寄存进池（暂停渲染 + 交出 WebGL + 摘出 DOM），不销毁——自研内核的 WASM
 * 内存不可回收，四屏来回切换必须命中池子。
 */
import React, { useEffect, useRef } from "react";
import { Terminal as TerminalIcon, X } from "lucide-react";
import { getAppMonoFontStack } from "@/platform/font";
import { subscribeAppTheme } from "@/platform/theme";
import {
  applyTerminalSettings,
  readTerminalSettings,
  subscribeTerminalSettings,
  terminalOptions,
  type TerminalSettings,
} from "@/platform/terminalSettings";
import {
  bindTerminalPageVisibility,
  clearTerminalHost,
  focusTerminalInput,
  isFollowingLatestLine,
  refreshTerminalRenderer,
  resolveEngineAppearance,
  type AppFitAddon,
  type AppTerminal,
  type TerminalEngine,
} from "@/platform/terminalAdapter";
import { acquireTerminal, type TerminalLease } from "@/platform/terminalPool";
import { Button } from "@/components/ui";
import { decidePassthrough, modeProbe } from "@/platform/terminal/blocks/promptDetect";
import { clearTerminalContent, resetTerminalContent } from "@/platform/terminalReset";
import { scrollbackLinesForSession } from "@/platform/scrollbackBudget";
import { startPointerDrag } from "./pointerDrag";
import { mountTerminalScrollbar } from "./terminalScrollbar";
import { fitTerminalGrid, watchTerminalFont } from "@/platform/terminalFit";
import { bindTerminalClipboard } from "@/platform/terminalClipboard";
import { sanitizeTerminalReplay } from "@/platform/terminalReplay";
import {
  appendTerminalText,
  clearTerminalTextBuffer,
  readTerminalText,
  takeTerminalText,
  type TerminalTextBuffer,
} from "@/platform/terminalBuffer";
import type { TerminalTab } from "./types";

/** 单帧每面板的输出字符预算；非焦点面板再除以 4（设计文档 §3.4）。 */
const OUTPUT_CHARS_PER_FRAME = 48 * 1024;
const UNFOCUSED_DIVISOR = 4;
/** 面板输出队列上限，超出的从最旧处丢弃（与原来单面板视图一致）。 */
const QUEUE_LIMIT = 240_000;

export interface TerminalPaneHandle {
  /** 追加输出（父级按 tabId 投递）。 */
  write: (data: string) => void;
  /** 立即重排一次并返回当前行列数。 */
  fit: () => { cols: number; rows: number } | null;
  focus: () => void;
  /** 清屏（保留实例与会话）。 */
  clear: () => void;
  /** 软重置后重放该页签的回滚缓冲（换绑页签 / 重连时用）。 */
  replay: () => void;
  /** 以下四项给右键菜单用：父级不直接持有内核实例。 */
  getSelection: () => string;
  hasSelection: () => boolean;
  selectAll: () => void;
  paste: (text: string) => void;
}

export interface TerminalPaneProps {
  /** null 表示空面板（无绑定页签）。 */
  tab: TerminalTab | null;
  focused: boolean;
  /** 当前布局下的面板数，仅用于回滚缓冲摊薄（变化不重建实例）。 */
  paneTotal: number;
  /** 换内核时父级把纪元 +1，强制重建实例。 */
  engineEpoch: number;
  /** 取某页签的重放文本（来自父级的回滚缓冲）。 */
  getReplayText: (tabId: string) => string;
  /** 取某页签当前的 PTY 会话 id（剪贴板会话校验用）。 */
  getSessionId: (tabId: string) => string | undefined;
  /** 键盘输入回传给父级（父级负责排队写 PTY）。 */
  onInput: (tabId: string, data: string) => void;
  /**
   * 交互式程序状态变化（开发方案 8.5 / F54）。
   * 只有面板拿得到内核实例，所以 mode 探测必须在这里做，再上报给父级去
   * 开关命令块切块——否则 vim/top 里的按键会被当成命令文本记进 block。
   */
  onInteractiveChange?: (tabId: string, interactive: boolean) => void;
  /**
   * 工作目录变化（设计文档 §15 cwd 跟随）：内核从 OSC 7 收到 `pwd` 后回调，
   * 由面板带上 tabId 上报父级，父级据此更新该页签的 root，右栏文件浏览自动跟随。
   * 只有 SSH / 本地 shell 会发 OSC 7；ADB 设备侧一般不发，回调不会触发。
   */
  onPwdChange?: (tabId: string, path: string) => void;
  /** 面板尺寸变化（父级负责 resize PTY）。 */
  onResize: (tabId: string, cols: number, rows: number) => void;
  onFocus: (tabId: string) => void;
  onContextMenu: (event: React.MouseEvent, tabId: string) => void;
  /** 设置里改了内核：面板无法自己重建，交给父级把纪元 +1。 */
  onEngineSwitchRequest: () => void;
  /** 实例就绪后回报内核名（父级顶栏胶囊用）。 */
  onEngine: (engine: TerminalEngine) => void;
  /** 面板挂载/卸载时登记句柄，父级用 tabId 索引以便投递输出。 */
  onRegister: (tabId: string, handle: TerminalPaneHandle | null) => void;
  /** 仅焦点面板渲染的附加条（设备切换 / 常用命令）。 */
  overlay?: React.ReactNode;
  /** 空面板时的提示文案。 */
  emptyHint?: string;
  /**
   * 空面板时整块替换掉默认占位内容（用「新建终端」悬浮选择器顶掉静态提示）。
   * 传了就完全由调用方负责这一块的排版。
   */
  emptyContent?: React.ReactNode;
  /**
   * 面板右上角的叉：只解绑该面板（页签仍在左栏），不是关会话。
   * 父级负责把后面的面板前移补位（需求 8）。
   */
  onDetach?: (tabId: string) => void;
  /** 拖拽：把某页签放到指定面板槽位（需求：左栏可拖到中栏指定位置）。 */
  /**
   * 页签拖到本面板上松手（需求 ⑤）。
   *
   * 多带两个相对坐标（相对本面板内容盒的 0..1）：父级要用它判断指针是否真的落在
   * 本面板内——`onDragOver` 只在指针进入时触发，`onDrop` 时指针也可能已经滑到
   * 面板边缘外（跨面板拖拽时很常见），光凭 slot 会把"擦边"当成"换位"。
   */
  onDropTab?: (slot: number, tabId: string, xRatio: number, yRatio: number) => void;
  /** 拖拽：把「已保存服务器」拖到某个槽位，在那个槽位新开一个连接。 */
  onDropSavedServer?: (slot: number, kind: "ssh" | "wsl", id: string) => void;
  /** 本面板在网格中的槽位下标。 */
  slot: number;
  /**
   * 嵌入模式（对话页等复用方）：不渲染面板标题栏，去掉卡片外框/圆角/焦点环，
   * 由宿主容器提供自己的框架与按钮——避免双层圆角和重复的标题/关闭行。
   */
  chromeless?: boolean;
}

export const TerminalPane: React.FC<TerminalPaneProps> = ({
  tab,
  focused,
  paneTotal,
  engineEpoch,
  getReplayText,
  getSessionId,
  onInput,
  onInteractiveChange,
  onPwdChange,
  onResize,
  onFocus,
  onContextMenu,
  onEngineSwitchRequest,
  onEngine,
  onRegister,
  overlay,
  emptyHint,
  emptyContent,
  onDetach,
  onDropTab,
  onDropSavedServer,
  slot,
  chromeless,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  /** 面板卡片根节点：底色跟随终端主题（覆盖圆角四角），见 syncBackdrop。 */
  const paneRef = useRef<HTMLDivElement | null>(null);
  // 回调与当前页签都放 ref：实例 effect 的依赖里不能出现 tab.id，否则换绑页签
  // 会重新 acquire（每次 acquire 都要软重置 + 重放，还会把池中实例来回搬动）。
  const tabIdRef = useRef<string | null>(tab?.id ?? null);
  tabIdRef.current = tab?.id ?? null;
  const focusedRef = useRef(focused);
  focusedRef.current = focused;
  const paneTotalRef = useRef(paneTotal);
  paneTotalRef.current = paneTotal;
  const callbacksRef = useRef({ getReplayText, getSessionId, onInput, onInteractiveChange, onPwdChange, onResize, onFocus, onContextMenu, onEngineSwitchRequest, onEngine, onRegister });
  callbacksRef.current = { getReplayText, getSessionId, onInput, onInteractiveChange, onPwdChange, onResize, onFocus, onContextMenu, onEngineSwitchRequest, onEngine, onRegister };

  const queueRef = useRef<TerminalTextBuffer>({ chunks: [], start: 0, length: 0 });
  const frameRef = useRef<number | null>(null);
  const writingRef = useRef(false);
  const followLatestRef = useRef(true);
  const terminalRef = useRef<AppTerminal | null>(null);
  const resizeRef = useRef<(() => { cols: number; rows: number } | null) | null>(null);
  const hasTab = tab !== null;

  useEffect(() => {
    if (!hasTab) return;
    const host = hostRef.current;
    if (!host) return;
    const queue = queueRef.current;
    const initialSettings = readTerminalSettings();
    let disposed = false;
    let cleanup: (() => void) | null = null;
    // 代际令牌 + 自愈看门狗：StrictMode 双挂载与实例池「同步 attach、迟到释放」
    // 交错时，最后一次挂载可能既拿不到实例、也不会再有 effect 触发——面板就永久
    // 停在"已绑定但无实例"的白屏（复现：1x1 下开第 2 个终端，升级布局后的新面板
    // canvas 数为 0）。令牌让旧尝试的迟到结果自动作废；看门狗发现 600ms 后仍无
    // 实例就重试，把任何一种交错损失都兜回来。
    let attempt = 0;
    let watchdog: number | null = null;
    let activeLease: TerminalLease | null = null;
    /** 获取一次实例；可安全重入（重试前先作废旧尝试）。 */
    const beginAttempt = () => {
    const token = ++attempt;
    // 实例池会把容器挂到宿主下，先清掉上一轮可能残留的节点。
    clearTerminalHost(host);
    clearTerminalTextBuffer(queue);
    writingRef.current = false;
    followLatestRef.current = true;

    const schedule = () => {
      if (frameRef.current !== null || disposed) return;
      frameRef.current = window.requestAnimationFrame(() => {
        frameRef.current = null;
        const term = terminalRef.current;
        if (!term || disposed) return;
        if (writingRef.current) {
          // 上一次写入还没回调完：等它结束后再排一帧，避免同一实例并发写。
          schedule();
          return;
        }
        const budget = focusedRef.current
          ? OUTPUT_CHARS_PER_FRAME
          : Math.floor(OUTPUT_CHARS_PER_FRAME / UNFOCUSED_DIVISOR);
        const output = takeTerminalText(queue, budget);
        if (!output) return;
        writingRef.current = true;
        term.write(output, () => {
          writingRef.current = false;
          if (disposed || terminalRef.current !== term) return;
          if (followLatestRef.current) term.scrollToBottom();
          if (queue.length) schedule();
        });
      });
    };

    void acquireTerminal(
      (engine) => ({
        convertEol: false,
        fontFamily: getAppMonoFontStack(),
        ...terminalOptions(initialSettings, engine),
        ...resolveEngineAppearance(initialSettings, host, engine),
        ...(engine === "core"
          ? {
              scrollback: scrollbackLinesForSession({
                requested: initialSettings.scrollback,
                terminals: paneTotalRef.current || 1,
              }),
            }
          : {}),
      }),
      host,
    )
      .then((lease) => {
        const { terminal: term, fit, engine } = lease;
        if (!disposed && token === attempt) activeLease = lease;
        if (disposed || token !== attempt) {
          lease.release();
          return;
        }
        // 两个内核的 FitAddon 都要先 activate 才有 `_terminal`，否则
        // `proposeDimensions()` 恒为 undefined，`fitTerminalGrid` 直接放弃——
        // 建立时（双格）量出的列数会被永久沿用：切到单格后宿主宽了一倍，文字
        // 仍按双列排版、右半边空着。池里 `createFresh` 装过，这里再装一次是幂等
        // 的（直接覆盖内部引用），顺带保证任何获取路径都不漏。
        try {
          fit.activate?.(term);
        } catch {
          /* 内核缺少 activate 时跳过（理论上不会发生）。 */
        }
        host.dataset.terminalEngine = engine;
        terminalRef.current = term;
        /**
         * 三层容器一起跟随终端主题上底色：内核画布只铺字符网格，外面的
         * `.terminal-xterm-fit`（hostRef）、带 padding 环的 `.terminal-output-viewport`
         * （外层视口，CSS 里是应用主题变量）、以及面板卡片根节点（圆角裁剪露出
         * 的四角）任何一层没跟上，选内置主题后就是一圈"没盖住"的异色空白。
         */
        const syncBackdrop = (settings: TerminalSettings, engine: TerminalEngine) => {
          const appearance = resolveEngineAppearance(settings, host, engine);
          const background = appearance.theme.background || "";
          host.style.background = background;
          const viewport = host.parentElement;
          if (viewport?.classList.contains("terminal-output-viewport")) viewport.style.background = background;
          if (paneRef.current) paneRef.current.style.background = background;
          return appearance;
        };
        syncBackdrop(initialSettings, engine);
        const unbindClipboard = bindTerminalClipboard(term, {
          getSessionId: () => {
            const tabId = tabIdRef.current;
            return tabId ? callbacksRef.current.getSessionId(tabId) : undefined;
          },
          onError: (message) => console.warn("[terminal]", message),
          engine,
        });
        const firstFit = fitTerminalGrid(term, fit);
        // 首次 fit 没量出尺寸时，回放挂起等 settle 双帧补量（见 settleFrame）。
        let pendingReplayOnSettle = false;
        let resizeFrame: number | null = null;
        let scrollFrame: number | null = null;
        const scrollToLatest = (shouldFollow = followLatestRef.current) => {
          if (!shouldFollow || scrollFrame !== null) return;
          scrollFrame = window.requestAnimationFrame(() => {
            scrollFrame = null;
            if (disposed) return;
            term.scrollToBottom();
            followLatestRef.current = true;
          });
        };
        // 回放期间内核产生的查询应答（DSR/DA）绝不能进 PTY——它们是历史输出的
        // 回声，写进 shell 会把提示符搅乱（光标错位 / 右侧时间换行）。
        let replayingHistory = false;
        const writeReplay = () => {
          const tabId = tabIdRef.current;
          const data = tabId ? callbacksRef.current.getReplayText(tabId) : "";
          if (!data) {
            scrollToLatest(true);
            return;
          }
          replayingHistory = true;
          term.write(sanitizeTerminalReplay(data), () => {
            replayingHistory = false;
            scrollToLatest(true);
          });
        };
        const currentTabId = tabIdRef.current;
        const buffered = currentTabId ? callbacksRef.current.getReplayText(currentTabId) : "";
        if (buffered) {
          if (firstFit) writeReplay();
          // 首次 fit 失败（宿主还没排完版，常见于拖拽互换的重挂载帧）：不能按
          // 旧列宽回放——等 settle 双帧补量出真实尺寸后再写，否则提示符按错误
          // 宽度折行（右侧时间被挤到第二行、光标不在提示符后面）。
          else pendingReplayOnSettle = true;
        } else scrollToLatest(true);
        const dataSubscription = term.onData((data) => {
          if (replayingHistory) return;
          const tabId = tabIdRef.current;
          if (!tabId) return;
          callbacksRef.current.onInput(tabId, data);
        });
        // §15 cwd 跟随：内核解析 OSC 7 得到远端/本地当前目录后回调。回调里再取
        // tabIdRef，因为页签换绑不会重建实例（回调绑定的是实例，不是页签）。
        const pwdSubscription = term.onPwdChange?.((pwd) => {
          const tabId = tabIdRef.current;
          if (!tabId || !pwd) return;
          callbacksRef.current.onPwdChange?.(tabId, pwd);
        });
        /**
         * 交互式程序探测（开发方案 8.5）：`hasMouseTracking` / `getMode(1049)`
         * 是内核实例上的方法，只有面板拿得到，因此在这里定时读一次再上报。
         * 不能挂 onData 里逐键判断：vim 全屏后很久不按键，状态得靠轮询发现。
         *
         * 必须校验 `terminalRef.current === term`：实例被 `lease.release()` 归还池子后，
         * cleanup 未必先于本次定时器回调跑完，此时读内核会抛
         * 内核句柄失效（result -2）异常。
         */
        const probeInteractive = () => {
          if (disposed || terminalRef.current !== term) return;
          const tabId = tabIdRef.current;
          if (!tabId) return;
          const decision = decidePassthrough({ terminal: modeProbe(term) });
          callbacksRef.current.onInteractiveChange?.(tabId, decision.passthrough);
        };
        const interactiveTimer = window.setInterval(probeInteractive, 1000);
        const scrollSubscription = term.onScroll(() => {
          followLatestRef.current = isFollowingLatestLine(term.buffer.active);
        });
        // 自绘滚动条：内建那条画在 canvas 里（贴不到面板右框、还压正文），见
        // `terminalScrollbar.ts` 顶部的说明。设置项「内建滚动条」现在管的是这一条。
        // xterm 内核自带一条独立的 DOM 滚动条，不需要也不该再叠一条。
        const scrollbar =
          engine === "core"
            ? mountTerminalScrollbar(term, host, () => readTerminalSettings().scrollbar)
            : { update: () => undefined, dispose: () => undefined };
        // 焦点放到隐藏 textarea 而不是宿主 div：宿主被内核设了 contenteditable，
        // WebView2 会把首键交给编辑通路（吞键 + 系统提示音），见 terminalAdapter。
        if (focusedRef.current) focusTerminalInput(term);
        // 首键「叮」兜底：acquire 异步窗口 / 程序化 focus() 的竞态里，contenteditable
        // 宿主仍可能短暂拿到焦点，落上去的第一下按键就会触发系统提示音并被吞掉。
        // 监听 focusin：焦点只要不是落在隐藏 textarea 上，立刻转给 textarea。
        const redirectHostFocus = (event: FocusEvent) => {
          if (disposed || terminalRef.current !== term) return;
          if (event.target instanceof HTMLTextAreaElement) return;
          const input = term.element?.querySelector("textarea");
          if (input instanceof HTMLElement && document.activeElement !== input) input.focus();
        };
        host.addEventListener("focusin", redirectHostFocus);
        let lastSize = "";
        const fitTerminal = () => {
          if (disposed) return null;
          const shouldFollow = followLatestRef.current;
          const dimensions = fitTerminalGrid(term, fit);
          if (!dimensions) return null;
          scrollbar.update();
          scrollToLatest(shouldFollow);
          const tabId = tabIdRef.current;
          if (tabId) {
            const size = `${tabId}:${dimensions.cols}x${dimensions.rows}`;
            if (size !== lastSize) {
              lastSize = size;
              callbacksRef.current.onResize(tabId, dimensions.cols, dimensions.rows);
            }
          }
          return dimensions;
        };
        let lastFitAt = 0;
        const scheduleFit = () => {
          if (resizeFrame !== null) return;
          resizeFrame = window.requestAnimationFrame(() => {
            resizeFrame = null;
            // 拖拽期间 ResizeObserver 每帧都来：内核 resize 是重活，80ms 内只做一次，
            // 尾随补一次——跟手性不受影响，快速拖拽不再"轻微卡顿"。
            const elapsed = performance.now() - lastFitAt;
            if (elapsed < 80) {
              window.setTimeout(scheduleFit, 80 - elapsed);
              return;
            }
            lastFitAt = performance.now();
            fitTerminal();
          });
        };
        resizeRef.current = fitTerminal;
        const observer = new ResizeObserver(scheduleFit);
        observer.observe(host);
        // xterm 渲染在 .xterm-screen 内层，自研内核直接画在宿主里的 canvas 上。
        observer.observe(term.element?.querySelector("canvas") ?? term.element?.querySelector(".xterm-screen") ?? host);
        window.addEventListener("resize", scheduleFit);
        let applied = initialSettings;
        const applySettings = () => {
          const next = readTerminalSettings();
          if (next.engine !== applied.engine) {
            // 换内核只能换实例：请求父级把纪元 +1，旧实例由各面板 cleanup 归还池子。
            applied = next;
            callbacksRef.current.onEngineSwitchRequest();
            return;
          }
          // 自研内核的 WASM 内存不可收缩，回滚缓冲只能在创建时按预算分配；
          // 因此热更新也要用预算值覆盖，否则设置一改就回退成用户设置值、突破预算。
          const effective =
            engine === "core"
              ? {
                  ...next,
                  scrollback: scrollbackLinesForSession({
                    requested: next.scrollback,
                    terminals: paneTotalRef.current || 1,
                  }),
                }
              : next;
          applyTerminalSettings(term, effective, engine);
          // 主题热更新：外观 + 宿主/卡片底色一起同步（初始见 acquire 完成处），
          // 直接写行内样式、不重渲染 React。
          const appearance = syncBackdrop(next, engine);
          term.options.theme = appearance.theme;
          term.options.minimumContrastRatio = appearance.minimumContrastRatio;
          // 换调色板后强制重绘一屏：个别内核不主动重画已渲染的帧，表现为
          // 面板整块空白——refresh 不可用时静默跳过（xterm 有、Turbo 视实现）。
          try {
            (term as { refresh?: (start: number, end: number) => void }).refresh?.(0, Math.max(1, term.rows || 50) - 1);
          } catch {
            /* 内核不支持 refresh：忽略，等待下一次写入自然重绘。 */
          }
          // 渲染后端 / 内建滚动条不在内核的 handleOptionChange 里，改完要重装渲染器才生效。
          if (next.renderer !== applied.renderer || next.scrollbar !== applied.scrollbar) {
            refreshTerminalRenderer(term);
          }
          applied = next;
          scheduleFit();
        };
        const unsubscribeTheme = subscribeAppTheme(applySettings);
        const unsubscribeSettings = subscribeTerminalSettings(applySettings);
        const unsubscribeMonoFont = watchTerminalFont(term, scheduleFit);
        const unbindPause = bindTerminalPageVisibility(term);
        fitTerminal();
        // 首帧补偿：acquire 是异步的，恢复时视图可能还没完成布局（多屏切换、页签刚
        // 插进网格、文件抽屉开合的同帧），此时 `fitTerminalGrid` 因为父级尺寸为 0 直接
        // 返回 undefined，内核停在创建时的列宽/行数，画布内容整块错位或空白。等两帧
        // 让布局稳定后再量一次；ResizeObserver 只在尺寸"变化"时回调，补不上这一次。
        const settleFrame = window.requestAnimationFrame(() => {
          window.requestAnimationFrame(() => {
            if (disposed || terminalRef.current !== term) return;
            const fitted = fitTerminal();
            if (!pendingReplayOnSettle) return;
            if (fitted) {
              pendingReplayOnSettle = false;
              writeReplay();
            } else {
              // 连两帧都量不出尺寸：不再等，按当前尺寸回放（宁可折行也不空白）。
              window.setTimeout(() => {
                if (disposed || terminalRef.current !== term || !pendingReplayOnSettle) return;
                pendingReplayOnSettle = false;
                fitTerminal();
                writeReplay();
              }, 120);
            }
          });
        });

        const handle: TerminalPaneHandle = {
          write: (data: string) => {
            appendTerminalText(queue, data, QUEUE_LIMIT);
            schedule();
          },
          fit: fitTerminal,
          focus: () => focusTerminalInput(term),
          clear: () => {
            clearTerminalTextBuffer(queue);
            clearTerminalContent(term);
            focusTerminalInput(term);
          },
          replay: () => {
            const tabId = tabIdRef.current;
            followLatestRef.current = true;
            clearTerminalTextBuffer(queue);
            resetTerminalContent(term);
            resizeRef.current?.();
            const data = tabId ? callbacksRef.current.getReplayText(tabId) : "";
            if (data) term.write(data, () => term.scrollToBottom());
            else term.scrollToBottom();
            if (focusedRef.current) focusTerminalInput(term);
          },
          getSelection: () => term.getSelection(),
          hasSelection: () => term.hasSelection(),
          selectAll: () => term.selectAll?.(),
          paste: (text: string) => term.paste(text),
        };
        const registeredId = tabIdRef.current;
        if (registeredId) callbacksRef.current.onRegister(registeredId, handle);
        callbacksRef.current.onEngine(engine);

        cleanup = () => {
          disposed = true;
          const activeId = tabIdRef.current;
          if (activeId) callbacksRef.current.onRegister(activeId, null);
          window.clearInterval(interactiveTimer);
          window.cancelAnimationFrame(settleFrame);
          unsubscribeTheme();
          unsubscribeSettings();
          unsubscribeMonoFont();
          unbindPause();
          unbindClipboard();
          host.removeEventListener("focusin", redirectHostFocus);
          observer.disconnect();
          window.removeEventListener("resize", scheduleFit);
          dataSubscription.dispose();
          pwdSubscription?.dispose();
          scrollbar.dispose();
          scrollSubscription.dispose();
          if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
          if (scrollFrame !== null) window.cancelAnimationFrame(scrollFrame);
          if (frameRef.current !== null) {
            window.cancelAnimationFrame(frameRef.current);
            frameRef.current = null;
          }
          clearTerminalTextBuffer(queue);
          writingRef.current = false;
          resizeRef.current = null;
          terminalRef.current = null;
          // 归还给实例池（暂停渲染、交出 WebGL、摘出 DOM），不销毁：WASM 内存不可回收。
          lease.release();
        };
      })
      .catch((reason) => console.error("[terminal] 面板实例创建失败", reason));
    };

    beginAttempt();
    let retries = 0;
    const armWatchdog = () => {
      if (watchdog !== null) return;
      watchdog = window.setTimeout(() => {
        watchdog = null;
        if (disposed || !tabIdRef.current) return;
        const term = terminalRef.current;
        // 健康判定：有实例，且内核真的把**画布与输入 textarea**都装进了宿主容器。
        // 实测（repro-mutation）池恢复交错后 lease 可以指向一个"空壳实例"——
        // 容器在 DOM 里但没有 canvas/textarea，写输入也没地方去，面板永久白屏
        //（用户实测"时好时坏"：只查 canvas 会漏掉"画布在、输入框不在"的半壳，
        // 表现为空白 + 打不进字——两个都在才算活，缺一个就当死亡重试）。
        const aliveHost = hostRef.current;
        const rendererAlive =
          term !== null &&
          aliveHost?.querySelector("canvas") != null &&
          aliveHost?.querySelector("textarea") != null;
        if (term && rendererAlive) return;
        if (retries >= 3) {
          console.warn("[terminal] 面板实例多次未挂上；切换一次布局或页签可恢复", tabIdRef.current);
          return;
        }
        retries += 1;
        if (term) {
          // 渲染器已死的实例销毁而不是寄存：寄存会把同一只空壳再次发出去。
          activeLease?.release({ dispose: true });
          activeLease = null;
          terminalRef.current = null;
          cleanup?.();
          cleanup = null;
        }
        beginAttempt();
        armWatchdog();
      }, 600);
    };
    armWatchdog();

    return () => {
      disposed = true;
      if (watchdog !== null) {
        window.clearTimeout(watchdog);
        watchdog = null;
      }
      cleanup?.();
    };
    // 只在"有没有绑定页签"和"内核纪元"变化时重建；paneTotal 走 ref 进预算，不触发重建。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasTab, engineEpoch]);

  // ---- 换绑页签：不重建实例，软重置后重放该页签的回滚缓冲 ----------------
  const boundTabIdRef = useRef<string | null>(null);
  useEffect(() => {
    const tabId = tab?.id ?? null;
    if (!tabId) {
      boundTabIdRef.current = null;
      return;
    }
    // 首次绑定由实例 effect 自己写入缓冲；这里只处理"换了一张页签"。
    if (boundTabIdRef.current === null) {
      boundTabIdRef.current = tabId;
      return;
    }
    if (boundTabIdRef.current === tabId) return;
    boundTabIdRef.current = tabId;
    const term = terminalRef.current;
    if (!term) return;
    followLatestRef.current = true;
    clearTerminalTextBuffer(queueRef.current);
    resetTerminalContent(term);
    resizeRef.current?.();
    const data = callbacksRef.current.getReplayText(tabId);
    // 换绑回放同样要清洗历史查询序列：不能让内核对着旧 DSR/DA 再应答一次。
    if (data) term.write(sanitizeTerminalReplay(data), () => term.scrollToBottom());
    else term.scrollToBottom();
    if (focusedRef.current) focusTerminalInput(term);
  }, [tab?.id]);

  // ---- 成为焦点面板时把键盘焦点拿过来（不抢用户正在别处输入的焦点）------
  useEffect(() => {
    if (!focused) return;
    const term = terminalRef.current;
    if (!term) return;
    const active = document.activeElement;
    if (!active || active === document.body || hostRef.current?.contains(active)) {
      focusTerminalInput(term);
    }
  }, [focused]);

  const paneType = tab?.type;
  const paneTag =
    paneType === "adb"
      ? "ADB"
      : paneType === "ssh"
        ? "SSH"
        : paneType === "wsl"
          ? "WSL"
          : paneType === "local"
            ? tab?.shell === "cmd"
              ? "CMD"
              : "Local"
            : "";
  const queuedChars = queueRef.current.length;

  return (
    <div
      ref={paneRef}
      className="terminal-pane"
      data-focused={focused && hasTab ? "true" : "false"}
      data-empty={hasTab ? "false" : "true"}
      data-drop-slot={slot}
      data-chromeless={chromeless ? "true" : undefined}
      onMouseDown={() => tab && onFocus(tab.id)}
      // 左栏的终端卡片可拖到这里：指定面板槽位（需求：拖到中间指定位置）。
      onDragOver={(event) => {
        // 两种载荷都收：左栏「已打开终端」（x-terminal-tab）与「已保存服务器」
        // （x-terminal-saved）。后者拖进来等于在这个槽位新开一个连接。
        const types = event.dataTransfer.types;
        if (!types.includes("application/x-terminal-tab") && !types.includes("application/x-terminal-saved")) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = types.includes("application/x-terminal-tab") ? "move" : "copy";
        event.currentTarget.dataset.dragOver = "true";
      }}
      onDragLeave={(event) => {
        delete (event.currentTarget as HTMLElement).dataset.dragOver;
      }}
      onDrop={(event) => {
        delete (event.currentTarget as HTMLElement).dataset.dragOver;
        // 保存的服务器载荷格式是 "ssh:<id>" / "wsl:<id>"。id 里理论上可能带冒号，
        // 所以只能按第一个冒号切分再拼回剩余部分，不能用 split(":")[1]。
        const saved = event.dataTransfer.getData("application/x-terminal-saved");
        if (saved) {
          const separator = saved.indexOf(":");
          if (separator > 0) {
            event.preventDefault();
            const kind = saved.slice(0, separator);
            const id = saved.slice(separator + 1);
            if ((kind === "ssh" || kind === "wsl") && id) {
              onDropSavedServer?.(slot, kind, id);
              return;
            }
          }
        }
        const dropped = event.dataTransfer.getData("application/x-terminal-tab");
        if (!dropped) return;
        event.preventDefault();
        // 相对本面板内容盒的 0..1 坐标：边界情况（擦边松手）由父级用
        // `planDropOnPane` 的 `isInsideZone` 判掉，这里只负责如实上报。
        const rect = event.currentTarget.getBoundingClientRect();
        const xRatio = rect.width > 0 ? (event.clientX - rect.left) / rect.width : Number.NaN;
        const yRatio = rect.height > 0 ? (event.clientY - rect.top) / rect.height : Number.NaN;
        onDropTab?.(slot, dropped, xRatio, yRatio);
      }}
    >
      {!chromeless && (
      <div
        className="terminal-pane-head"
        title={tab ? `拖动标题栏到另一块面板可交换位置` : undefined}
        onPointerDown={(event) => {
          // 面板自身的拖动换位（v3 需求）：拖标题栏 → 命中其它面板 → 交换。
          // 复用 HTML5 时代的 onDropTab 处理链（planDropOnPane 的交换语义）。
          if (!tab) return;
          startPointerDrag(event, {
            label: tab.title,
            ignore: event.currentTarget.closest<HTMLElement>(".terminal-pane"),
            onDrop: (target, xRatio, yRatio) => {
              const slot = Number(target.dataset.dropSlot);
              if (!Number.isInteger(slot)) return;
              onDropTab?.(slot, tab.id, xRatio, yRatio);
            },
          });
        }}
      >
        <div className="terminal-pane-title">
          <TerminalIcon size={12} />
          <span>{tab?.title || "空闲面板"}</span>
        </div>
        {paneTag && <span className="terminal-pane-tag">{paneTag}</span>}
        {hasTab && tab?.sessionId && <span className="terminal-pane-dot" data-live="true" />}
        {hasTab && tab && onDetach && (
          <Button
            variant="bare"
            size="xs"
            icon={<X size={11} />}
            aria-label={`从面板移除 ${tab.title}`}
            title="从这个面板移除（会话仍在左栏，可再拖回来）"
            className="terminal-pane-close"
            onClick={(event) => {
              event.stopPropagation();
              onDetach(tab.id);
            }}
          />
        )}
      </div>
      )}
      {overlay}
      {hasTab ? (
        <div className="terminal-output-viewport terminal-xterm-host">
          <div
            ref={hostRef}
            className="terminal-xterm-fit"
            data-backlog={queuedChars}
            onMouseDown={() => focusTerminalInput(terminalRef.current)}
            onContextMenu={(event) => {
              event.preventDefault();
              if (tab) onContextMenu(event, tab.id);
            }}
          />
        </div>
      ) : (
        emptyContent || (
          <div className="terminal-pane-empty">
            <TerminalIcon size={20} />
            <span>空闲面板</span>
            <small>{emptyHint || "点左栏里已打开的终端占用这个面板"}</small>
          </div>
        )
      )}
    </div>
  );
};

/** 供父级读取某页签已缓存的回滚文本（`readTerminalText` 的薄封装，保持单点导入）。 */
export function readPaneReplayText(buffers: Record<string, TerminalTextBuffer>, tabId: string): string {
  return readTerminalText(buffers[tabId]);
}
