/**
 * 命令块 AI 服务层（开发方案 9.2 / 任务 T3-1）。
 *
 * 职责：把一条 `CommandBlock` 变成一次只读的模型问答——组装提示词、
 * 调用一次性模型通道 `agent_one_shot`（原 send_agent_message 更名，语义为同步 RPC 而非对话消息）、把结果交回 UI。
 *
 * 三条设计约束：
 * 1. **不新增后端命令**。复用 `/chat` 那条 `agent_one_shot`（`toolPolicy:
 *    "disabled"`），所以不用改 Rust 侧，也不会引入第二套模型配置。
 * 2. **可单测**。本文件不静态 import `@/platform/*`：`runtime.ts` 顶层就
 *    `import { invoke } from "@tauri-apps/api/core"`，在 `node
 *    --experimental-strip-types` 下加载即失败。所有 IO 通过 `BlockAiDeps`
 *    注入，真实实现在文件末尾的 `createTerminalAiDeps()` 里惰性组装。
 * 3. **失败不抛到 UI 崩**。除了"没有可用模型"这种用户必须知道的前置条件，
 *    其余异常都归一化成 `{ ok: false, message }`，由卡片就地显示。
 */
import type { CommandBlock } from "../blocks/types.ts";
import { isTerminalStatus } from "../blocks/types.ts";
import { recordAiCall } from "../metrics.ts";
import { buildBlockPrompt, MAX_REQUEST_BYTES, TERMINAL_AI_LIMITS, type TerminalAiMode } from "./prompts.ts";

/**
 * 注入的 IPC 通道。签名与 `src/platform/runtime.ts` 的 `invoke` 一致，
 * 只是为了本模块可单测而不直接依赖它。
 */
export type DeviceInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** 模型选择：与 `AgentModelPicker` 的 `AgentModelSelection` 同构。 */
export interface TerminalModelSelection {
  providerId: string;
  modelId: string;
}

/**
 * provider / model 的最小结构。
 *
 * 刻意不用 `@/platform/api` 的 `AgentProvider`：那个模块顶层依赖 Tauri，
 * 引入它会让本文件在 Node 单测里无法加载。字段按需声明即可满足结构化匹配。
 */
export interface TerminalProviderLike {
  id: string;
  name?: string;
  model?: string;
  models?: Array<{ id: string; name?: string; supportsReasoning?: boolean | null }>;
}

export interface BlockAiDeps {
  invoke: DeviceInvoke;
  /** 列出已配置的供应商；缺省时用真实 `listAgentProviders`。 */
  listProviders?: () => Promise<TerminalProviderLike[]>;
  /** 订阅后端事件；缺省时不流式，只等最终文本。 */
  listen?: <T>(event: string, handler: (payload: T) => void) => Promise<() => void>;
  now?: () => number;
  /** 生成 sessionId / turnId；单测可注入确定性实现。 */
  newId?: (prefix: string) => string;
}

export interface TerminalAiOptions {
  /** 目标模型；未给时自动挑第一个可用供应商的第一个模型。 */
  selection?: TerminalModelSelection;
  /** 关闭脱敏；默认 false，即默认开启（S4）。 */
  disableRedaction?: boolean;
  /** 用户补充问题。 */
  question?: string;
  /** 流式增量回调；需要 `deps.listen` 才会触发。 */
  onDelta?: (chunk: string, aggregated: string) => void;
  /** 取消信号；已 abort 时不再发起请求。 */
  signal?: AbortSignal;
  /** 覆盖输出预算，单测用。 */
  limits?: Partial<typeof TERMINAL_AI_LIMITS>;
}

export type BlockAiErrorCode =
  | "no_model"
  | "unsupported_block"
  | "request_too_large"
  | "aborted"
  | "invoke_failed";

export interface BlockAiResult {
  ok: true;
  mode: TerminalAiMode;
  /** 模型返回的全文。 */
  text: string;
  /** 实际使用的 `"<providerId>:<modelId>"`。 */
  modelId: string;
  /** 请求 message 的 UTF-8 字节数（F43 口径）。 */
  requestBytes: number;
  /** 是否因超限又压缩过一轮输出。 */
  tightened: boolean;
  durationMs: number;
}

export interface BlockAiFailure {
  ok: false;
  mode: TerminalAiMode;
  code: BlockAiErrorCode;
  /** 可直接展示给用户的中文说明。 */
  message: string;
}

export type BlockAiOutcome = BlockAiResult | BlockAiFailure;

/**
 * 从供应商列表里定出要用的模型。
 *
 * 规则与 `QuickToolPalette.tsx:888-893` 保持一致：优先用用户上次的选择，
 * 选择失效（供应商被删、模型下线）时退回第一个可用项，不报错——终端里
 * 点"解释"不该因为模型选择过期而失败。
 */
