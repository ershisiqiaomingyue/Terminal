/**
 * 命令块切块状态机（开发方案 8.3 / 8.4，任务 T2-2、T2-3、T2-4）。
 *
 * 两条路径：
 * 1. **OSC 133 路径**：shell 上报语义提示符时，直接用 `prompt-start` /
 *    `input-start` / `output-start` / `command-end` 切块，退出码由内核给。
 * 2. **前端主导路径**：`adb shell`（mksh / toybox sh）没有 OSC 133，
 *    只能在把输入写进 PTY 之前记账，再从输出里找"回到提示符"。
 *
 * 两路共用的部分：命令文本一律来自**输入流自己记录**的行编辑器，不取
 * `SemanticPromptEvent.command`——附录 B V6 实测该字段为空串。
 *
 * 设计上刻意不含定时器：空闲计时由调用方驱动（`tick`），这样单测可以注入
 * 固定时钟，也不需要 fake timers。
 */
import { BlockStore } from "./blockStore.ts";
import { recordBlockCreated, recordBlockFinished } from "../metrics.ts";
import { decideFinished, matchesPrompt } from "./promptDetect.ts";
import { toPlainText } from "./ansiText.ts";
import type { BlockStatus, BlockTargetKind, CommandBlock } from "./types.ts";

// `modeProbe` 的实现在 promptDetect（视图只需探测内核模式，不必连带整块 store），
// 这里重新导出保持既有导入路径可用。
export { modeProbe } from "./promptDetect.ts";

/** 内核语义事件（对应自研内核 `onSemanticPrompt` 的四值）。 */
export interface SemanticPromptLike {
  kind: "prompt-start" | "input-start" | "output-start" | "command-end";
  exitCode?: number;
  command?: string;
}

export interface TrackerSessionConfig {
  targetKind: BlockTargetKind;
  deviceSerial?: string;
  cwd?: string;
  /** 该会话是否支持 OSC 133；false 时只走前端主导切块。 */
  osc133?: boolean;
}

export interface BlockTrackerOptions {
  store?: BlockStore;
  /** 静默阈值，默认 800 ms。 */
  quiesceMs?: number;
  /** 时间源。 */
  now?: () => number;
  /** 块状态变化时回调（UI 订阅）。 */
  onChange?: (block: CommandBlock, reason: TrackerEvent) => void;
}

export type TrackerEvent =
  | "created"
  | "command"
  | "output"
  | "finished"
  | "interrupted"
  | "passthrough";

/** 行编辑器的单行上限：防止一次粘贴把内存打满。 */
export const MAX_COMMAND_LENGTH = 8 * 1024;

interface SessionRuntime {
  config: TrackerSessionConfig;
  /** 前端主导路径下"已提交但还在跑"的块 id。 */
  activeBlockId: string | null;
  /**
   * OSC 133 会话里"已回车但内核还没发 output-start"的命令文本。
   * 这类会话不能在前端提交时就建块——否则会与 output-start 的块重复，
   * 并把 source 错标成 frontend；命令文本要留到 output-start 再用。
   */
  pendingCommand: string;
  /** 最近一次输出时刻（ms）。 */
  lastOutputAt: number;
  /** 最近 1 秒输出字节累计的滑动窗口采样点。 */
  throughput: { at: number; bytes: number }[];
  passthrough: boolean;
  passthroughReason?: string;
}

/**
 * 从内核的字节流还原"用户敲了什么"。
 *
 * 只消费能确定语义的按键（8.4 的已知局限都写在下面）：
 * - 可打印字符累加；
 * - `\r` / `\n` 提交；
 * - `\x7f` / `\b` 退格；
 * - `\x15`（Ctrl+U）清行；
 * - `\x03`（Ctrl+C）中断当前命令。
 * 其余控制序列（方向键、Tab、ESC）整体忽略——历史命令回溯与行内编辑会被记成
 * "从空开始的新命令"，这是文档里已声明的局限，不做伪精确。
 */
export class CommandLineEditor {
  private buffer = "";

  get value(): string {
    return this.buffer;
  }

  reset(): void {
    this.buffer = "";
  }

