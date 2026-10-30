/**
 * 输入侧 AI：自然语言 → 命令草稿（开发方案 9.3 / 任务 T4-2）。
 *
 * 与 `prompts.ts` 一样是**纯字符串 / 纯逻辑**，不 import 任何平台 API：
 * 单测用 `node --experimental-strip-types` 直接跑，`@tauri-apps/*` 的静态
 * import 会在加载期就失败。
 *
 * 这个文件只负责三件事，UI 与网络调用都不在这里：
 * 1. 组装"要求模型返回严格 JSON"的提示词；
 * 2. 把模型返回的文本解析成结构化草稿（容忍围栏、前后废话）；
 * 3. **本地兜底风险分级** —— 这是 S1/S2 的关键，见 `classifyCommandsRisk`。
 */
import { utf8ByteLength } from "../blocks/ansiText.ts";

/** 命令目标。故意用字面量而不是 import 视图里的类型，避免逻辑层依赖 UI 层。 */
export type ComposerTarget = "local" | "wsl" | "ssh";

/** 风险等级（9.3.2 的输出协议，顺序即严重度）。 */
export type CommandRisk = "read-only" | "mutating" | "destructive";

const RISK_ORDER: Record<CommandRisk, number> = { "read-only": 0, mutating: 1, destructive: 2 };

export const COMMAND_RISKS: readonly CommandRisk[] = ["read-only", "mutating", "destructive"] as const;

/** 取两个风险里更严重的那个。 */
export function maxRisk(left: CommandRisk, right: CommandRisk): CommandRisk {
  return RISK_ORDER[right] > RISK_ORDER[left] ? right : left;
}

export function isCommandRisk(value: unknown): value is CommandRisk {
  return typeof value === "string" && (COMMAND_RISKS as readonly string[]).includes(value);
}

export interface CommandDraft {
  /** 至少一条；多命令按 9.3.3 逐条确认。 */
  commands: string[];
  explanation: string;
  /** **最终**风险：`max(模型判定, 本地分级)`，UI 只认这个。 */
  risk: CommandRisk;
  /** 模型原始判定，仅用于埋点与排查（模型可能低估，见文件头说明）。 */
  modelRisk?: CommandRisk;
  needsRoot: boolean;
  /** 模型输出是否被本地分级上调过，命中时应提示用户。 */
  riskEscalated: boolean;
}

/* ------------------------------------------------------------------ *
 * 提示词组装
 * ------------------------------------------------------------------ */

export interface ComposeInput {
  /** 用户输入的自然语言（`#` 之后的部分）。 */
  nl: string;
  target: ComposerTarget;
  /** 当前工作目录，本地/WSL/SSH 才有意义。 */
  cwd?: string;
  /** 最近执行过的命令，最新的在前。 */
  history?: string[];
  /** 目标 shell，用于提示语法。 */
  shell?: string;
  /** 单测用。 */
  limits?: { maxHistory?: number; maxNlChars?: number };
}

const DEFAULT_COMPOSE_LIMITS = { maxHistory: 10, maxNlChars: 2000 };

/** 整个请求体的硬上限，与输出侧保持一致的 16 KB（F43 同一口径）。 */
export const MAX_COMPOSER_REQUEST_BYTES = 16 * 1024;

const TARGET_LABEL: Record<ComposerTarget, string> = {
  local: "本机（Windows）",
  wsl: "WSL",
  ssh: "远程 SSH 主机",
};

function composerSystemPrompt(input: ComposeInput): string {
  const lines: string[] = [
    "你在把一个自然语言需求翻译成一条可执行的终端命令。",
    "只输出一个 JSON 对象，不要任何解释文字、不要 Markdown 围栏。",
    "",
    "JSON 结构：",
    '{"commands":["命令1"],"explanation":"一句话说明这条命令做了什么","risk":"read-only","needsRoot":false}',
    "",
    "规则：",
    "1. `risk` 只能是 `read-only` / `mutating` / `destructive` 三者之一。",
    "2. 默认只给**一条**命令；只有确实需要多步才能完成时才给多条，且越少越好。",
    "3. 不要生成 `sudo`、`su` 之外的提权前缀；需要 root 时把 `needsRoot` 设为 true 并在说明里写明。",
    "4. 不要在执行环境不存在的路径上操作；跨系统路径（Windows 路径 ↔ 设备路径）要各自转换。",
    "5. 无法完成时返回 `{\"commands\":[],\"explanation\":\"原因\",\"risk\":\"read-only\",\"needsRoot\":false}`。",
    "",
    `执行环境：${TARGET_LABEL[input.target]}`,
  ];
  if (input.shell) lines.push(`Shell：${input.shell}`);
  return lines.join("\n");
}