export function resolveModelChoice(
  providers: TerminalProviderLike[],
  selection?: TerminalModelSelection,
): { modelId: string; supportsReasoning: boolean; providerId: string } | null {
  if (!providers.length) return null;
  const provider = (selection?.providerId
    ? providers.find((item) => item.id === selection.providerId)
    : undefined) || providers[0];
  if (!provider) return null;
  const models = provider.models?.length
    ? provider.models
    : provider.model
      ? [{ id: provider.model, name: provider.model }]
      : [];
  if (!models.length) return null;
  const model = (selection?.modelId ? models.find((item) => item.id === selection.modelId) : undefined) || models[0];
  if (!model?.id) return null;
  return {
    modelId: `${provider.id}:${model.id}`,
    providerId: provider.id,
    supportsReasoning: Boolean(model.supportsReasoning),
  };
}

/**
 * 组装 `agent_one_shot` 的 request。
 *
 * 关键字段都对齐 `QuickToolPalette.tsx:904-925` 的既有调用：
 * - `toolPolicy: "disabled"`：解释任务绝不能触发工具调用（F41 的"不自动执行"）；
 * - `permissionMode: "ask"`：即使后端忽略 `toolPolicy`，也会停在权限确认上；
 * - `sessionId` / `turnId` 独立生成：不污染用户的对话历史线程。
 */
export function buildBlockAiRequest(input: {
  modelId: string;
  supportsReasoning: boolean;
  prompt: { system: string; user: string };
  sessionId: string;
  turnId: string;
}): Record<string, unknown> {
  return {
    modelId: input.modelId,
    // 系统提示词与用户消息拼成单条 message：`agent_one_shot` 只接受一个
    // message 字段，没有独立的 system 通道。
    message: `${input.prompt.system}\n\n---\n\n${input.prompt.user}`,
    jiraContext: null,
    artifacts: [],
    attachments: [],
    workspacePath: null,
    sessionId: input.sessionId,
    turnId: input.turnId,
    permissionMode: "ask",
    analysisMode: false,
    context: [],
    toolPolicy: "disabled",
    skillIds: [],
    requestedTool: null,
    reasoningEffort: input.supportsReasoning ? "low" : null,
    taskPlanMode: "off",
    taskPlanMinSteps: 2,
    // 解释任务本身不落盘任何东西，固定 local。
    executionTarget: { kind: "local", connectionId: null, profileId: null, root: null },
  };
}

/** 只有终态、非直通、且有输出的块才配 AI 入口（F41：不自动刷屏）。 */
export function isBlockAiEligible(block: Pick<CommandBlock, "status" | "passthrough" | "outputText">): boolean {
  if (block.passthrough) return false;
  if (!isTerminalStatus(block.status)) return false;
  return Boolean(block.outputText?.trim());
}

/**
 * 订阅 `agent:event`，把匹配本次请求的 `text_delta` 转发给 `onDelta`。
 *
 * 返回 `unsubscribe`。事件 payload 是弱类型（`AgentEventPayload` 全字段可选），
 * 所以这里同时容忍 snake_case 与 camelCase，并用 `turn_id` 优先匹配——同一
 * session 可能有并发轮次。
 */
async function subscribeDeltas(
  deps: BlockAiDeps,
  sessionId: string,
  turnId: string,
  onDelta: (chunk: string, aggregated: string) => void,
): Promise<() => void> {
  if (!deps.listen) return () => {};
  let aggregated = "";
  let disposed = false;
  try {
    const unsubscribe = await deps.listen<Record<string, unknown>>("agent:event", (payload) => {
      if (disposed) return;
      const type = typeof payload?.type === "string" ? payload.type : "";
      if (type !== "text_delta" && type !== "text") return;
      const eventSession = (payload.session_id ?? payload.sessionId) as string | undefined;
      const eventTurn = (payload.turn_id ?? payload.turnId) as string | undefined;
      if (eventSession !== sessionId) return;
      if (eventTurn && eventTurn !== turnId) return;
      const chunk = typeof payload.text === "string" ? payload.text : "";
      if (!chunk) return;
      aggregated += chunk;
      onDelta(chunk, aggregated);
    });
    return () => {
      disposed = true;
      try { unsubscribe(); } catch { /* 卸载失败不影响主流程 */ }
    };
  } catch {
    // 事件监听不可用（例如浏览器预览）时退化为非流式，属于预期降级。
    return () => {};
  }
}

/** `explain` / `diagnose` 共用的执行体。 */
/** 这些失败是**发起前**就返回的，代表"没请求出去"，不该计入 AI 错误率。 */
const PREFLIGHT_CODES = new Set<BlockAiErrorCode>(["unsupported_block", "aborted", "no_model"]);

/**
 * 对外入口：包一层埋点（T3-7），把每次真实请求的模式、耗时、请求体大小与成败
 * 交给 `metrics` 聚合，用于验证出口条件（请求体 P95 ≤ 16 KB、首 token P50 ≤ 1.5 s）。
 * 埋点自身不改变返回值，任何异常都不允许影响 AI 主流程。
 */