  /**
   * 喂一段输入。返回值表示这段输入里发生的动作，调用方按顺序处理。
   */
  feed(input: string): LineEditorAction[] {
    const actions: LineEditorAction[] = [];
    for (let index = 0; index < input.length; index += 1) {
      const char = input[index];
      const code = input.charCodeAt(index);
      if (code === 13 || code === 10) {
        const command = this.buffer.trim();
        this.buffer = "";
        actions.push({ type: "submit", command });
        continue;
      }
      if (code === 3) {
        actions.push({ type: "interrupt" });
        continue;
      }
      if (code === 21) {
        this.buffer = "";
        continue;
      }
      if (code === 127 || code === 8) {
        this.buffer = this.buffer.slice(0, -1);
        continue;
      }
      // ESC 开头的控制序列整体跳过：\x1b[3~ 之类不能把 "[" "3" "~" 当字符记下来。
      if (code === 27) {
        index = skipEscapeSequence(input, index);
        continue;
      }
      // 其余 C0 控制字符（Ctrl+A 等）不参与命令文本。
      if (code < 32) continue;
      if (this.buffer.length >= MAX_COMMAND_LENGTH) continue;
      this.buffer += char;
    }
    return actions;
  }
}

export type LineEditorAction =
  | { type: "submit"; command: string }
  | { type: "interrupt" };

/** 从 ESC 处跳到该控制序列末尾，返回应继续处理的下一个索引。 */
function skipEscapeSequence(input: string, start: number): number {
  const next = input[start + 1];
  if (next === undefined) return start;
  if (next === "[") {
    let index = start + 2;
    while (index < input.length) {
      const code = input.charCodeAt(index);
      if (code >= 0x40 && code <= 0x7e) return index;
      index += 1;
    }
    return input.length - 1;
  }
  if (next === "]") {
    let index = start + 2;
    while (index < input.length) {
      if (input.charCodeAt(index) === 7) return index;
      if (input.charCodeAt(index) === 27 && input[index + 1] === "\\") return index + 1;
      index += 1;
    }
    return input.length - 1;
  }
  return start + 1;
}

export class BlockTracker {
  private readonly store: BlockStore;
  private readonly quiesceMs: number;
  private readonly now: () => number;
  private readonly onChange?: (block: CommandBlock, reason: TrackerEvent) => void;
  private readonly editors = new Map<string, CommandLineEditor>();
  private readonly sessions = new Map<string, SessionRuntime>();

  constructor(options: BlockTrackerOptions = {}) {
    this.store = options.store ?? new BlockStore();
    this.quiesceMs = options.quiesceMs ?? 800;
    this.now = options.now ?? (() => Date.now());
    this.onChange = options.onChange;
  }

  /** 登记一个会话（打开 PTY 之后调用）。重复登记会保留已有块。 */
  registerSession(sessionId: string, config: TrackerSessionConfig): void {
    const existing = this.sessions.get(sessionId);
    if (existing) {
      existing.config = config;
      return;
    }
    this.sessions.set(sessionId, {
      config,
      activeBlockId: null,
      pendingCommand: "",
      lastOutputAt: this.now(),
      throughput: [],
      passthrough: false,
    });
    this.editors.set(sessionId, new CommandLineEditor());
  }

  forgetSession(sessionId: string): void {
    this.sessions.delete(sessionId);
    this.editors.delete(sessionId);
    this.store.forgetSession(sessionId);
  }

