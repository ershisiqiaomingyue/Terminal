import type { AgentEventPayload, AgentThreadSnapshot, SessionUsageTotals } from "../../platform/chatApi.ts";
import type { ChatUsage } from "./types.ts";
import { isDurableTerminalEvent, eventTurnId } from "./historyRecovery.ts";

function tokenNumber(value: unknown): number | undefined {
  if (value == null || value === "") return undefined;
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= 0 ? number : undefined;
}

function costNumber(value: unknown): number | undefined {
  if (value == null || value === "") return undefined;
  const number = Number(value);
  return Number.isFinite(number) && number >= 0 ? number : undefined;
}

export interface TurnPricing {
  inputPricePerMillion?: number | null;
  outputPricePerMillion?: number | null;
  currency?: string | null;
}

export function estimateTurnCost(usage: ChatUsage | undefined, pricing?: TurnPricing): { amount?: number; currency?: string; partial?: boolean } {
  if (!usage) return {};
  const reported = costNumber(usage.totalCost)
    ?? (costNumber(usage.inputCost) != null && costNumber(usage.outputCost) != null ? usage.inputCost! + usage.outputCost! : undefined);
  if (reported != null) return { amount: reported, currency: usage.currency,
    partial: false };
  if (pricing) {
    const input = usageInputTotal(usage);
    const output = usage.output;
    const inputPrice = costNumber(pricing.inputPricePerMillion);
    const outputPrice = costNumber(pricing.outputPricePerMillion);
    if (input == null || output == null || (input > 0 && inputPrice == null) || (output > 0 && outputPrice == null)) return {};
    const amount = (input * (inputPrice ?? 0) + output * (outputPrice ?? 0)) / 1_000_000;
    return Number.isFinite(amount) ? { amount, currency: pricing.currency?.trim() || undefined } : {};
  }
  return {};
}

export function readChatUsage(raw: unknown, includeLatest = true): ChatUsage | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const value = raw as Record<string, unknown>;
  const number = (...keys: string[]) => keys.map((key) => tokenNumber(value[key])).find((item) => item !== undefined);
  const stats = value.cacheStats && typeof value.cacheStats === "object" ? value.cacheStats as Record<string, unknown> : {};
  const providerKind = value.providerKind ?? value.provider_kind ?? stats.providerKind;
  const rate = value.cacheHitRate ?? stats.hitRate;
  const parsed: ChatUsage = {
    providerKind: typeof providerKind === "string" ? providerKind : undefined,
    cacheHitRate: typeof rate === "number" && Number.isFinite(rate) && rate >= 0 && rate <= 1 ? rate : undefined,
    latestRequestUsage: includeLatest && value.latestRequestUsage && typeof value.latestRequestUsage === "object"
      ? readChatUsage(value.latestRequestUsage, false) ?? {} : undefined,
    input: number("input", "inputTokens", "input_tokens"),
    output: number("output", "outputTokens", "output_tokens"),
    reasoning: number("reasoning", "reasoningTokens", "reasoning_tokens"),
    generationTokens: number("generationTokens", "generation_tokens"),
    generationDurationMs: number("generationDurationMs", "generation_duration_ms"),
    generationTimingSource: (value.generationTimingSource ?? value.generation_timing_source) === "request" ? "request" : undefined,
    cacheRead: number("cacheRead", "cachedInputTokens", "cached_input_tokens"),
    cacheCreation: number("cacheCreation", "cacheCreationInputTokens", "cache_creation_input_tokens"),
    contextWindow: number("contextWindow", "context_window"),
    contextTokens: number("contextTokens", "context_tokens"),
    reservedOutputTokens: number("reservedOutputTokens", "reserved_output_tokens"),
    availableTokens: number("availableTokens", "available_tokens"),
    inputCost: costNumber(value.inputCost ?? value.input_cost),
    outputCost: costNumber(value.outputCost ?? value.output_cost),
    totalCost: costNumber(value.totalCost ?? value.total_cost),
    currency: typeof value.currency === "string" ? value.currency.trim() || undefined : undefined,
  };
  return Object.values(parsed).some((item) => item !== undefined) ? parsed : undefined;
}

export function restoreHistoryUsage(record: { usage?: unknown; metadata?: Record<string, unknown> }, events: AgentEventPayload[], messages: Array<{ role: string; turnId?: string }>) {
  const terminal = [...events].reverse().find(isDurableTerminalEvent);
  const savedTurnId = typeof record.metadata?.usageTurnId === "string" ? record.metadata.usageTurnId : undefined;
  const latestTurnId = [...messages].reverse().find((message) => message.role === "assistant")?.turnId;
  const turnId = eventTurnId(terminal || {}) || savedTurnId || latestTurnId;
  // A terminal without usage belongs to that turn, not the previous known turn.
  const saved = !terminal || (savedTurnId ? savedTurnId === turnId : latestTurnId === turnId)
    ? readChatUsage(record.usage) : undefined;
  return { usage: mergeChatUsage(saved, readChatUsage(terminal?.usage)), usageTurnId: turnId };
}

