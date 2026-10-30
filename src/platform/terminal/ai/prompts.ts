/**
 * 命令块 AI 提示词（开发方案 9.2 / 任务 T3-1）。
 *
 * 这个文件是**纯字符串组装**，不 import 任何平台 API，原因有两个：
 * 1. 单测用 `node --experimental-strip-types` 直接跑 `.mts`，任何 `@tauri-apps/*`
 *    的静态 import 都会在加载期失败；
 * 2. 提示词是产品的"输出契约"（F41–F46 的验收口径全靠它），把它与网络/IO
 *    分离，改措辞时不用碰调用链。
 *
 * 两种模式：
 * - `explain`：用户主动点"解释这条命令"，甚至命令成功了也会问；
 * - `diagnose`：命令失败后的"为什么失败 / 怎么修"，必须给出可执行下一步。
 */
import type { CommandBlock } from "../blocks/types.ts";
import { blockOutputText, truncateText, utf8ByteLength, type RedactOptions } from "../blocks/ansiText.ts";

export type TerminalAiMode = "explain" | "diagnose";

/**
 * 进 prompt 的输出预算（9.2）。
 * 失败信息几乎总在尾部（最后一段报错），所以裁剪默认 `keepTail`，
 * 并额外保留开头若干行——很多工具（`gradle`、`make`）的失败原因在开头就写了。
 */
export interface TerminalAiLimits {
  /** 尾部保留行数。 */
  maxLines: number;
  /** 绝对字节上限；配合 F43「请求体 P95 ≤ 16 KB」。 */
  maxBytes: number;
  /** 头部额外保留行数（用于 `FAILURE: Build failed` 这类前置结论）。 */
  headLines: number;
}

export const TERMINAL_AI_LIMITS: TerminalAiLimits = {
  maxLines: 160,
  maxBytes: 8 * 1024,
  headLines: 20,
};

/** 整个请求体（message 全文）的硬上限，超过就要继续砍输出（F43）。 */
export const MAX_REQUEST_BYTES = 16 * 1024;

export interface BlockOutputSnippet {
  /** 已去 ANSI、已裁剪、已按需脱敏的文本。 */
  text: string;
  /** 是否发生过裁剪（行数或字节）。 */
  truncated: boolean;
  /** 原始字节量，用于在 prompt 里说明"我看到的不是全部"。 */
  originalBytes: number;
}

export interface BuildPromptInput {
  block: CommandBlock;
  mode: TerminalAiMode;
  /** 脱敏开关；默认全开（S4）。 */
  redact?: RedactOptions;
  /** 用户额外补充的问题（可选）。 */
  question?: string;
  /** 覆盖默认预算，单测用。 */
  limits?: Partial<typeof TERMINAL_AI_LIMITS>;
}

/**
 * 从块里抽输出片段。
 *
 * 与 `blockOutputText` 的区别：这里额外做"头 + 尾"两段拼接。纯尾部裁剪会
 * 丢掉 `make`/`gradle` 在开头打印的失败摘要，而那段往往比栈更可读。
 */
export function snippetFromBlock(
  block: Pick<CommandBlock, "outputText" | "outputTruncated">,
  options: { redact?: RedactOptions; limits?: Partial<typeof TERMINAL_AI_LIMITS> } = {},
): BlockOutputSnippet {
  const limits = { ...TERMINAL_AI_LIMITS, ...options.limits };
  const raw = block.outputText ?? "";
  const originalBytes = utf8ByteLength(raw);

  // 整块都放得下就不做头尾拼接，避免平白多一次切分。
  const whole = blockOutputText(raw, { maxBytes: limits.maxBytes, maxLines: limits.maxLines, ...options.redact });
  if (!whole.truncated) {
    return { text: whole.text, truncated: Boolean(block.outputTruncated), originalBytes };
  }

  const plain = blockOutputText(raw, { ...options.redact }).text;
  const lines = plain.split("\n");
  // 头部同样要受字节预算约束：单行超长输出（例如一行 base64 或压缩过的
  // gradle 日志）只按行数切会原样把整行塞进来，头尾拼完反而比原文件还大。
  const headBudget = Math.max(512, Math.floor(limits.maxBytes * 0.25));
  const headText = truncateText(lines.slice(0, limits.headLines).join("\n"), {
    maxBytes: headBudget,
    keepTail: false,
  }).text;
  const tail = blockOutputText(raw, {
    maxLines: limits.maxLines,
    maxBytes: Math.max(1024, Math.floor(limits.maxBytes * 0.75)),
    keepTail: true,
    ...options.redact,
  }).text;

  const omitted = Math.max(0, lines.length - limits.headLines - limits.maxLines);
  const joined = `${headText}\n…（中间省略约 ${omitted} 行）…\n${tail}`;
  return { text: joined, truncated: true, originalBytes };
}

/** 出口码的可读化：`undefined` 表示 shell 没上报，不代表成功。 */
function describeExitCode(block: CommandBlock): string {
  if (block.status === "running") return "仍在运行";
  if (block.status === "interrupted") return "被用户中断（Ctrl+C）";
  if (block.status === "partial") return "输出被 scrollback 裁剪，不完整";
  if (block.exitCode === undefined) return "未知（shell 未上报 OSC 133 command-end）";
  return block.exitCode === 0 ? "0（成功）" : `${block.exitCode}（失败）`;
}