  /** 直通模式（8.5）：由视图按内核 mode 探针判定后设置，切断切块与输入面板。 */
  setPassthrough(sessionId: string, passthrough: boolean, reason?: string): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime) return;
    if (runtime.passthrough === passthrough) return;
    runtime.passthrough = passthrough;
    runtime.passthroughReason = passthrough ? reason : undefined;
    if (passthrough) {
      // 进入直通时把未提交的行清掉；正在跑的块保持 running 不动，
      // 只给它打 passthrough 标记，让 UI 不再给它挂 AI 入口。
      this.editors.get(sessionId)?.reset();
      runtime.pendingCommand = "";
      const active = runtime.activeBlockId ? this.store.get(runtime.activeBlockId) : undefined;
      if (!active || active.status !== "running") {
        runtime.activeBlockId = null;
      } else {
        active.passthrough = true;
      }
    }
  }

  isPassthrough(sessionId: string): boolean {
    return this.sessions.get(sessionId)?.passthrough ?? false;
  }

  passthroughReason(sessionId: string): string | undefined {
    return this.sessions.get(sessionId)?.passthroughReason;
  }

  /** 当前编辑中的命令行（UI 可展示）。 */
  pendingLine(sessionId: string): string {
    return this.editors.get(sessionId)?.value ?? "";
  }

  /** 块存储本体（视图导出 markdown / 纯文本时用，不复制数据）。 */
  blockStore(): BlockStore {
    return this.store;
  }

  /**
   * 输入侧接入点（8.4）：由父级 `queueTerminalInput` 调用。
   * 返回值是本次输入产生的动作，调用方不需要处理——这里内部已处理完。
   */
  feedInput(sessionId: string, input: string): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime || !input) return;
    if (runtime.passthrough) return;
    // OSC 133 会话下命令文本仍由这里记录（内核的 cmdline 字段为空串，见 V6）。
    const editor = this.editor(sessionId);
    for (const action of editor.feed(input)) {
      if (action.type === "interrupt") {
        this.interrupt(sessionId);
        continue;
      }
      if (action.type === "submit") this.commit(sessionId, action.command);
    }
  }

  /**
   * 输出侧接入点（8.4）：在 `appendTerminalText` / `term.write` 之前调用，
   * 保证 block 看到的字节顺序与内核一致。
   */
  feedOutput(sessionId: string, data: string): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime || !data) return;
    const now = this.now();
    const bytes = data.length;
    runtime.lastOutputAt = now;
    runtime.throughput.push({ at: now, bytes });
    // 只保留最近 1 s 的采样点。
    while (runtime.throughput.length && now - runtime.throughput[0].at > 1000) runtime.throughput.shift();
    if (runtime.passthrough) return;
    const blockId = runtime.activeBlockId;
    if (!blockId) return;
    const block = this.store.get(blockId);
    if (!block || block.status !== "running") return;
    this.store.appendOutput(blockId, data);
    this.onChange?.(block, "output");
  }

  /**
   * 空闲判定（8.3.2 静默兜底）。调用方按需驱动，例如每 100 ms 一次，
   * 或在一段输出处理完后立即调用一次。返回是否有块被收尾。
   */
  tick(sessionId: string): boolean {
    const runtime = this.sessions.get(sessionId);
    if (!runtime || runtime.passthrough || !runtime.activeBlockId) return false;
    const block = this.store.get(runtime.activeBlockId);
    if (!block || block.status !== "running") return false;
    const decision = decideFinished({
      output: block.outputText,
      target: runtime.config.targetKind,
      idleMs: this.now() - runtime.lastOutputAt,
      quiesceMs: this.quiesceMs,
      recentBytesPerSecond: this.recentBytesPerSecond(runtime),
    });
    if (!decision.finished) return false;
    // 静默判定（quiesce）收尾的块也记 done：source 已经是 frontend，
    // UI 可以据此显示"可能仍在运行"的弱提示（8.3.2），状态不需要再分叉。
    this.finishActive(sessionId, "done");
    return true;
  }

  /** Ctrl+C / 主动中断（后端 interrupt_terminal_session 之后调用）。 */
  interrupt(sessionId: string): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime) return;
    this.editors.get(sessionId)?.reset();
    const blockId = runtime.activeBlockId;
    if (!blockId) return;
    runtime.activeBlockId = null;
    this.store.interrupt(blockId);
    recordBlockFinished("interrupted");
    const block = this.store.get(blockId);
    if (block) this.onChange?.(block, "interrupted");
  }

  /** OSC 7：更新当前目录。 */
  setCwd(sessionId: string, cwd: string): void {
    this.store.setCwd(sessionId, cwd);
    const runtime = this.sessions.get(sessionId);
    if (runtime) runtime.config.cwd = cwd;
  }

  /**
   * OSC 133 路径。返回被创建/结束的块 id，便于视图就地刷新。
   * 若会话被标记为不支持 OSC 133（`osc133: false`），本方法整体忽略，
   * 避免某些 shell 误发 133 序列导致与前端主导路径重复切块。
   */
  handleSemanticPrompt(sessionId: string, event: SemanticPromptLike): string | undefined {
    const runtime = this.sessions.get(sessionId);
    if (!runtime || runtime.passthrough) return undefined;
    // 只有显式声明 osc133: true 的会话才吃这些事件；否则某些 shell 误发 133
    // 会与前端主导切块重复建块。
    if (runtime.config.osc133 !== true) return undefined;
    switch (event.kind) {
      case "prompt-start": {
        // 新提示符出现 = 上一条命令彻底结束（兜底：前端没判定成功时在这里收尾）。
        if (runtime.activeBlockId) {
          const block = this.store.get(runtime.activeBlockId);
          if (block && block.status === "running") this.finishActive(sessionId, "done");
        }
        this.editors.get(sessionId)?.reset();
        runtime.pendingCommand = "";
        return undefined;
      }
      case "input-start":
        this.editors.get(sessionId)?.reset();
        runtime.pendingCommand = "";
        return undefined;
      case "output-start": {
        // 命令开始执行：若还没有块（前端提交动作漏掉），补建一个。
        if (runtime.activeBlockId) {
          const block = this.store.get(runtime.activeBlockId);
          if (block && block.status === "running") return block.id;
        }
        // 命令文本优先用"回车时记下的"，其次退回行编辑器里尚未提交的内容。
        const command = runtime.pendingCommand || (this.editors.get(sessionId)?.value?.trim() ?? "");
        runtime.pendingCommand = "";
        return this.beginBlock(sessionId, command, "osc133").id;
      }
      case "command-end": {
        const blockId = runtime.activeBlockId;
        if (!blockId) return undefined;
        const status: BlockStatus = event.exitCode === undefined || event.exitCode === 0 ? "done" : "failed";
        this.finishActive(sessionId, status, event.exitCode);
        return blockId;
      }
      default:
        return undefined;
    }
  }

  /** 滚动缓冲裁剪后调用（F36）。 */
  markPartialFromLine(sessionId: string, firstValidLine: number): void {
    this.store.markPartialFromLine(sessionId, firstValidLine);
  }

  blocksFor(sessionId: string): CommandBlock[] {
    return this.store.blocksFor(sessionId);
  }

  latest(sessionId: string): CommandBlock | undefined {
    return this.store.latest(sessionId);
  }

  private editor(sessionId: string): CommandLineEditor {
    let editor = this.editors.get(sessionId);
    if (!editor) {
      editor = new CommandLineEditor();
      this.editors.set(sessionId, editor);
    }
    return editor;
  }

  /** 该会话是否走 OSC 133 语义事件路径；未显式开启的一律走前端主导切块。 */
  private usesOsc133(runtime: SessionRuntime): boolean {
    return runtime.config.osc133 === true;
  }

  private commit(sessionId: string, command: string): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime) return;
    // OSC 133 会话：提交时**不建块**，只记下命令文本，等内核的 output-start 再建。
    // 否则会与 output-start 建出的块重复，source 也会被错标成 frontend。
    if (this.usesOsc133(runtime)) {
      runtime.pendingCommand = command;
      return;
    }
    // 新命令开始即旧命令结束：静默判定失败时也不会让两个块重叠（8.3.2 末条）。
    if (runtime.activeBlockId) {
      const previous = this.store.get(runtime.activeBlockId);
      if (previous && previous.status === "running") this.finishActive(sessionId, "done");
    }
    if (!command) return;
    const block = this.beginBlock(sessionId, command, "frontend");
    this.onChange?.(block, "created");
  }

  private beginBlock(sessionId: string, command: string, source: CommandBlock["source"]): CommandBlock {
    const runtime = this.sessions.get(sessionId);
    // 兜底配置必须是完整类型，否则下面访问 deviceSerial / cwd 会在 strict 下报错。
    const config: TrackerSessionConfig = runtime?.config ?? { targetKind: "local" };
    const block = this.store.create({
      sessionId,
      targetKind: config.targetKind,
      deviceSerial: config.deviceSerial,
      command,
      cwd: config.cwd,
      source,
      startedAt: this.now(),
      // 到这里就代表"命令正在执行"，直接进 running（空命令的块也一样）。
      status: "running",
    });
    if (runtime) {
      runtime.activeBlockId = block.id;
      runtime.lastOutputAt = this.now();
    }
    recordBlockCreated();
    return block;
  }

  private finishActive(sessionId: string, status: BlockStatus, exitCode?: number): void {
    const runtime = this.sessions.get(sessionId);
    if (!runtime?.activeBlockId) return;
    const blockId = runtime.activeBlockId;
    runtime.activeBlockId = null;
    this.store.finish(blockId, status, exitCode);
    recordBlockFinished(status);
    const block = this.store.get(blockId);
    if (block) this.onChange?.(block, "finished");
  }

  private recentBytesPerSecond(runtime: SessionRuntime): number {
    const now = this.now();
    let bytes = 0;
    for (const sample of runtime.throughput) {
      if (now - sample.at <= 1000) bytes += sample.bytes;
    }
    return bytes;
  }
}

/** 输出是否是"明显的命令回显"（用于把命令自身那行从 output 里去掉）。 */
export function stripCommandEcho(output: string, command: string): string {
  if (!command) return output;
  const plain = toPlainText(output);
  const escaped = command.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(`^[^\\n]*${escaped}[^\\n]*\\n?`);
  return plain.replace(pattern, "");
}

/** 视图侧：把"提示符已出现"这层信息也暴露出去（部分 UI 需要立刻收尾）。 */
export function outputHitsPrompt(output: string, target: BlockTargetKind): boolean {
  return matchesPrompt(toPlainText(output), target);
}
