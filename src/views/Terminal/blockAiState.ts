/**
 * 命令块 AI 卡片的**纯状态与文本处理**（开发方案 9.2.4 / 任务 T3-4）。
 *
 * 与 `BlockAiPanel.tsx` 分开的原因和 service 层一样：这里不 import 任何 React
 * 或 `@/platform/*`，可以用 `node --experimental-strip-types` 直接单测。
 * 组件只负责把这里的 state 画出来、把用户动作翻译成 action。
 *
 * 三件事：
 * 1. 卡片状态机（idle → loading → streaming → done/error），带取消与追问；
 * 2. 折叠态的一行摘要：从模型返回的 markdown 里抽出"结论那一句"；
 * 3. 建议命令提取（F44）：把诊断结果里的代码块变成可一键插入的命令。
 */
// 相对路径而非 `@/` 别名：本文件要能被 `node --experimental-strip-types`
// 直接 import 做单测，Node 不认识 Vite 的路径别名。
import type { BlockAiErrorCode, BlockAiOutcome } from "../../platform/terminal/ai/terminalAiService.ts";
import type { TerminalAiMode } from "../../platform/terminal/ai/prompts.ts";

/** 卡片生命周期。`streaming` 表示已经收到增量、但还没拿到最终文本。 */
export type BlockAiCardStatus = "idle" | "loading" | "streaming" | "done" | "error";

/** 追问轮次上限：设计文档 9.2.4 要求"保留最近 2 轮"。 */
export const MAX_FOLLOW_UP_ROUNDS = 2;

/** 折叠态摘要的最大字符数：超过就截断，保证永远是一行。 */
export const SUMMARY_MAX_CHARS = 60;

/** 建议命令单条最大字符数与最多条数：防止把一大段日志当命令列出来。 */
export const SUGGESTED_COMMAND_MAX_CHARS = 300;
export const SUGGESTED_COMMAND_MAX_ITEMS = 5;

export interface BlockAiFollowUp {
  question: string;
  answer: string;
}

export interface BlockAiCardState {
  blockId: string;
  mode: TerminalAiMode;
  status: BlockAiCardStatus;
  /** 已收到的全文（流式时是增量累积）。 */
  text: string;
  /** 折叠态；新结果默认展开，用户手动折叠后记住。 */
  collapsed: boolean;
  /** 失败信息；`status === "error"` 时必有。 */
  error?: { code: BlockAiErrorCode; message: string };
  /** 成功时的统计信息，展示在卡片footer。 */
  meta?: {
    modelId: string;
    requestBytes: number;
    durationMs: number;
    usedDeviceContext: boolean;
    tightened: boolean;
  };
  /** 追问历史，最新在后；最多 `MAX_FOLLOW_UP_ROUNDS` 条。 */
  followUps: BlockAiFollowUp[];
}

export type BlockAiAction =
  | { type: "start"; mode: TerminalAiMode; blockId: string }
  | { type: "delta"; text: string }
  | { type: "success"; outcome: Extract<BlockAiOutcome, { ok: true }> }
  | { type: "failure"; outcome: Extract<BlockAiOutcome, { ok: false }> }
  | { type: "toggleCollapsed" }
  | { type: "setCollapsed"; collapsed: boolean }
  /** 追问：把上一轮问答移进历史，正文清空重来。 */
  | { type: "followUp"; question: string; answer: string }
  | { type: "reset" };

/** 新卡片的初始状态。 */
export function initialBlockAiState(blockId: string, mode: TerminalAiMode = "explain"): BlockAiCardState {
  return {
    blockId,
    mode,
    status: "idle",
    text: "",
    collapsed: false,
    followUps: [],
  };
}

/** 卡片是否还能发起请求（避免重复点击打出两次请求）。 */
export function isBusy(state: BlockAiCardState): boolean {
  return state.status === "loading" || state.status === "streaming";
}

