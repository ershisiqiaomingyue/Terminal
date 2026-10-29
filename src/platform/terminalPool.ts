/**
 * 终端实例池（开发方案 6.7.2「WASM 内存不可收缩」与 14 章 RK7）。
 *
 * 自研内核（WASM）的线性内存只增不减：实测每创建一个实例就永久多占
 * ≈0.48 MB（WebGL）/ ≈0.13 MB（Canvas），`dispose()` 与清空 DOM 都不归还，
 * 150 次"创建 → 销毁"后堆线性增长且不收敛（见性能审计附录 C）。而视图侧很爱开关
 * 实例：`ChatTerminalPanel` 的构造 effect 依赖 `[activeId, visible]`，每切一次
 * 终端页签都会重建一个实例。
 *
 * 因此这里加一层"寄存"：视图卸载时不销毁实例，而是暂停渲染、交出 WebGL 上下文、
 * 把宿主容器摘出 DOM 放进池里；下次挂载优先复用同内核的实例。
 *
 * 为什么必须连宿主容器一起持有：库的 `open()` 在已打开的实例上会抛
 * `Terminal is already open`（自研内核 `lib/terminal.ts:416-418`），
 * 所以复用只能复用"已经 open 在自己容器里"的实例，再把这个容器挂进当前视图宿主。
 */
import {
  createAppTerminal,
  dropTerminalContentEditable,
  readTerminalEngine,
  type AppFitAddon,
  type AppTerminal,
  type TerminalEngine,
  type TerminalOptions,
} from "./terminalAdapter";
import { guardTerminalKeydown } from "./terminalKeys";
import { resetTerminalContent } from "./terminalReset";

/**
 * 池里最多寄存几个实例；超出的真正销毁（内存上界 ≈0.5 MB × 这个数）。
 *
 * 取值 4 的依据：中栏最多四屏（2x2），四块面板同时挂载时各自持有一份 lease，
 * 切回小布局或换绑页签时这 4 份都会进池。若仍取 2，四屏 ↔ 单屏往返会立刻
 * 把实例销毁重建，而自研内核的 WASM 内存不可回收（见 terminalAdapter 实测）。
 */
const MAX_PARKED = 4;

/** 允许同时存在的实例上界：面板数（≤4）+ 寄存数（4）。 */
export const TERMINAL_LIVE_LIMIT = MAX_PARKED + 4;

/**
 * 当前存活实例数（已借出 + 寄存）。结构上不会超过 `TERMINAL_LIVE_LIMIT`
 * （面板数与池容量都有限），这里计数并在超限时告警，是给"lease 泄漏"
 * （视图忘记 release）这类 bug 一个能在控制台直接看到的信号。
 */
let liveInstances = 0;

export function liveTerminalCount(): number {
  return liveInstances;
}

interface ParkedTerminal {
  engine: TerminalEngine;
  container: HTMLDivElement;
  terminal: AppTerminal;
  fit: AppFitAddon;
  /** 宿主上的按键兜底解绑函数（见 terminalKeys.ts）。 */
  unbindKeys: () => void;
}

const parked: ParkedTerminal[] = [];

export interface TerminalLease {
  terminal: AppTerminal;
  fit: AppFitAddon;
  engine: TerminalEngine;
  /** 视图卸载时归还：默认寄存进池；`{ dispose: true }` 或池满时真正销毁。 */
  release: (options?: { dispose?: boolean }) => void;
}

function createContainer(engine: TerminalEngine): HTMLDivElement {
  const container = document.createElement("div");
  container.className = "terminal-engine-host";
  container.dataset.terminalEngine = engine;
  return container;
}

function disposeEntry(entry: ParkedTerminal): void {
  entry.unbindKeys();
  try {
    entry.terminal.dispose();
  } catch (error) {
    console.warn("[terminal] 终端实例销毁失败", error);
  }
  entry.container.remove();
  liveInstances = Math.max(0, liveInstances - 1);
}

function park(entry: ParkedTerminal): void {
  // 寄存期不该继续绘制，也不该占着 GPU：暂停并交出 WebGL 上下文。
  try {
    entry.terminal.setPaused?.(true, { releaseRenderer: true });
  } catch (error) {
    console.warn("[terminal] 终端暂停失败", error);
  }
  entry.container.remove();
  while (parked.length >= MAX_PARKED) {
    const victim = parked.shift();
    if (victim) disposeEntry(victim);
  }
  parked.push(entry);
}