function composerUserMessage(input: ComposeInput): string {
  const limits = { ...DEFAULT_COMPOSE_LIMITS, ...input.limits };
  const parts: string[] = [];
  if (input.cwd) parts.push(`当前目录：${input.cwd}`);
  const history = (input.history ?? []).filter((line) => line.trim()).slice(0, limits.maxHistory);
  if (history.length) {
    parts.push("最近执行过的命令（最新在前）：");
    for (const line of history) parts.push(`- ${line}`);
  }
  const nl = input.nl.length > limits.maxNlChars ? input.nl.slice(0, limits.maxNlChars) : input.nl;
  parts.push("", `需求：${nl}`);
  return parts.join("\n");
}

export interface ComposedPrompt {
  system: string;
  user: string;
  bytes: number;
}

/**
 * 组装提示词。
 *
 * 超限时优先砍历史命令（它只是辅助模型模仿用户习惯），不砍需求本身——
 * 需求被截断会让模型猜错意图，比缺少上下文更糟。
 */
export function buildComposerPrompt(input: ComposeInput): ComposedPrompt {
  const system = composerSystemPrompt(input);
  let user = composerUserMessage(input);
  let bytes = utf8ByteLength(system) + utf8ByteLength(user);

  for (const maxHistory of [3, 0]) {
    if (bytes <= MAX_COMPOSER_REQUEST_BYTES) break;
    user = composerUserMessage({ ...input, limits: { ...input.limits, maxHistory } });
    bytes = utf8ByteLength(system) + utf8ByteLength(user);
  }

  return { system, user, bytes };
}

/* ------------------------------------------------------------------ *
 * 风险分级（本地兜底）
 * ------------------------------------------------------------------ */

/** 命令分隔符：管道、串行、且/或。逐段判风险再取最大值。 */
const SEGMENT_SPLIT = /\|\||&&|[|;\n]/;

/** 剥掉这些前缀后再判断真正干活的那个程序。 */
const PREFIX_PATTERNS: RegExp[] = [
  /^sudo\s+/,
  /^su\s+-c\s+/,
  // `bash -c "…"` / `sh -c '…'`：等同直接执行引号里的命令。
  /^(?:bash|sh|zsh|dash)\s+-c\s+/,
  /^env\s+/,
  /^(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)+/, // FOO=1 cmd
];

/**
 * 剥掉包裹整个命令的引号。
 *
 * `su -c 'rm -rf /data'` 剥掉前缀后剩 `'rm -rf /data'`，程序名会变成 `'rm`，
 * 于是 `rm` 分支永远走不到——红队测试抓到的真实漏判。
 */
