/**
 * 命令块的存储与裁剪（开发方案 8.6，任务 T2-1）。
 *
 * 职责边界：本文件只管"数据"——创建、追加输出、裁剪、结束、按会话索引、导出。
 * "什么时候该结束"的判定在 `blockTracker.ts`（那里才有提示符正则与静默判定），
 * 两者拆开是为了让存储与判定都能各自进单测（R-45）。
 *
 * 裁剪策略与 `terminalBuffer.ts` 一致：**保留尾部，从头部丢弃**，并置
 * `outputTruncated = true`。理由是 AI 诊断最需要"最后报了什么错"。
 * 体积按 UTF-8 字节算（中文 3 字节/字），不能用 `string.length`（8.6 明确要求）。
 */
import { toPlainText, truncateText, utf8ByteLength } from "./ansiText.ts";
import {
  MAX_BLOCKS_PER_SESSION,
  MAX_BLOCK_OUTPUT_BYTES,
  MAX_BLOCK_OUTPUT_LINES,
  isTerminalStatus,
  type BlockStatus,
  type BlockTargetKind,
  type CommandBlock,
  type BlockSource,
} from "./types.ts";

export interface CreateBlockInput {
  sessionId: string;
  targetKind: BlockTargetKind;
  command?: string;
  cwd?: string;
  source?: BlockSource;
  startedAt?: number;
  /** 初始状态；不传时按"有没有命令文本"推断。 */
  status?: BlockStatus;
  /** 内核 buffer 起始行（前端主导切块时记录，用于跳转定位）。 */
  startLine?: number;
}

export interface BlockStoreOptions {
  maxBlocksPerSession?: number;
  maxOutputBytes?: number;
  maxOutputLines?: number;
  /** 时间源，单测注入固定时钟。 */
  now?: () => number;
  /** id 生成器，单测注入稳定序列。 */
  createId?: (sessionId: string, sequence: number) => string;
}

/** 未被裁剪时的追加阈值冗余：避免每个 chunk 都整串重算字节数。 */
const TRIM_SLACK_RATIO = 1.25;

interface SessionState {
  blocks: CommandBlock[];
  nextSequence: number;
}

function defaultId(sessionId: string, sequence: number): string {
  return `${sessionId}#${sequence}`;
}

/** 数换行符。`lineCount` 与增量维护共用，保证两处口径一致。 */
function countNewlines(text: string): number {
  if (!text) return 0;
  let count = 0;
  for (let index = 0; index < text.length; index += 1) {
    if (text.charCodeAt(index) === 10) count += 1;
  }
  return count;
}

/**
 * 按会话保存命令块。所有方法都是同步的——它们跑在输出热路径上（8.4），
 * 不能做异步或复制整个数据流。
 */
export class BlockStore {
  private readonly sessions = new Map<string, SessionState>();
  private readonly maxBlocksPerSession: number;
  private readonly maxOutputBytes: number;
  private readonly maxOutputLines: number;
  private readonly now: () => number;
  private readonly createId: (sessionId: string, sequence: number) => string;
  /** 每块已计入的 UTF-8 字节数（与 block.outputText 同步维护）。 */
  private readonly outputBytes = new Map<string, number>();
  /** 每块已计入的行数；增量维护，避免每个 chunk 都整串数换行（热路径）。 */
  private readonly outputLines = new Map<string, number>();

  constructor(options: BlockStoreOptions = {}) {
    this.maxBlocksPerSession = Math.max(1, Math.floor(options.maxBlocksPerSession ?? MAX_BLOCKS_PER_SESSION));
    this.maxOutputBytes = Math.max(0, Math.floor(options.maxOutputBytes ?? MAX_BLOCK_OUTPUT_BYTES));
    this.maxOutputLines = Math.max(1, Math.floor(options.maxOutputLines ?? MAX_BLOCK_OUTPUT_LINES));
    this.now = options.now ?? (() => Date.now());
    this.createId = options.createId ?? defaultId;
  }