function leaseOf(entry: ParkedTerminal): TerminalLease {
  return {
    terminal: entry.terminal,
    fit: entry.fit,
    engine: entry.engine,
    release: (options) => {
      if (options?.dispose) disposeEntry(entry);
      else park(entry);
    },
  };
}

async function createFresh(
  build: (engine: TerminalEngine) => TerminalOptions,
  parent: HTMLElement,
): Promise<TerminalLease> {
  const { terminal, fit, engine } = await createAppTerminal(build);
  liveInstances += 1;
  if (liveInstances > TERMINAL_LIVE_LIMIT) {
    // 自研内核的 WASM 内存只增不减（每实例 ≈0.48 MB），超限大概率是某处
    // 拿了 lease 没 release——池寄存只会腾位置不会减实例数。
    console.warn(
      `[terminal] 存活终端实例 ${liveInstances} 超过上界 ${TERMINAL_LIVE_LIMIT}，疑似存在未归还的 lease`,
    );
  }
  const container = createContainer(engine);
  terminal.loadAddon(fit);
  terminal.open(container);
  // open() 会把宿主设成 contenteditable，让 WebView2 走可编辑/IME 通路（首键被吞 + 响一声）。
  // 去掉它，焦点统一落在隐藏 textarea 上（见 terminalAdapter.dropTerminalContentEditable）。
  dropTerminalContentEditable(container);
  // 焦点万一落回宿主本身（内核 focus() 的兜底分支），也要把按键留在应用内，别让宿主窗口响提示音。
  const unbindKeys = guardTerminalKeydown(container);
  parent.appendChild(container);
  return leaseOf({ engine, container, terminal, fit, unbindKeys });
}

/**
 * 取一个终端实例：优先复用池里同内核的寄存实例，否则新建。
 * 返回的宿主容器已经挂到 `parent` 下，调用方不需要再 `open()`。
 */
export async function acquireTerminal(
  build: (engine: TerminalEngine) => TerminalOptions,
  parent: HTMLElement,
): Promise<TerminalLease> {
  const desired = readTerminalEngine();
  const index = parked.findIndex((entry) => entry.engine === desired);
  if (index >= 0) {
    const [entry] = parked.splice(index, 1);
    // 选项（含 renderer/scrollbar/主题/字体）在恢复前赋值即可生效：寄存时交出了 WebGL，
    // 恢复会按新 options 重装渲染器（实测 webgl → canvas、canvas → webgl 都会重建）。
    const options = build(entry.engine);
    Object.assign(entry.terminal.options, options);
    // 顺序很关键：先把容器挂回 DOM，再恢复绘制。
    // `setPaused(false)` 内部会 `draw(true)` 全屏重绘，如果此刻容器还没挂上父级，
    // 内核拿不到布局尺寸，重绘落在"零尺寸"状态；随后视图的 fit 又可能因为父级
    // 尚未完成布局而读到 0，两边都不纠正就表现为整块空白终端。
    parent.appendChild(entry.container);
    // 清空上一轮内容：视图随后会重放自己的回放缓冲。这里必须用 `resetTerminalContent`
    // 而不是内核的 `reset()`：后者会换掉 WASM 句柄，而 InputHandler/SelectionManager
    // 仍持有旧句柄，鼠标事件会抛内核句柄失效（result -2）异常（见 terminalReset.ts）。
    resetTerminalContent(entry.terminal);
    try {
      entry.terminal.setPaused?.(false, {});
    } catch (error) {
      console.warn("[terminal] 终端恢复失败", error);
    }
    // 若寄存时是从 canvas 状态收的（例如用户先把渲染后端切成 canvas），内核不会重装
    // 渲染器（`setPaused` 只在释放过 WebGL 时才重建，见 lib/terminal.ts:688-710），
    // 复用会永远停在 canvas；这种情况丢弃这个实例重新建。
    const mismatched =
      options.renderer !== undefined && entry.terminal.rendererType !== undefined && entry.terminal.rendererType !== options.renderer;
    if (mismatched) {
      disposeEntry(entry);
      return createFresh(build, parent);
    }
    return leaseOf(entry);
  }
  return createFresh(build, parent);
}

/** 当前寄存了几个实例（诊断/测试用）。 */
export function parkedTerminalCount(): number {
  return parked.length;
}

/** 清空池（诊断/测试/退出用）。 */
export function disposeParkedTerminals(): void {
  while (parked.length) {
    const entry = parked.pop();
    if (entry) disposeEntry(entry);
  }
}