function unwrapQuotes(text: string): string {
  const trimmed = text.trim();
  const match = /^(['"])([\s\S]*)\1$/.exec(trimmed);
  return match ? match[2].trim() : trimmed;
}

/** 破坏性命令的"程序名 → 参数条件"。不满足参数条件时降为 mutating。 */
const DESTRUCTIVE_PROGRAMS = new Set([
  "mkfs",
  "wipefs",
  "shred",
  "fastboot-flash",
  "format",
]);

/** 这些程序一旦出现就至少是 mutating（改状态，需要确认）。 */
const MUTATING_PROGRAMS = new Set([
  "rm", "rmdir", "mv", "cp", "chmod", "chown", "chgrp", "ln", "touch", "mkdir",
  "dd", "truncate", "tee", "sed", "awk",
  "pm", "am", "settings", "setprop", "svc", "wm", "input",
  "kill", "killall", "pkill", "reboot", "shutdown", "poweroff",
  "install", "uninstall", "push", "pull", "sync", "mount", "umount", "remount",
  "git", "npm", "pnpm", "yarn", "pip", "cargo", "make", "gradle",
  "docker", "kubectl", "systemctl", "service", "tar", "unzip",
  // `curl` / `wget` 刻意**不**放进来：单纯下载/请求是只读的，只有带 `-o`/`-O`
  // 落盘才算改状态，交给下面那条专门的判断（放这里会把所有 curl 都判成 mutating）。
]);

/** `fastboot` / `dd` 这类要看子命令或参数才知道有多危险。 */
function fastbootRisk(tokens: string[]): CommandRisk {
  const sub = tokens[1] ?? "";
  // 查询类子命令不改设备状态。
  if (["devices", "getvar", "version", "help", "oem-help", ""].includes(sub)) return "read-only";
  if (["flash", "erase", "format", "flashall", "update", "oem", "set_active"].includes(sub)) return "destructive";
  return "mutating";
}

function rmRisk(tokens: string[]): CommandRisk {
  const flags = tokens.slice(1).filter((token) => token.startsWith("-")).join("");
  const recursive = flags.includes("r") || flags.includes("R");
  // 递归删除一律按破坏性处理：`rm -r` 删的是整棵目录树，和 `rm -rf` 一样不可恢复，
  // 少一个 `f` 只是"不忽略错误"，不改变后果的严重程度。
  if (recursive) return "destructive";
  // 非递归的 `rm file` / `rm -f file` 只是普通改状态。
  return "mutating";
}

function ddRisk(tokens: string[]): CommandRisk {
  const hasDeviceOutput = tokens.some((token) => /^of=\/dev\//.test(token));
  return hasDeviceOutput ? "destructive" : "mutating";
}

/**
 * 单条（可能含管道/串联的）命令的风险。
 *
 * 逐段判断取最大值：`rm -rf /sdcard/x | cat` 的第一段已经是破坏性的，
 * 不能因为整行"看起来是管道"就放过。
 */
export function classifyCommandRisk(command: string): CommandRisk {
  let risk: CommandRisk = "read-only";
  for (const rawSegment of command.split(SEGMENT_SPLIT)) {
    const segment = rawSegment.trim();
    if (!segment) continue;
    risk = maxRisk(risk, classifySegment(segment));
    if (risk === "destructive") return risk;
  }
  return risk;
}

function classifySegment(segment: string): CommandRisk {
  let text = segment;
  for (const pattern of PREFIX_PATTERNS) text = text.replace(pattern, "");
  text = unwrapQuotes(text);
  // 写入设备节点 / 块设备：`echo x > /dev/block/...`、`cat x > /dev/sda`。
  if (/>+\s*\/dev\/(?:block|sd|null|zero)/.test(text)) return "destructive";

  const tokens = text.split(/\s+/).filter(Boolean);
  if (!tokens.length) return "read-only";
  const program = tokens[0].replace(/^.*\//, "").toLowerCase();

  if (program === "rm") return rmRisk(tokens);
  if (program === "dd") return ddRisk(tokens);
  if (program === "fastboot") return fastbootRisk(tokens);
  if (program.startsWith("mkfs")) return "destructive";
  if (DESTRUCTIVE_PROGRAMS.has(program)) return "destructive";
  // `pm clear` 抹掉应用全部数据且不可恢复——按 10.1 的口径（不可恢复即破坏性）处理。
  if (program === "pm" && tokens[1] === "clear") return "destructive";
  if (MUTATING_PROGRAMS.has(program)) {
    // `sed` / `awk` 只有带 `-i` 才真的改文件。
    if (program === "sed" && !tokens.some((token) => token === "-i" || /^-[a-zA-Z]*i/.test(token))) return "read-only";
    if (program === "awk" && !tokens.includes("-i")) return "read-only";
    return "mutating";
  }
  // `curl -o file` / `wget -O file` 会落盘。
  if ((program === "curl" || program === "wget") && tokens.some((token) => /^-[a-zA-Z]*[oO]/.test(token))) {
    return "mutating";
  }
  return "read-only";
}

/** 多条命令的整体风险 = 各条最大值。 */
export function classifyCommandsRisk(commands: string[]): CommandRisk {
  return commands.reduce<CommandRisk>((acc, command) => maxRisk(acc, classifyCommandRisk(command)), "read-only");
}

/** 是否需要提权（本地兜底，模型给的 `needsRoot` 只作补充）。 */
export function detectNeedsRoot(commands: string[]): boolean {
  return commands.some((command) => {
    const text = command.toLowerCase();
    return (
      /\bsu\s+-c\b/.test(text) ||
      /^\s*su\b/.test(text) ||
      /\bmount\s+-o\s+rw\b/.test(text)
    );
  });
}

/* ------------------------------------------------------------------ *
 * 二次确认（S2 / 10.1 第二层）
 * ------------------------------------------------------------------ */

/** 只有只读命令可以一键执行；其余都要一次显式确认。 */
export function needsTypedConfirmation(risk: CommandRisk): boolean {
  return risk === "destructive";
}

export function needsConfirmClick(risk: CommandRisk): boolean {
  return risk !== "read-only";
}

function riskRank(risk: CommandRisk): number {
  if (risk === "destructive") return 2;
  if (risk === "mutating") return 1;
  return 0;
}

/**
 * destructive 命令要求用户**手打命令首词**才放行（10.1 第二层）。
 * 例如 `rm -rf /sdcard/x` 要打 `rm`。
 *
 * 复合命令取**风险最高那一段**的程序名：`ls || rm -rf /sdcard` 的首词是良性
 * 的 `ls`，让用户打 `ls` 等于用一个无害的词去确认一条危险命令（红队用例抓到的
 * 真实问题）。同风险时保持原顺序，取第一段。
 */
export function confirmationToken(command: string): string {
  const segments = command
    .split(SEGMENT_SPLIT)
    .map((segment) => segment.trim())
    .filter(Boolean);
  const ranked = segments
    .map((segment) => ({ segment, rank: riskRank(classifySegment(segment)) }))
    .sort((a, b) => b.rank - a.rank);
  const target = ranked[0]?.segment ?? command.trim();
  let text = target;
  for (const pattern of PREFIX_PATTERNS) text = text.replace(pattern, "");
  text = unwrapQuotes(text);
  const token = text.split(/\s+/).filter(Boolean)[0] ?? "";
  return token.replace(/^.*\//, "").replace(/^['"]+|['"]+$/g, "").toLowerCase();
}

export function isConfirmationSatisfied(command: string, typed: string): boolean {
  const expected = confirmationToken(command);
  return expected.length > 0 && typed.trim().toLowerCase() === expected;
}

/* ------------------------------------------------------------------ *
 * 解析模型输出
 * ------------------------------------------------------------------ */

/**
 * 从模型返回文本里抠出 JSON 对象。
 *
 * 三种常见污染都要容忍：Markdown 围栏、JSON 前后的解释文字、尾随逗号。
 * 只做"第一个 `{` 到最后一个 `}`"的截取，不写完整 JSON 修复器——解析失败
 * 时让用户重试比猜错一条命令安全得多。
 */
function extractJsonObject(raw: string): { ok: true; value: unknown } | { ok: false; error: string } {
  const text = raw.replace(/```(?:json)?/gi, "").trim();
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start < 0 || end <= start) return { ok: false, error: "模型没有返回 JSON 对象" };
  const candidate = text.slice(start, end + 1);
  for (const attempt of [candidate, candidate.replace(/,\s*([}\]])/g, "$1")]) {
    try {
      return { ok: true, value: JSON.parse(attempt) };
    } catch {
      // 继续试下一种修法
    }
  }
  return { ok: false, error: "模型返回的 JSON 无法解析" };
}

export type ParseDraftOutcome =
  | { ok: true; draft: CommandDraft }
  | { ok: false; error: string; raw: string };

/**
 * 解析并**加固**模型输出。
 *
 * 要点：`risk` 取 `max(模型, 本地)`。模型把 `rm -rf /sdcard` 说成 `read-only`
 * 是真实会发生的（提示词再多也压不住），只信模型就等于把 S1/S2 交给对手。
 */
export function parseCommandDraft(raw: string): ParseDraftOutcome {
  const extracted = extractJsonObject(raw);
  if (!extracted.ok) return { ok: false, error: extracted.error, raw };
  const value = extracted.value;
  if (typeof value !== "object" || value === null) return { ok: false, error: "模型返回的不是对象", raw };

  const record = value as Record<string, unknown>;
  const rawCommands = Array.isArray(record.commands) ? record.commands : [];
  const commands = rawCommands
    .filter((item): item is string => typeof item === "string")
    .map((item) => item.trim().replace(/\r/g, ""))
    .filter((item) => item.length > 0);
  const explanation = typeof record.explanation === "string" ? record.explanation.trim() : "";
  if (!commands.length) {
    return { ok: false, error: explanation || "模型没有给出可用命令", raw };
  }

  const modelRisk = isCommandRisk(record.risk) ? record.risk : undefined;
  const localRisk = classifyCommandsRisk(commands);
  const risk = modelRisk ? maxRisk(modelRisk, localRisk) : localRisk;
  const modelNeedsRoot = record.needsRoot === true;
  const needsRoot = modelNeedsRoot || detectNeedsRoot(commands);

  return {
    ok: true,
    draft: {
      commands,
      explanation,
      risk,
      modelRisk,
      needsRoot,
      riskEscalated: modelRisk !== undefined && risk !== modelRisk,
    },
  };
}

/* ------------------------------------------------------------------ *
 * 草稿操作（9.3.3 / 9.3.4）
 * ------------------------------------------------------------------ */

/** 默认只执行第一条，其余要用户逐条确认。 */
export function primaryCommand(draft: CommandDraft): string | undefined {
  return draft.commands[0];
}

/** 把"全部插入"的内容拼成面板里的多行草稿（不执行）。 */
export function draftAsMultiline(draft: CommandDraft, separator: string): string {
  return draft.commands.join(separator);
}

/**
 * 直通模式下输入面板必须禁用（9.3.4）。
 * 返回给 UI 的提示文案，`null` 表示可以正常用面板。
 */
export function composerBlockedReason(passthrough: boolean, passthroughHint?: string): string | null {
  if (!passthrough) return null;
  return passthroughHint
    ? `当前程序需要直接输入（${passthroughHint}），输入面板已暂停`
    : "当前程序需要直接输入，输入面板已暂停";
}