  /** 创建一个新块（尚未收到输出）。 */
  create(input: CreateBlockInput): CommandBlock {
    const state = this.state(input.sessionId);
    const sequence = state.nextSequence++;
    const block: CommandBlock = {
      id: this.createId(input.sessionId, sequence),
      sessionId: input.sessionId,
      targetKind: input.targetKind,
      command: input.command ?? "",
      cwd: input.cwd,
      status: input.status ?? (input.command ? "running" : "idle"),
      startedAt: input.startedAt ?? this.now(),
      outputText: "",
      outputTruncated: false,
      source: input.source ?? "frontend",
      rawRange: input.startLine === undefined ? undefined : { startLine: input.startLine },
    };
    state.blocks.push(block);
    this.outputBytes.set(block.id, 0);
    this.outputLines.set(block.id, 0);
    this.trimSession(state);
    return block;
  }

  get(blockId: string): CommandBlock | undefined {
    for (const state of this.sessions.values()) {
      const found = state.blocks.find((block) => block.id === blockId);
      if (found) return found;
    }
    return undefined;
  }

  /** 某会话的全部块（按创建顺序，最旧在前）。 */
  blocksFor(sessionId: string): CommandBlock[] {
    return this.sessions.get(sessionId)?.blocks ?? [];
  }

  /** 最近一个块；没有则 undefined。 */
  latest(sessionId: string): CommandBlock | undefined {
    const blocks = this.blocksFor(sessionId);
    return blocks.length ? blocks[blocks.length - 1] : undefined;
  }

  /** 最近一个仍在跑的块。 */
  running(sessionId: string): CommandBlock | undefined {
    const blocks = this.blocksFor(sessionId);
    for (let index = blocks.length - 1; index >= 0; index -= 1) {
      if (blocks[index].status === "running") return blocks[index];
    }
    return undefined;
  }

  setCommand(blockId: string, command: string): void {
    const block = this.get(blockId);
    if (!block || isTerminalStatus(block.status)) return;
    block.command = command;
    if (block.status === "idle" && command) block.status = "running";
  }

  /**
   * 追加一段原始输出（含 ANSI）。内部做去 ANSI 与按阈值裁剪。
   * 已结束的块不再接收输出（避免 Ctrl+C 之后迟到的输出污染已定稿的块）。
   */
  appendOutput(blockId: string, raw: string): void {
    if (!raw) return;
    const block = this.get(blockId);
    if (!block || isTerminalStatus(block.status)) return;
    const plain = toPlainText(raw);
    if (!plain) return;
    block.outputText += plain;
    const bytes = (this.outputBytes.get(blockId) ?? 0) + utf8ByteLength(plain);
    this.outputBytes.set(blockId, bytes);
    // 行数增量维护：输出热路径上不能每个 chunk 都整串数一遍换行。
    const lines = (this.outputLines.get(blockId) ?? 0) + countNewlines(plain);
    this.outputLines.set(blockId, lines);
    // 留出冗余再裁剪：每个 chunk 都整串切一次太贵，超过阈值 1.25 倍才动手。
    if (bytes > this.maxOutputBytes * TRIM_SLACK_RATIO || lines > this.maxOutputLines * TRIM_SLACK_RATIO) {
      this.trimBlock(block);
    }
  }

  /** 结束一个块并落定状态与退出码。已在终态的块不会被改写。 */
  finish(blockId: string, status: BlockStatus, exitCode?: number): void {
    const block = this.get(blockId);
    if (!block || isTerminalStatus(block.status)) return;
    this.trimBlock(block);
    block.status = status;
    block.exitCode = exitCode;
    block.endedAt = this.now();
    if (block.rawRange) block.rawRange.endLine = block.rawRange.startLine + this.lineCount(block.outputText);
  }

  /** Ctrl+C / 主动中断：记为 interrupted，不写退出码。 */
  interrupt(blockId: string): void {
    this.finish(blockId, "interrupted");
  }

