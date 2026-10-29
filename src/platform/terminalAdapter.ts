/**
 * 终端内核适配层（开发方案 7.3）。
 *
 * 用一套中立类型同时描述自研终端内核（WASM）与 xterm.js，
 * 让视图层不再直接依赖 `@xterm/*`。灰度期通过设置项 `terminal.engine`
 * 在 "core" / "xterm" 之间切换，xterm 分支始终保持可用以便回滚。
 */
import { FitAddon as XtermFitAddon } from "@xterm/addon-fit";
import { Terminal as XtermTerminal, type ITerminalOptions } from "@xterm/xterm";
import { createCustomFitAddon, createCustomTerminal } from "./customTerminal";
import { readTerminalSettings, resolveTerminalAppearance, type TerminalSettings } from "./terminalSettings";

export type TerminalEngine = "core" | "xterm";
export type TerminalRenderer = "webgl" | "canvas";

/** 两个内核共有的主题字段（并集）。 */
export interface TerminalTheme {
  foreground?: string;
  background?: string;
  cursor?: string;
  cursorAccent?: string;
  selectionBackground?: string;
  selectionForeground?: string;
  black?: string;
  red?: string;
  green?: string;
  yellow?: string;
  blue?: string;
  magenta?: string;
  cyan?: string;
  white?: string;
  brightBlack?: string;
  brightRed?: string;
  brightGreen?: string;
  brightYellow?: string;
  brightBlue?: string;
  brightMagenta?: string;
  brightCyan?: string;
  brightWhite?: string;
  /** 仅 xterm.js 使用；自研内核不接受该字段（构造前需剥离）。 */
  extendedAnsi?: string[];
}

/** 两个内核共有的构造参数/运行时选项（并集）。 */
export interface TerminalOptions {
  cols?: number;
  rows?: number;
  cursorBlink?: boolean;
  cursorStyle?: "block" | "bar" | "underline";
  fontFamily?: string;
  fontSize?: number;
  fontWeight?: number | "normal" | "bold";
  fontWeightBold?: number | "normal" | "bold";
  letterSpacing?: number;
  lineHeight?: number;
  scrollback?: number;
  scrollSensitivity?: number;
  smoothScrollDuration?: number;
  minimumContrastRatio?: number;
  allowTransparency?: boolean;
  convertEol?: boolean;
  disableStdin?: boolean;
  scrollOnUserInput?: boolean;
  macOptionIsMeta?: boolean;
  theme?: TerminalTheme;
  /** 仅自研内核生效：渲染后端。 */
  renderer?: TerminalRenderer;
  /** 仅自研内核生效：内建滚动条。 */
  scrollbar?: boolean;
}

export interface TerminalDisposable {
  dispose(): void;
}

export interface TerminalBufferLine {
  translateToString(trimRight?: boolean, startColumn?: number, endColumn?: number): string;
}

export interface TerminalBuffer {
  readonly viewportY: number;
  readonly baseY: number;
  readonly length: number;
  readonly cursorX: number;
  readonly cursorY: number;
  getLine(y: number): TerminalBufferLine | null | undefined;
}

export interface TerminalBufferNamespace {
  readonly active: TerminalBuffer;
  readonly normal: TerminalBuffer;
  readonly alternate: TerminalBuffer;
}

/** 中立终端接口：视图层只能通过它访问内核。 */
export interface AppTerminal {
  readonly element?: HTMLElement;
  cols: number;
  rows: number;
  options: TerminalOptions;
  readonly buffer: TerminalBufferNamespace;
  /** 自研内核返回 'webgl' | 'canvas'；xterm 不提供该字段。 */
  readonly rendererType?: TerminalRenderer;
  loadAddon(addon: unknown): void;
  open(parent: HTMLElement): void;
  write(data: string | Uint8Array, callback?: () => void): void;
  onData(listener: (data: string) => void): TerminalDisposable;
  onScroll(listener: (line?: number) => void): TerminalDisposable;
  focus(): void;
  blur(): void;
  reset(): void;
  clear(): void;
  scrollToBottom(): void;
  scrollToTop(): void;
  scrollLines(amount: number): void;
  resize(cols: number, rows: number): void;
  dispose(): void;
  paste(data: string): void;
  getSelection(): string;
  hasSelection(): boolean;
  selectAll?(): void;
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void;
  /** 仅自研内核提供：暂停渲染，可顺带释放 WebGL 上下文。 */
  setPaused?(paused: boolean, options?: { releaseRenderer?: boolean }): void;
  /**
   * 仅自研内核提供：shell 通过 OSC 7 上报的工作目录（设计文档 §15 cwd 跟随）。
   * xterm 内核没有这条通路，取不到就是 undefined，调用方必须可缺省。
   */
  onPwdChange?(listener: (pwd: string) => void): TerminalDisposable;
}