/** 状态机。纯函数，便于单测覆盖取消、重试、追问三条路径。 */
export function blockAiReducer(state: BlockAiCardState, action: BlockAiAction): BlockAiCardState {
  switch (action.type) {
    case "start":
      // 同一个块上重开一次：保留追问历史，正文与统计清空。
      return {
        ...state,
        blockId: action.blockId,
        mode: action.mode,
        status: "loading",
        text: "",
        collapsed: false,
        error: undefined,
        meta: undefined,
      };
    case "delta":
      if (!action.text) return state;
      // 后端可能只发增量、也可能把全文重复发一遍；统一按"增量拼接"处理，
      // 由 `subscribeDeltas` 保证同一 session 只订阅一次，不会重复计数。
      return {
        ...state,
        status: "streaming",
        text: state.text + action.text,
      };
    case "success":
      return {
        ...state,
        status: "done",
        // 流式已拼出全文时以流式结果为准（后端最终文本有时不含换行尾），
        // 但流式为空（例如没有 listen 能力）时回落到最终文本。
        text: state.text.trim() ? state.text : action.outcome.text,
        error: undefined,
        meta: {
          modelId: action.outcome.modelId,
          requestBytes: action.outcome.requestBytes,
          durationMs: action.outcome.durationMs,
          usedDeviceContext: action.outcome.usedDeviceContext,
          tightened: action.outcome.tightened,
        },
      };
    case "failure":
      return {
        ...state,
        status: "error",
        // 失败时把已经流出来的半截留下：用户能靠它判断是不是被截断的。
        error: { code: action.outcome.code, message: action.outcome.message },
        meta: undefined,
      };
    case "toggleCollapsed":
      return { ...state, collapsed: !state.collapsed };
    case "setCollapsed":
      return { ...state, collapsed: action.collapsed };
    case "followUp": {
      const next = [...state.followUps, { question: action.question, answer: action.answer }];
      return {
        ...state,
        followUps: next.slice(-MAX_FOLLOW_UP_ROUNDS),
        status: "done",
        text: "",
        error: undefined,
        meta: undefined,
      };
    }
    case "reset":
      return initialBlockAiState(state.blockId, state.mode);
    default:
      return state;
  }
}

/** 去掉 markdown 行内标记，只留可读文字（摘要与命令行共用）。 */
function stripInlineMarkdown(text: string): string {
  return text
    .replace(/`{1,3}([^`]*)`{1,3}/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/^#{1,6}\s*/, "")
    .replace(/^[-*+]\s+/, "")
    .trim();
}

/**
 * 折叠态一行摘要。
 *
 * 提示词要求模型按 `**失败原因**：xxx` / `**这条命令在做什么**：xxx` 的结构输出，
 * 所以优先取"第一个加粗标签后面那半句"——那通常就是结论本身。
 * 拿不到结构（模型没守格式、或用户自己在追问）就退回首行有效文字。
 */
export function summarizeBlockAi(text: string, maxChars = SUMMARY_MAX_CHARS): string {
  const source = (text ?? "").trim();
  if (!source) return "";

  const lines = source.split(/\r?\n/);
  let picked = "";
  // 围栏**内部**才是命令原文，比围栏标记更容易被误当成结论（模型经常先
  // 贴一遍命令再解释）。所以整段跳过，而不是只跳过 ``` 那一行。
  let inFence = false;

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("```")) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    if (!trimmed) continue;
    // 表格分隔线不是结论。
    if (/^[|\-\s:]+$/.test(trimmed)) continue;
    const boldLabel = /^\*\*([^*]{1,20})\*\*\s*[：:]?\s*(.*)$/.exec(trimmed);
    if (boldLabel && boldLabel[2].trim()) {
      picked = boldLabel[2];
      break;
    }
    const plain = stripInlineMarkdown(trimmed);
    if (plain) {
      picked = plain;
      break;
    }
  }

  if (!picked) return "";
  const condensed = picked.replace(/\s+/g, " ").trim();
  return condensed.length > maxChars ? `${condensed.slice(0, maxChars - 1)}…` : condensed;
}