const TARGET_LABEL: Record<CommandBlock["targetKind"], string> = {
  local: "本机（Windows）",
  wsl: "WSL 发行版",
  ssh: "远程 SSH 主机",
};

/** 两种模式共享的系统提示词骨架：强调"只读、不调工具、不给编造的命令"。 */
function systemPrompt(mode: TerminalAiMode): string {
  const common = [
    "你是一名资深终端排障助手，内嵌在一个终端面板里。",
    "给你的是用户刚刚在终端里执行过的一条命令、它的退出状态和输出片段。",
    "",
    "硬性约束：",
    "1. 你没有工具、不能执行命令、不能读取文件。只能基于下面给你的内容作答。",
    "2. 你看不到完整输出时不代表没有输出；不确定就说不确定，不要编造报错文本。",
    "3. 涉及破坏性操作（`rm -rf`、`mkfs`、`format`）时必须提醒风险。",
    "4. 用中文回答，命令、路径、参数、报错原文保持原样不翻译。",
    "5. 不要输出思考过程，直接给结论。",
  ];
  const byMode =
    mode === "explain"
      ? [
          "",
          "输出结构（标题用 **加粗**，不要用 Markdown 一级标题）：",
          "**这条命令在做什么**：一到两句，说明命令与关键参数的作用。",
          "**输出如何解读**：只看这段输出里真实出现的信息；成功就说明成功在哪里，有警告就指出警告的意思。",
          "**需要注意**：仅在有风险或异常时给出，没有就省略这一节。",
        ]
      : [
          "",
          "输出结构（标题用 **加粗**，不要用 Markdown 一级标题）：",
          "**失败原因**：一句话给出最可能的原因，不要罗列所有可能。",
          "**依据**：引用输出里能支撑这个判断的具体几行（原文照抄，用代码格式）。",
          "**怎么修**：给出 1 到 3 条可直接执行的命令或具体操作，按推荐顺序排列；每条后面一句话说明它在做什么。",
          "**如果还不行**：给出一条用于进一步定位的诊断命令。",
          "",
          "如果输出看起来其实是成功的，或者失败与这条命令无关，直接说出来，不要硬凑原因。",
        ];
  return [...common, ...byMode].join("\n");
}

/** 组装用户消息体（不含 system）。 */
export function buildBlockUserMessage(input: BuildPromptInput): string {
  const { block, mode } = input;
  const snippet = snippetFromBlock(block, { redact: input.redact, limits: input.limits });
  const parts: string[] = [];

  parts.push("## 命令信息");
  parts.push(`- 目标：${TARGET_LABEL[block.targetKind]}`);
  if (block.cwd) parts.push(`- 工作目录：${block.cwd}`);
  parts.push(`- 退出状态：${describeExitCode(block)}`);
  if (block.endedAt && block.startedAt) {
    const seconds = Math.max(0, (block.endedAt - block.startedAt) / 1000);
    parts.push(`- 耗时：${seconds.toFixed(seconds < 10 ? 1 : 0)} 秒`);
  }
  parts.push("");
  parts.push("## 命令原文");
  parts.push("```");
  parts.push(block.command.trim());
  parts.push("```");

  parts.push("");
  parts.push("## 输出");
  if (!snippet.text.trim()) {
    parts.push("（没有捕获到任何输出）");
  } else {
    if (snippet.truncated) {
      parts.push(`（输出共约 ${snippet.originalBytes} 字节，以下为头部与尾部的片段，中间已省略）`);
    }
    parts.push("```");
    parts.push(snippet.text);
    parts.push("```");
  }

  if (input.question?.trim()) {
    parts.push("");
    parts.push("## 用户补充问题");
    parts.push(input.question.trim());
  }

  parts.push("");
  parts.push(mode === "explain" ? "请解释这条命令与这段输出。" : "请判断失败原因并给出修复方案。");
  return parts.join("\n");
}

export interface BuiltPrompt {
  system: string;
  user: string;
  /** 两者 UTF-8 字节数之和，用于 F43 的 P95 ≤ 16 KB 校验。 */
  bytes: number;
  /** 为压缩体积是否又砍过一轮输出。 */
  tightened: boolean;
}

/**
 * 组装完整提示词。
 *
 * 若一次组装就超过 `MAX_REQUEST_BYTES`（比如命令本身是超长的一行 base64），
 * 会再压一轮输出预算；仍然超限就保留现状——此时宁可让模型看到被截断的现场，
 * 也不静默丢弃命令原文，否则模型会基于错误前提作答。
 */
export function buildBlockPrompt(input: BuildPromptInput): BuiltPrompt {
  const system = systemPrompt(input.mode);
  let user = buildBlockUserMessage(input);
  let bytes = utf8ByteLength(system) + utf8ByteLength(user);
  let tightened = false;

  if (bytes > MAX_REQUEST_BYTES) {
    tightened = true;
    const half = {
      maxLines: Math.max(20, Math.floor(TERMINAL_AI_LIMITS.maxLines / 3)),
      maxBytes: Math.max(1024, Math.floor(TERMINAL_AI_LIMITS.maxBytes / 3)),
      headLines: 8,
    };
    user = buildBlockUserMessage({ ...input, limits: { ...input.limits, ...half } });
    bytes = utf8ByteLength(system) + utf8ByteLength(user);
  }

  return { system, user, bytes, tightened };
}