export interface AppFitAddon {
  fit(): void;
  proposeDimensions(): { cols: number; rows: number } | undefined;
  activate?(terminal: unknown): void;
  dispose?(): void;
}

export interface AppTerminalSession {
  terminal: AppTerminal;
  fit: AppFitAddon;
  engine: TerminalEngine;
}

/** 当前设置里选择的内核。 */
export function readTerminalEngine(): TerminalEngine {
  return readTerminalSettings().engine === "xterm" ? "xterm" : "core";
}

/**
 * 创建终端实例。`build` 会拿到最终生效的内核，用于拼装内核相关的选项
 * （例如 renderer/scrollbar 只有自研内核认）。
 * 自研内核加载失败时自动回退 xterm，保证终端始终可用。
 */
export async function createAppTerminal(
  build: (engine: TerminalEngine) => TerminalOptions,
): Promise<AppTerminalSession> {
  if (readTerminalEngine() === "core") {
    try {
      const terminal = await createCustomTerminal(build("core"));
      return { terminal, fit: createCustomFitAddon(), engine: "core" };
    } catch (error) {
      console.warn("[terminal] 自研内核加载失败，本次回退到 xterm.js", error);
    }
  }
  const terminal = new XtermTerminal(build("xterm") as ITerminalOptions);
  // xterm.js 没有 OSC 7 的回调口子：`onPwdChange` 保持 undefined，视图层据此跳过
  // 订阅（设计文档 §15 的 cwd 跟随只对自研内核生效）。
  return {
    terminal: terminal as unknown as AppTerminal,
    fit: new XtermFitAddon() as unknown as AppFitAddon,
    engine: "xterm",
  };
}

/** 剥离只有 xterm 认识的主题字段（extendedAnsi）。 */
export function engineTheme(theme: TerminalTheme, engine: TerminalEngine): TerminalTheme {
  if (engine !== "core") return theme;
  const { extendedAnsi: _extendedAnsi, ...rest } = theme;
  return rest;
}

/** 视图构造与热更新共用的外观选项。 */
export function resolveEngineAppearance(
  settings: TerminalSettings,
  element: Element | null | undefined,
  engine: TerminalEngine,
): { theme: TerminalTheme; minimumContrastRatio: number } {
  const appearance = resolveTerminalAppearance(settings, element ?? undefined);
  return {
    theme: engineTheme(appearance.theme, engine),
    minimumContrastRatio: appearance.minimumContrastRatio,
  };
}

/**
 * 页面不可见时暂停渲染并释放 WebGL 上下文，回到前台再恢复（开发方案 6.8）。
 * xterm 内核没有该能力，直接返回空清理函数。
 */
export function bindTerminalPageVisibility(terminal: AppTerminal): () => void {
  if (typeof terminal.setPaused !== "function") return () => undefined;
  const sync = () => {
    const hidden = typeof document !== "undefined" && document.visibilityState === "hidden";
    try {
      terminal.setPaused?.(hidden, hidden ? { releaseRenderer: true } : {});
    } catch (error) {
      console.warn("[terminal] setPaused 调用失败", error);
    }
  };
  document.addEventListener("visibilitychange", sync);
  sync();
  return () => document.removeEventListener("visibilitychange", sync);
}