export function mergeChatUsage(base: ChatUsage | undefined, reported: ChatUsage | undefined): ChatUsage | undefined {
  if (!reported) return base;
  const merged = { ...base, ...Object.fromEntries(Object.entries(reported || {}).filter(([, value]) => value !== undefined)) };
  if (reported.generationTokens != null || reported.generationDurationMs != null) merged.generationTimingSource = reported.generationTimingSource;
  return merged;
}

export function usageInputTotal(usage?: ChatUsage): number | undefined {
  if (usage?.input == null) return undefined;
  return usage.providerKind === "anthropic"
    ? usage.input + (usage.cacheRead ?? 0) + (usage.cacheCreation ?? 0)
    : usage.input;
}

export function outputSpeedForDisplay(usage?: ChatUsage) {
  const tokens = tokenNumber(usage?.generationTokens);
  const duration = tokenNumber(usage?.generationDurationMs);
  // Legacy visible-stream timing excludes hidden reasoning and buffered output.
  if (usage?.generationTimingSource !== "request" || tokens == null || duration == null || duration <= 0) return undefined;
  const rate = tokens * 1000 / duration;
  return Number.isFinite(rate) ? { rate } : undefined;
}

export function usageCacheHitRate(usage?: ChatUsage): number | undefined {
  const input = usageInputTotal(usage);
  const cached = usage?.cacheRead;
  // Prefer counters from the same request over a rounded provider percentage.
  if (input != null && cached != null) {
    return input > 0 && cached <= input ? cached / input : undefined;
  }
  return usage?.cacheHitRate;
}

/**
 * P9-C 增量（对齐 P8-D 的处理方式）：cached > input 的口径错配读数。
 * 既有测试锁定它**不得显示为命中率**（不显示可能错误的 100%）——保持
 * undefined；这里把它从「未上报」区分为「读数异常」，让用户知道统计
 * 拿到了但自相矛盾，而不是以为没统计。
 */
export function cacheStatAnomaly(usage?: ChatUsage): boolean {
  const input = usageInputTotal(usage);
  const cached = usage?.cacheRead;
  return input != null && cached != null && input > 0 && cached > input;
}

export function formatCacheHitRate(rate?: number): string {
  if (rate == null || !Number.isFinite(rate) || rate < 0 || rate > 1) return "未上报";
  if (rate === 0 || rate === 1) return `${rate * 100}%`;
  const percent = Math.round(rate * 1000) / 10;
  if (percent === 100) return ">99.9%";
  if (percent === 0) return "<0.1%";
  return `${percent}%`;
}

export function latestRequestCacheHitRate(usage?: ChatUsage): number | undefined {
  // Older records contain only turn totals; do not present those as one request.
  return usageCacheHitRate(usage?.latestRequestUsage);
}

function hasMeasuredUsage(usage: ChatUsage | undefined) {
  if (!usage) return false;
  const { latestRequestUsage, ...counters } = usage;
  if (Object.values(counters).some((value) => value !== undefined)) return true;
  // A lone latest-request sample still carries the cache ratio of a request,
  // even when no turn counter arrived with it.
  return latestRequestUsage != null && Object.values(latestRequestUsage).some((value) => value !== undefined);
}

export function visibleTurnUsage(persisted: ChatUsage | undefined, persistedTurnId: string | undefined, snapshot: AgentThreadSnapshot | null): ChatUsage | undefined {
  if (!snapshot?.activeTurn) return persisted;
  const running = snapshot.activeTurn.status === "running";
  const live = readChatUsage(snapshot.runtime?.usage);
  if (running) return live;
  if (persistedTurnId === snapshot.activeTurn.id) {
    return mergeChatUsage(persisted, live);
  }
  // Another turn owns the persisted counters, so they must not be borrowed.
  // What the snapshot measured for its own turn is still real, including a
  // report that only carries the latest request usage: dropping it made the
  // cache ratio fall back to "—" the moment a turn stopped.
  return hasMeasuredUsage(live) ? live : undefined;
}

export function sessionUsageWithLive(totals: SessionUsageTotals | undefined, usage: ChatUsage | undefined, turn?: AgentThreadSnapshot["activeTurn"]) {
  const executions = totals?.executions ?? [];
  const included = turn && executions.find((execution) => execution.turnId === turn.id
    && (execution.startedAt === 0 || execution.startedAt === turn.startedAtMs));
  const liveInput = turn ? usageInputTotal(usage) : undefined;
  const liveOutput = turn ? usage?.output : undefined;
  const liveReasoning = turn ? usage?.reasoning : undefined;
  const addLive = (total: number | null | undefined, live: number | undefined, previous: number | null | undefined) => {
    if (total == null && live == null) return undefined;
    return (total ?? 0) + Math.max(0, (live ?? 0) - (previous ?? 0));
  };
  return {
    input: addLive(totals?.inputTokens, liveInput, included?.inputTokens),
    output: addLive(totals?.outputTokens, liveOutput, included?.outputTokens),
    reasoning: addLive(totals?.reasoningTokens, liveReasoning, included?.reasoningTokens),
    inputPartial: executions.some((execution) => execution.inputTokens == null),
    outputPartial: executions.some((execution) => execution.outputTokens == null),
    reasoningPartial: executions.some((execution) => execution.reasoningTokens == null),
  };
}