/**
 * 从诊断结果里提取建议命令（F44）。
 *
 * 只认 **围栏代码块**里的内容，不从正文里猜——正文里的 `adb shell` 只是解释，
 * 拿它当按钮会误导用户执行。多行代码块取非注释的前几行逐条列出，但整块若是
 * 一段 shell 脚本（含 `if` / 变量赋值）就整体保留成一条，不拆开。
 */
export function extractSuggestedCommands(markdown: string): string[] {
  const source = markdown ?? "";
  if (!source) return [];

  const commands: string[] = [];
  const fence = /```[^\n]*\n([\s\S]*?)```/g;
  let match: RegExpExecArray | null;

  while ((match = fence.exec(source)) !== null) {
    const body = match[1].replace(/\r/g, "").trim();
    if (!body) continue;
    // 已经在输出里贴过的原始日志/报错不该变成"建议命令"，用启发式排除。
    if (!looksLikeCommandBlock(body)) continue;

    const lines = body
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line && !line.startsWith("#") && !line.startsWith("//"));

    const scriptLike = lines.some((line) => /\b(if|then|fi|for|do|done|while)\b/.test(line) || /^[A-Z_][A-Z0-9_]*=/.test(line));
    if (scriptLike) {
      const whole = lines.join("\n");
      if (whole.length <= SUGGESTED_COMMAND_MAX_CHARS) commands.push(whole);
      continue;
    }

    for (const line of lines) {
      // 去掉提示符前缀：`$ adb devices` → `adb devices`
      const command = line.replace(/^[$#>]\s+/, "").trim();
      if (!command || command.length > SUGGESTED_COMMAND_MAX_CHARS) continue;
      commands.push(command);
    }
  }

  const unique: string[] = [];
  for (const command of commands) {
    if (!unique.includes(command)) unique.push(command);
    if (unique.length >= SUGGESTED_COMMAND_MAX_ITEMS) break;
  }
  return unique;
}

/**
 * 判断一个代码块是不是"命令"而不是"贴回来的输出"。
 *
 * 输出块的典型特征：含大量 `|` 表格线、缩进堆栈、`at com.xxx()` 栈帧、
 * `E/AndroidRuntime` 这类 logcat 前缀。宁可漏掉一条建议，也不要让用户
 * 把一段报错点进输入面板。
 */
function looksLikeCommandBlock(body: string): boolean {
  const lines = body.split("\n").filter((line) => line.trim());
  if (!lines.length) return false;
  if (lines.length > 12) return false;
  if (lines.some((line) => /^\s+at\s+[\w.$]+\(/.test(line))) return false;
  if (lines.some((line) => /^[VDIWEF]\/[A-Za-z]/.test(line.trim()))) return false;
  const longLines = lines.filter((line) => line.length > 160).length;
  if (longLines > 0) return false;
  // 至少一行以常见命令开头，避免把纯文本说明当成命令。
  const COMMAND_HEADS = /^(adb|fastboot|shell|pm|am|cmd|dumpsys|logcat|ls|cat|mkdir|rm|cp|mv|chmod|chown|ps|kill|grep|find|df|du|top|getprop|setprop|settings|input|wm|svc|mount|remount|pm|gradle|\.\/gradlew|npm|pnpm|node|git|curl|wget|echo|export|source|cd|pwd)\b/;
  return lines.some((line) => COMMAND_HEADS.test(line.replace(/^[$#>]\s+/, "").trim()));
}

/** footer 上的一行统计。`requestBytes` 用 KB 表示，不足 1 KB 就显示字节。 */
export function formatBlockAiMeta(meta: BlockAiCardState["meta"]): string {
  if (!meta) return "";
  const size = meta.requestBytes >= 1024
    ? `${(meta.requestBytes / 1024).toFixed(1)} KB`
    : `${meta.requestBytes} B`;
  const seconds = Math.max(0.1, meta.durationMs / 1000);
  const parts = [size, `${seconds.toFixed(1)} s`, meta.modelId];
  if (meta.usedDeviceContext) parts.push("含设备上下文");
  if (meta.tightened) parts.push("输出已压缩");
  return parts.join(" · ");
}