/**
 * 清掉宿主里残留的终端节点（canvas / textarea）。
 * React 严格模式下 effect 会先挂载再卸载，而终端是异步创建的：一旦某次实例的
 * dispose 没跑到渲染器清理，宿主的 canvas/textarea 就会留下旧画面（表现为残影
 * 或一行高的旧光标）。open() 之前与 dispose() 之后都调用一次，避免累积。
 */
export function clearTerminalHost(host: HTMLElement | null | undefined): void {
  if (!host) return;
  for (const node of Array.from(host.querySelectorAll("canvas, textarea"))) {
    node.remove();
  }
}

/**
 * 让"渲染后端 / 内建滚动条"的改动立刻生效（设置项 `renderer`、`scrollbar`）。
 *
 * 内核的 `handleOptionChange`（`lib/terminal.ts:253-327`）没有处理 `renderer`：直接改
 * `options.renderer` 在已打开的终端上不会换渲染器（实测 `rendererType` 仍是 webgl，
 * canvas 节点数也不变）。必须暂停并交出渲染器再恢复，内核才会按新的 options 重新
 * `installRenderer`（`lib/terminal.ts:688-710`）。实测该路径能把 webgl 换成 canvas 并
 * 让 canvas 节点 2→1；反向同样成立。
 *
 * 调用方只在渲染相关设置真的变了时调用它（重装渲染器会重建 glyph atlas，有成本）。
 * 页面隐藏时不抓 GPU 上下文，留给可见性恢复流程（`bindTerminalPageVisibility`）。
 */
export function refreshTerminalRenderer(terminal: AppTerminal): void {
  if (typeof terminal.setPaused !== "function") return;
  if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
  try {
    terminal.setPaused(true, { releaseRenderer: true });
    terminal.setPaused(false, {});
  } catch (error) {
    console.warn("[terminal] 渲染后端切换失败", error);
  }
}

/**
 * 去掉宿主上的 `contenteditable`（自研内核 `open()` 会把它设成 true，见 `lib/terminal.ts:433-435`）。
 *
 * 内核把 `contenteditable` 设在宿主 div 上、并且 `focus()` 聚焦的也是宿主 div
 * （`lib/terminal.ts:950-952` → `this.element.focus()`）。这样焦点落在"可编辑区域"上，
 * WebView2 会把按键先交给编辑/IME 通路：首键可能被编辑宿主吞掉并触发系统提示音
 * （用户实测"敲第一个键会响一声且不显示，第二个才正常"），而 xterm.js 是把焦点放在
 * 隐藏 `<textarea>` 上、走标准表单控件通路的，所以没有这个现象。
 *
 * `open()` 之后调用本函数，并配合 `focusTerminalInput()`，让焦点始终落在隐藏 textarea 上。
 * 宿主上的 `keydown` 监听（`lib/input-handler.ts:283`）靠事件冒泡仍然能收到按键，
 * 因此去掉 `contenteditable` 不影响输入。
 */
export function dropTerminalContentEditable(host: HTMLElement | null | undefined): void {
  host?.removeAttribute("contenteditable");
}

/**
 * 把焦点放到内核的隐藏 `<textarea>` 上（而不是宿主 div）。
 * xterm.js 的 `terminal.focus()` 本来就是聚焦 textarea；自研内核聚焦的是宿主，
 * 这里统一成前者，避免 contenteditable 编辑通路。没有 textarea 时退回 `focus()`。
 */
export function focusTerminalInput(terminal: AppTerminal | null | undefined): void {
  if (!terminal) return;
  const input = terminal.element?.querySelector("textarea");
  if (input instanceof HTMLElement) {
    input.focus();
    return;
  }
  terminal.focus();
}

/**
 * "跟随最新输出"判定：只有视口停在缓冲底部时才自动滚动（开发方案 6.3 表 #7）。
 * 实现在 `terminalScroll.ts`，这里转发一次，让视图层只需依赖适配层。
 * 详细语义说明见该文件（POC V2：自研内核与 xterm 一致，0 = 顶部）。
 */
export { isFollowingLatestLine } from "./terminalScroll";
