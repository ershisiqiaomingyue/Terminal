/**
 * 输入侧 AI（阶段 4 / T4-1 服务层）：自然语言 → 命令草稿。
 *
 * 与 `terminalAiService.ts`（输出侧解释/诊断）的分工：
 * - 那边是「读」：把命令块输出喂给模型，回答是给人看的文本；
 * - 这边是「写」：把自然语言喂给模型，回答是**要执行的命令**，所以多一层
 *   解析 + 风险分级 + 本地兜底（模型说 read-only 的命令，本地仍会复核）。
 *
 * 本文件不静态 import `@/platform/*`：`runtime.ts` 顶层就
 * `import { invoke } from "@tauri-apps/api/core"`，在 `node
 * --experimental-strip-types` 下单测加载即失败。真实依赖由
 * `terminalAiDeps.ts` 注入，两边复用同一份。
 */
import { recordAiCall } from "../metrics.ts";
import {
  buildComposerPrompt,
  parseCommandDraft,
  MAX_COMPOSER_REQUEST_BYTES,
  type CommandDraft,
  type ComposeInput,
} from "./commandComposer.ts";
import {
  buildBlockAiRequest,
  resolveModelChoice,
  type BlockAiDeps,
  type TerminalModelSelection,
  type TerminalProviderLike,
} from "./terminalAiService.ts";

export type ComposeErrorCode =
  | "empty_input"
  | "no_model"
  | "request_too_large"
  | "aborted"
  | "parse_failed"
  | "invoke_failed";

export interface ComposeOptions {
  selection?: TerminalModelSelection;
  signal?: AbortSignal;
}

export interface ComposeResult {
  ok: true;
  draft: CommandDraft;
  /** 模型原始返回，便于排查解析失败前的原文。 */
  raw: string;
  modelId: string;
  requestBytes: number;
  durationMs: number;
}

export interface ComposeFailure {
  ok: false;
  code: ComposeErrorCode;
  /** 可直接展示给用户的中文说明。 */
  message: string;
}

export type ComposeOutcome = ComposeResult | ComposeFailure;

/** 自然语言里允许的最大字符数；超了直接拒，不浪费一次模型调用。 */
export const MAX_NL_CHARS = 2000;

/** 这些失败代表「请求没发出去」，不计入 AI 错误率。 */
const PREFLIGHT_CODES = new Set<ComposeErrorCode>(["empty_input", "no_model", "request_too_large", "aborted"]);

/** 从 `#` 开头的输入里取出自然语言；不是 `#` 开头返回 null。 */
export function extractNaturalLanguage(input: string): string | null {
  const trimmed = input.trimStart();
  if (!trimmed.startsWith("#")) return null;
  return trimmed.slice(1).trim();
}

/**
 * 对外入口：`draftCommands` 包一层埋点，与 `runBlockAi` 同一口径。
 * 埋点异常一律吞掉，不能因为统计坏掉而让输入侧 AI 不可用。
 */
export async function draftCommands(
  input: ComposeInput,
  deps: BlockAiDeps,
  options: ComposeOptions = {},
): Promise<ComposeOutcome> {
  const now = deps.now ?? (() => Date.now());
  const startedAt = now();
  const outcome = await draftCommandsAttempt(input, deps, options);
  if (!(outcome.ok === false && PREFLIGHT_CODES.has(outcome.code))) {
    try {
      recordAiCall({
        // 输入侧与输出侧共用同一个埋点管道，用 explain 槽位区分不了，
        // 因此沿用 mode 字段但由调用方从 metrics 的草稿计数里区分（见 metrics.ts）。
        mode: "explain",
        ok: outcome.ok,
        durationMs: Math.max(0, now() - startedAt),
        requestBytes: outcome.ok ? outcome.requestBytes : undefined,
      });
    } catch {
      // 静默丢弃，见上。
    }
  }
  return outcome;
}

async function draftCommandsAttempt(
  input: ComposeInput,
  deps: BlockAiDeps,
  options: ComposeOptions = {},
): Promise<ComposeOutcome> {
  const nl = input.nl?.trim() ?? "";
  if (!nl) {
    return { ok: false, code: "empty_input", message: "先用 `#` 写一句想做什么，例如 `# 看一下电池温度`。" };
  }
  if (nl.length > MAX_NL_CHARS) {
    return {
      ok: false,
      code: "request_too_large",
      message: `需求太长（${nl.length} 字），请压缩到 ${MAX_NL_CHARS} 字以内。`,
    };
  }
  if (options.signal?.aborted) {
    return { ok: false, code: "aborted", message: "已取消。" };
  }

  const now = deps.now ?? (() => Date.now());
  const newId = deps.newId ?? ((prefix: string) => `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`);
  const startedAt = now();

  const listProviders = deps.listProviders;
  if (!listProviders) {
    return { ok: false, code: "no_model", message: "未接入模型供应商列表，无法生成命令草稿。" };
  }
  let providers: TerminalProviderLike[] = [];
  try {
    providers = await listProviders();
  } catch {
    providers = [];
  }
  const choice = resolveModelChoice(providers, options.selection);
  if (!choice) {
    return { ok: false, code: "no_model", message: "请先在设置中配置模型供应商，再让 AI 生成命令。" };
  }

  const prompt = buildComposerPrompt({ ...input, nl });
  if (prompt.bytes > MAX_COMPOSER_REQUEST_BYTES * 2) {
    return {
      ok: false,
      code: "request_too_large",
      message: `请求内容过长（约 ${Math.round(prompt.bytes / 1024)} KB），已停止发送。请精简需求或历史命令后重试。`,
    };
  }

  // 草稿生成是**独立会话**：不写进用户的主对话线程，也不带工具策略（只产文本）。
  const sessionId = `terminal-compose-${newId("composer")}`;
  const turnId = newId("terminal-compose-turn");
  const request = buildBlockAiRequest({
    modelId: choice.modelId,
    supportsReasoning: choice.supportsReasoning,
    prompt,
    sessionId,
    turnId,
  });

  try {
    const response = await deps.invoke<{ text?: string }>("agent_one_shot", { request });
    const raw = (response?.text ?? "").trim();
    if (!raw) {
      return { ok: false, code: "invoke_failed", message: "模型没有返回内容，请重试。" };
    }
    const parsed = parseCommandDraft(raw);
    if (parsed.ok === false) {
      return { ok: false, code: "parse_failed", message: `${parsed.error}（模型原文：${shorten(raw)}）` };
    }
    return {
      ok: true,
      draft: parsed.draft,
      raw,
      modelId: choice.modelId,
      requestBytes: prompt.bytes,
      durationMs: Math.max(0, now() - startedAt),
    };
  } catch (error) {
    return { ok: false, code: "invoke_failed", message: describeInvokeError(error) };
  }
}

function shorten(text: string, max = 160): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine;
}

/** 与输出侧同一套错误措辞，避免两处提示风格不一致。 */
function describeInvokeError(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const detail = raw.trim();
  if (!detail) return "AI 请求失败，请稍后重试。";
  if (/timeout|timed out/i.test(detail)) return `AI 请求超时，请重试。（${detail}）`;
  if (/401|403|unauthor|forbidden|api key/i.test(detail)) return `模型鉴权失败，请检查供应商配置。（${detail}）`;
  if (/429|rate limit|quota/i.test(detail)) return `模型限流或额度不足，请稍后重试。（${detail}）`;
  return `AI 请求失败：${detail}`;
}