  /** 更新工作目录（OSC 7）。 */
  setCwd(sessionId: string, cwd: string): void {
    if (!cwd) return;
    const latest = this.running(sessionId) ?? this.latest(sessionId);
    if (latest) latest.cwd = cwd;
  }

  /**
   * scrollback 裁剪后调用：把行号已失效的块标记为 partial（F36）。
   * 注意这里**要**覆盖已结束的块：`done` 只说明命令跑完了，行被裁掉后
   * 它的 `rawRange` 已经指向不存在的行，继续保留可跳转的假象会让 UI 越界读取。
   */
  markPartialFromLine(sessionId: string, firstValidLine: number): void {
    for (const block of this.blocksFor(sessionId)) {
      if (!block.rawRange) continue;
      if (block.rawRange.startLine >= firstValidLine) continue;
      block.status = "partial";
      block.rawRange = undefined;
    }
  }

  /** 清空某会话的块（清屏 / 重连时用）。 */
  clearSession(sessionId: string): void {
    const state = this.sessions.get(sessionId);
    if (!state) return;
    for (const block of state.blocks) {
      this.outputBytes.delete(block.id);
      this.outputLines.delete(block.id);
    }
    state.blocks = [];
  }

  /** 移除会话（会话关闭时用，连同块一起丢弃）。 */
  forgetSession(sessionId: string): void {
    this.clearSession(sessionId);
    this.sessions.delete(sessionId);
  }

  /** 把块导出为 markdown（供聊天引用，F37）。 */
  exportMarkdown(blockId: string): string {
    const block = this.get(blockId);
    if (!block) return "";
    const header = `$ ${block.command || "(空命令)"}`;
    const meta = [
      `- 会话：${block.targetKind}`,
      block.cwd ? `- 目录：${block.cwd}` : "",
      `- 状态：${block.status}${block.exitCode === undefined ? "" : `（退出码 ${block.exitCode}）`}`,
      block.outputTruncated ? "- 注意：输出已被裁剪，仅保留尾部" : "",
    ].filter(Boolean);
    const body = block.outputText.trimEnd();
    return [`\`\`\``, header, `\`\`\``, "", meta.join("\n"), "", body ? `\`\`\`\n${body}\n\`\`\`` : "（无输出）"].join("\n");
  }

  /** 导出为纯文本（AI prompt 与剪贴板用）。 */
  exportText(blockId: string): string {
    const block = this.get(blockId);
    if (!block) return "";
    const head = `$ ${block.command || ""}`.trim();
    const tail = block.exitCode === undefined ? "" : `\n[exit ${block.exitCode}]`;
    return `${head}\n${block.outputText.trimEnd()}${tail}`.trim();
  }

  private state(sessionId: string): SessionState {
    let state = this.sessions.get(sessionId);
    if (!state) {
      state = { blocks: [], nextSequence: 0 };
      this.sessions.set(sessionId, state);
    }
    return state;
  }

  private lineCount(text: string): number {
    return countNewlines(text);
  }

  /** 按行数与字节数裁剪单个块的输出，保留尾部。 */
  private trimBlock(block: CommandBlock): void {
    const before = block.outputText;
    const { text, truncated } = truncateText(before, {
      maxLines: this.maxOutputLines,
      maxBytes: this.maxOutputBytes,
      keepTail: true,
    });
    if (!truncated) return;
    block.outputText = text;
    block.outputTruncated = true;
    this.outputBytes.set(block.id, utf8ByteLength(text));
    this.outputLines.set(block.id, countNewlines(text));
  }

  /** 会话块数超限时从最旧处丢弃（可丢的优先丢已结束的）。 */
  private trimSession(state: SessionState): void {
    while (state.blocks.length > this.maxBlocksPerSession) {
      let index = state.blocks.findIndex((block) => isTerminalStatus(block.status));
      if (index < 0) index = 0;
      const [removed] = state.blocks.splice(index, 1);
      if (!removed) break;
      this.outputBytes.delete(removed.id);
      this.outputLines.delete(removed.id);
    }
  }
}
