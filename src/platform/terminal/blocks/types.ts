/**
 * 命令块（Command Block）的数据结构（开发方案 8.2）。
 *
 * 这一层是 UI 与 AI 的唯一数据源：UI 用它渲染可折叠的块，AI 用它组装 prompt。
 * 它不依赖终端内核的具体实现，只消费三路输入——输出字节流、输入字节流、
 * 内核语义事件（OSC 133 / OSC 7）。
 *
 * 设计约束（都在 8.2 里给了理由）：
 * 1. `command` 不取 `SemanticPromptEvent.command`：OSC 133 的 `cmdline` 扩展
 *    并非所有 shell 都上报（附录 B V6 实测为空串），统一由输入侧自己记录；
 * 2. `outputText` 存去 ANSI 后的纯文本，不存原始字节（体积差 3–10 倍）；
 * 3. `rawRange` 只在创建时记录，行号会随 scrollback 裁剪失效，用前必须校验。
 */

/** 会话目标类型，与后端 `TerminalTarget` 的四类一一对应。 */
export type BlockTargetKind = "local" | "wsl" | "ssh" | "adb";

export type BlockStatus =
  /** 已创建但用户还没提交（OSC 133 的 prompt-start 之后）。 */
  | "idle"
  /** 命令已提交、输出还在流。 */
  | "running"
  | "done"
  | "failed"
  /** 用户 Ctrl+C 或主动中断。 */
  | "interrupted"
  /** 所属行区间已被 scrollback 裁掉，输出不完整。 */
  | "partial";

/** 切块依据：有 OSC 133 的 shell 用语义事件，其余用前端主导切块。 */
export type BlockSource = "osc133" | "frontend";

export interface BlockRawRange {
  /** 内核 buffer 的绝对起始行（block 创建时刻的快照）。 */
  startLine: number;
  /** 结束行；仍在运行时为 undefined。 */
  endLine?: number;
}

/** 一次命令执行的结构化记录（8.2）。 */
export interface CommandBlock {
  /** 稳定 id，作为 UI 的 React key 与 AI 引用句柄。 */
  id: string;
  sessionId: string;
  targetKind: BlockTargetKind;
  /** `targetKind === "adb"` 时存在。 */
  deviceSerial?: string;
  /** 命令原文（不含提交用的回车）。 */
  command: string;
  /** 当前工作目录，来自 OSC 7 或会话初始目录。 */
  cwd?: string;
  status: BlockStatus;
  /** 来自 OSC 133 的 command-end，或前端判定（0 / undefined）。 */
  exitCode?: number;
  /** 毫秒时间戳。 */
  startedAt: number;
  endedAt?: number;
  /** 去 ANSI 后的纯文本（已按块预算裁剪）。 */
  outputText: string;
  /** 输出是否因超限被从头裁剪过。 */
  outputTruncated: boolean;
  rawRange?: BlockRawRange;
  source: BlockSource;
  /** 交互式程序（直通模式）下不切块，用此标记让 UI 不做 AI 入口。 */
  passthrough?: boolean;
}

/** 单个块允许保留的输出预算；超出保留尾部（8.6）。 */
export const MAX_BLOCK_OUTPUT_BYTES = 64 * 1024;
export const MAX_BLOCK_OUTPUT_LINES = 1000;
/** 每个会话允许保留的块数上限，超出从最旧处丢弃。 */
export const MAX_BLOCKS_PER_SESSION = 500;

/** 终态集合：到这些状态后不再接受输出追加。 */
export const TERMINAL_BLOCK_STATUSES: readonly BlockStatus[] = ["done", "failed", "interrupted", "partial"];

export function isTerminalStatus(status: BlockStatus): boolean {
  return TERMINAL_BLOCK_STATUSES.includes(status);
}

/** 只有失败或中断的块才值得提示"要诊断吗"（F41：不自动刷屏）。 */
export function isFailableStatus(status: BlockStatus): boolean {
  return status === "failed" || status === "interrupted";
}