export async function runBlockAi(
  block: CommandBlock,
  mode: TerminalAiMode,
  deps: BlockAiDeps,
  options: TerminalAiOptions = {},
): Promise<BlockAiOutcome> {
  const now = deps.now ?? (() => Date.now());
  const startedAt = now();
  const outcome = await runBlockAiAttempt(block, mode, deps, options);
  // 前置拒绝（块不可用 / 已取消 / 没配模型）没有真正发请求，计数会污染错误率。
  if (!(outcome.ok === false && PREFLIGHT_CODES.has(outcome.code))) {
    try {
      recordAiCall({
        mode,
        ok: outcome.ok,
        durationMs: Math.max(0, now() - startedAt),
        requestBytes: outcome.ok ? outcome.requestBytes : undefined,
      });
    } catch {
      // 埋点失败静默丢弃：诊断能力不能因为统计坏掉而不可用。
    }
  }
  return outcome;
}

async function runBlockAiAttempt(
  block: CommandBlock,
  mode: TerminalAiMode,
  deps: BlockAiDeps,
  options: TerminalAiOptions = {},
): Promise<BlockAiOutcome> {
  if (!isBlockAiEligible(block)) {
    return {
      ok: false,
      mode,
      code: "unsupported_block",
      message: block.passthrough
        ? "交互式程序（直通模式）不支持 AI 解释。"
        : "这条命令还没有结果，稍后再试。",
    };
  }
  if (options.signal?.aborted) {
    return { ok: false, mode, code: "aborted", message: "已取消。" };
  }

  const now = deps.now ?? (() => Date.now());
  const newId = deps.newId ?? ((prefix: string) => `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`);
  const startedAt = now();

  const listProviders = deps.listProviders;
  if (!listProviders) {
    return { ok: false, mode, code: "no_model", message: "未接入模型供应商列表，无法发起 AI 请求。" };
  }
  let providers: TerminalProviderLike[] = [];
  try {
    providers = await listProviders();
  } catch {
    providers = [];
  }
  const choice = resolveModelChoice(providers, options.selection);
  if (!choice) {
    return { ok: false, mode, code: "no_model", message: "请先在设置中配置模型供应商，再使用 AI 解释。" };
  }

  const prompt = buildBlockPrompt({
    block,
    mode,
    question: options.question,
    redact: options.disableRedaction ? { serial: false, ip: false, homePath: false, token: false } : undefined,
    limits: options.limits,
  });

  // 提示词本身已按 `MAX_REQUEST_BYTES` 压缩过一轮；这里只做兜底拦截，
  // 避免把明显超限的请求打出去再等后端拒绝。
  if (prompt.bytes > MAX_REQUEST_BYTES * 2) {
    return {
      ok: false,
      mode,
      code: "request_too_large",
      message: `命令或输出过长（约 ${Math.round(prompt.bytes / 1024)} KB），已停止发送。请缩短命令后重试。`,
    };
  }

  const sessionId = `terminal-ai-${block.id}`;
  const turnId = newId("terminal-ai-turn");
  const request = buildBlockAiRequest({
    modelId: choice.modelId,
    supportsReasoning: choice.supportsReasoning,
    prompt,
    sessionId,
    turnId,
  });

  const unsubscribe = options.onDelta
    ? await subscribeDeltas(deps, sessionId, turnId, options.onDelta)
    : () => {};

  try {
    const response = await deps.invoke<{ text?: string }>("agent_one_shot", { request });
    const text = (response?.text ?? "").trim();
    if (!text) {
      return { ok: false, mode, code: "invoke_failed", message: "模型没有返回内容，请重试。" };
    }
    return {
      ok: true,
      mode,
      text,
      modelId: choice.modelId,
      requestBytes: prompt.bytes,
      tightened: prompt.tightened,
      durationMs: Math.max(0, now() - startedAt),
    };
  } catch (error) {
    return { ok: false, mode, code: "invoke_failed", message: describeInvokeError(error) };
  } finally {
    unsubscribe();
  }
}

/** 把后端错误转成一句用户能懂的话；原文附在括号里便于排查。 */
function describeInvokeError(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const detail = raw.trim();
  if (!detail) return "AI 请求失败，请稍后重试。";
  if (/timeout|timed out/i.test(detail)) return `AI 请求超时，请重试。（${detail}）`;
  if (/401|403|unauthor|forbidden|api key/i.test(detail)) return `模型鉴权失败，请检查供应商配置。（${detail}）`;
  if (/429|rate limit|quota/i.test(detail)) return `模型限流或额度不足，请稍后重试。（${detail}）`;
  return `AI 请求失败：${detail}`;
}

/** 解释一条命令（用户主动点击；成功/失败都可以问）。 */
export function explainBlock(
  block: CommandBlock,
  deps: BlockAiDeps,
  options?: TerminalAiOptions,
): Promise<BlockAiOutcome> {
  return runBlockAi(block, "explain", deps, options);
}

/** 诊断一条失败的命令（F41 的失败提示只引导到这里，不自动发起）。 */
export function diagnoseBlock(
  block: CommandBlock,
  deps: BlockAiDeps,
  options?: TerminalAiOptions,
): Promise<BlockAiOutcome> {
  return runBlockAi(block, "diagnose", deps, options);
}
