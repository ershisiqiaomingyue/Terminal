/**
 * 提示符识别与"命令是否还在跑"的判定（开发方案 8.3.2 / 8.5）。
 *
 * 没有 OSC 133 集成的 shell 只能从前端视角判断"输出回到提示符了"= 命令结束。
 * 这里把判定全部做成纯函数，Node 单测可直接跑（内核与 DOM 在测试进程里都装不起来，
 * 见 7.2.6 R-47 的教训）。
 */
import type { BlockTargetKind } from "./types.ts";
import { applyCarriageReturns } from "./ansiText.ts";

/** 提示符正则表；顺序即优先级，命中任一即认为到了新提示符。 */
const PROMPT_PATTERNS: Record<BlockTargetKind, RegExp[]> = {
  local: [
    // pwsh：`PS C:\Users\name>`，带可选尾部空格
    /(?:^|\n)PS [A-Za-z]:\\[^\n]*>\s*$/i,
    // cmd：`C:\path>`；PowerShell 无 PS 前缀的自定义提示符
    /(?:^|\n)[A-Za-z]:\\[^\n]{0,200}>\s*$/,
    // PowerShell 默认带 `>`，兼容 `PS>`（无路径）
    /(?:^|\n)PS>\s*$/i,
  ],
  wsl: [
    // bash / zsh / fish：`user@host:~$ `、`root@host:/# `
    /(?:^|\n)[\w.\-]+@[\w.\-]+:[^\n]{0,200}?[$#]\s*$/,
    /(?:^|\n)[\w.\-@:/~]*\s[$#]\s*$/,
    /(?:^|\n)➜\s+[^\n]*$/,
  ],
  ssh: [
    /(?:^|\n)[\w.\-]+@[\w.\-]+:[^\n]{0,200}?[$#]\s*$/,
    /(?:^|\n)[\w.\-@:/~]*\s[$#]\s*$/,
    /(?:^|\n)[^\n]{1,120}\s[$#]\s*$/,
  ],
};

/** 末尾是这些字符的行，才可能是提示符（静默判定的前置条件）。 */
const PROMPT_TAIL = /[$#%>](?:\s*)$/;

/** 「命令还在跑」的特征：命中任一条就不能判定结束（8.3.2）。 */
const RUNNING_PATTERNS: RegExp[] = [
  /\[[Yy]\/[Nn]\]\s*$/, // 交互式确认
  /\(yes\/no\)\s*$/i,
  /[Pp]assword(?: for [^\s:]+)?:\s*$/, // 密码提示
  /--More--\s*$/i, // 分页器
  /Press any key/i,
  /Continue\?\s*$/i,
  /Do you want to continue/i,
];

/** 输出尾部最后一个非空行（已应用 `\r` 覆盖语义）。 */
export function lastOutputLine(text: string): string {
  if (!text) return "";
  const lines = text.split("\n");
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    // 不能直接 `replace(/\r.*$/, "")`：进度条那种 `\r` 覆盖行里，
    // 真正显示的是最后一段，截断到第一个 `\r` 会把有用内容丢掉。
    const line = applyCarriageReturns(lines[index]).trimEnd();
    if (line.trim()) return line;
  }
  return "";
}

/** 输出尾部的全部内容是否匹配某类 target 的提示符。 */
export function matchesPrompt(text: string, target: BlockTargetKind): boolean {
  if (!text) return false;
  const line = lastOutputLine(text);
  if (!line) return false;
  return PROMPT_PATTERNS[target].some((pattern) => pattern.test(`\n${line}`) || pattern.test(line));
}

/** 尾部是否带"命令仍在进行中"的特征。 */
export function looksRunning(text: string): boolean {
  if (!text) return false;
  const tail = text.slice(-2048).replace(/\r/g, "\n");
  if (RUNNING_PATTERNS.some((pattern) => pattern.test(tail))) return true;
  // 进度条覆盖：`\r` 后仍在更新（最后 200 字符里有 \r 且不是行尾）
  const recent = text.slice(-200);
  if (recent.indexOf("\r") >= 0 && !recent.endsWith("\r")) return true;
  return false;
}

export interface FinishDecisionInput {
  /** 自命令开始以来累积的输出（纯文本）。 */
  output: string;
  target: BlockTargetKind;
  /** 距离最后一次输出的毫秒数。 */
  idleMs: number;
  /** 静默阈值，默认 800 ms。 */
  quiesceMs?: number;
  /** 最近 1 秒输出字节数（用于排除高速流）。 */
  recentBytesPerSecond?: number;
  /** 内核是否处于 alt screen / 鼠标追踪等交互态。 */
  interactive?: boolean;
}

/** 静默判定的默认阈值：800 ms 无输出且尾部像提示符才收尾。 */
export const DEFAULT_QUIESCE_MS = 800;
/** 超过这个速率视为持续输出，不判定结束。 */
export const HIGH_THROUGHPUT_BYTES_PER_SECOND = 10 * 1024;

export interface FinishDecision {
  finished: boolean;
  /** 判定依据，便于埋点统计切块准确率（N8）。 */
  reason: "prompt" | "quiesce" | "running" | "idle" | "interactive";
}

/**
 * 判定一条命令是否结束。三条出口：命中提示符、静默且尾部像提示符、否则继续跑。
 * 顺序按"确定性从高到低"：提示符 > 运行特征 > 静默。
 */
export function decideFinished(input: FinishDecisionInput): FinishDecision {
  if (input.interactive) return { finished: false, reason: "interactive" };
  if (matchesPrompt(input.output, input.target)) return { finished: true, reason: "prompt" };
  if (looksRunning(input.output)) return { finished: false, reason: "running" };
  if ((input.recentBytesPerSecond ?? 0) > HIGH_THROUGHPUT_BYTES_PER_SECOND) {
    return { finished: false, reason: "running" };
  }
  const quiesce = input.quiesceMs ?? DEFAULT_QUIESCE_MS;
  if (input.idleMs < quiesce) return { finished: false, reason: "idle" };
  // 静默够了，但尾部不像提示符（例如只输出了一半的错误）→ 继续等。
  if (!PROMPT_TAIL.test(lastOutputLine(input.output))) return { finished: false, reason: "idle" };
  return { finished: true, reason: "quiesce" };
}

/**
 * 直通模式判定所需的内核能力（结构化满足，便于单测传假对象）。
 * 对应自研内核的 `hasMouseTracking` / `hasFocusEvents` / `hasBracketedPaste` / `getMode`。
 */
export interface TerminalModeProbe {
  hasMouseTracking?: () => boolean;
  hasFocusEvents?: () => boolean;
  hasBracketedPaste?: () => boolean;
  getMode?: (mode: number, isAnsi?: boolean) => boolean;
}

/** alt screen 的两种模式号（1049 与 47/1047 老式写法）。 */
export const ALT_SCREEN_MODES: readonly number[] = [1049, 1047, 47];

export interface PassthroughInput {
  terminal?: TerminalModeProbe | null;
  /** 最近 1 秒输出字节数。 */
  recentBytesPerSecond?: number;
  /** 用户显式开关（Ctrl+G 退出后为 false）。 */
  enabled?: boolean;
}

export interface PassthroughDecision {
  passthrough: boolean;
  /** 命中的原因，用于 UI 提示"当前程序需要直接输入"。 */
  reason?: "mouse" | "focus" | "bracketed-paste" | "alt-screen" | "throughput" | "disabled";
}

/**
 * 交互式程序判定（8.5 / F54）：命中任一条件即进入直通模式，不切块、不走输入面板。
 * 注意：`hasBracketedPaste` 单独不能说明是交互式程序（bash 默认就开），
 * 因此它只在与其他条件同时成立时才计入——这里按"与 mouse/alt-screen 同组"处理，
 * 单独出现时**不**触发直通，避免把正常 shell 误判成交互式。
 */
export function decidePassthrough(input: PassthroughInput): PassthroughDecision {
  if (input.enabled === false) return { passthrough: false, reason: "disabled" };
  if ((input.recentBytesPerSecond ?? 0) > HIGH_THROUGHPUT_BYTES_PER_SECOND) {
    return { passthrough: true, reason: "throughput" };
  }
  const terminal = input.terminal;
  if (!terminal) return { passthrough: false };
  if (terminal.hasMouseTracking?.()) return { passthrough: true, reason: "mouse" };
  if (terminal.hasFocusEvents?.()) return { passthrough: true, reason: "focus" };
  if (terminal.getMode) {
    for (const mode of ALT_SCREEN_MODES) {
      if (terminal.getMode(mode, true)) return { passthrough: true, reason: "alt-screen" };
    }
  }
  return { passthrough: false };
}

/** 供 UI 提示用的文案（与 9.3.4 一致）。 */
export function passthroughHint(reason: PassthroughDecision["reason"]): string | null {
  switch (reason) {
    case "mouse":
    case "focus":
    case "alt-screen":
      return "当前程序需要直接输入，自然语言输入面板已暂停。";
    case "throughput":
      return "输出量很大，已暂停命令切块。";
    default:
      return null;
  }
}

/**
 * 把内核实例包成 `TerminalModeProbe`（视图侧用）。
 *
 * 放在本文件而不是 `blockTracker.ts`：视图只需要"探测内核模式"这一件事，
 * 不该为此把整个 blockStore / 切块状态机一起拉进 bundle。
 *
 * **每个方法都必须吞异常**：自研内核实例被池回收、或被 `reset()` 换掉底层
 * WASM 句柄后，内核句柄失效（`failed with result -2`）会抛异常
 * （线上日志里刷过几百行）。探针是每秒轮询的"尽力而为"信息，
 * 读不到就当作"不是交互式"，绝不能让探测把渲染打崩。
 */
export function modeProbe(terminal: unknown): TerminalModeProbe | null {
  if (!terminal || typeof terminal !== "object") return null;
  const candidate = terminal as Record<string, unknown>;
  const probe: TerminalModeProbe = {};
  const safe = (fn: () => boolean): boolean => {
    try {
      return Boolean(fn());
    } catch {
      return false;
    }
  };
  if (typeof candidate.hasMouseTracking === "function") {
    probe.hasMouseTracking = () => safe(() => (candidate.hasMouseTracking as () => boolean).call(terminal));
  }
  if (typeof candidate.hasFocusEvents === "function") {
    probe.hasFocusEvents = () => safe(() => (candidate.hasFocusEvents as () => boolean).call(terminal));
  }
  if (typeof candidate.hasBracketedPaste === "function") {
    probe.hasBracketedPaste = () => safe(() => (candidate.hasBracketedPaste as () => boolean).call(terminal));
  }
  if (typeof candidate.getMode === "function") {
    probe.getMode = (mode: number, isAnsi?: boolean) =>
      safe(() => (candidate.getMode as (mode: number, isAnsi?: boolean) => boolean).call(terminal, mode, isAnsi));
  }
  return Object.keys(probe).length ? probe : null;
}
