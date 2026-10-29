import type { AgentThreadSnapshot } from "../../platform/chatApi.ts";

type ContextUsageEvent = Record<string, unknown>;
type HistoricalMessage = { role: string; turnId?: string; events?: ContextUsageEvent[] };

/** Keep legacy cached readings when a bounded native history omits them. */
export function retainContextUsageEvents(events: ContextUsageEvent[]): ContextUsageEvent[] {
  const byIdentity = new Map<string, { event: ContextUsageEvent; timestamp: number; order: number }>();
  events.forEach((event, order) => {
    const turn = event.turn_id ?? event.turnId;
    if (event.type !== "context" || typeof turn !== "string" || typeof event.model_id !== "string" || typeof event.provider_id !== "string") return;
    if (!readContextUsage([event], () => true, turn)) return;
    const rawTimestamp = event.timestamp_ms ?? event.timestampMs;
    const timestamp = typeof rawTimestamp === "number" && Number.isFinite(rawTimestamp) && rawTimestamp >= 0 ? rawTimestamp : 0;
    const key = JSON.stringify([turn, event.model_id, event.provider_id]);
    const previous = byIdentity.get(key);
    if (!previous || timestamp >= previous.timestamp) byIdentity.set(key, { event, timestamp, order });
  });
  return [...byIdentity.values()].sort((left, right) => left.timestamp - right.timestamp || left.order - right.order).slice(-16).map(({ event }) => event);
}

type ContextReading = { system: number; tools: number; total: number; window?: number; compactionTriggerTokens?: number; modelId?: string };
type ContextMatcher = (data: ContextUsageEvent) => boolean;

export type ContextUsageScan = {
  /** 最新一条自洽的读数；没有时为 null（UI 显示未知/估算）。 */
  reading: ContextReading | null;
  /**
   * 最新一条自相矛盾的读数（P8-D）：system + tools > total。此前这类读数被
   * 静默丢弃，用户只看到空白；现在作为异常透出，UI 在没有有效读数时显示
   * 「数据异常」标记。缺字段的读数不算异常（不完整 ≠ 矛盾）。
   */
  anomaly: ContextReading | null;
};

function readContextScan(events: ContextUsageEvent[], matches: ContextMatcher, turnId?: string): ContextUsageScan {
  let latest: ContextReading | null = null;
  let latestTimestamp = -1;
  let latestAnomaly: ContextReading | null = null;
  let latestAnomalyTimestamp = -1;
  for (const data of events) {
    if ((data.type != null && data.type !== "context") || !matches(data)) continue;
    const eventTurnId = data.turn_id ?? data.turnId;
    if (turnId && eventTurnId !== turnId) continue;
    const count = (value: unknown) => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
    const system = count(data.system_prompt_tokens);
    const tools = count(data.tool_definition_tokens);
    const total = count(data.context_tokens);
    const window = count(data.context_window);
    const timestamp = count(data.timestamp_ms ?? data.timestampMs) ?? 0;
    const compactionTrigger = count(data.compaction_trigger_tokens ?? data.compactionTriggerTokens);
    const modelId = typeof data.model_id === "string" && data.model_id ? data.model_id : undefined;
    const reading = (base: ContextReading): ContextReading => ({
      ...base,
      ...(compactionTrigger == null ? {} : { compactionTriggerTokens: compactionTrigger }),
      ...(modelId == null ? {} : { modelId }),
    });
    if (system == null || tools == null || total == null) continue;
    if (system + tools > total) {
      if (timestamp >= latestAnomalyTimestamp) {
        latestAnomalyTimestamp = timestamp;
        latestAnomaly = reading({ system, tools, total, ...(window == null ? {} : { window }) });
      }
      continue;
    }
    if (timestamp < latestTimestamp) continue;
    latestTimestamp = timestamp;
    latest = reading({ system, tools, total, ...(window == null ? {} : { window }) });
  }
  return { reading: latest, anomaly: latestAnomaly };
}

function readContextUsage(events: ContextUsageEvent[], matches: ContextMatcher, turnId?: string): ContextReading | null {
  return readContextScan(events, matches, turnId).reading;
}

/**
 * A provider model can be removed from the configured list while its historical
 * readings live on; the session then falls back to another model id and an exact
 * match would hide a breakdown that is still meaningful. Fall back to the newest
 * reading of the same provider, keeping the turn filter so numbers never cross turns.
 * Third level (model-agnostic): a conversation can be reopened while the global
 * model picker points at another provider that never reported for this turn —
 * the turn's own newest reading is still the honest breakdown; showing "—" would
 * make users think history was never stored. Anomalies follow the same
 * exact→provider→any fallback (P8-D).
 */
function readContextScanWithFallback(events: ContextUsageEvent[], modelId: string, providerId: string, turnId?: string): ContextUsageScan {
  const exact = readContextScan(events, (data) => data.model_id === modelId && data.provider_id === providerId, turnId);
  if (exact.reading) return exact;
  const provider = readContextScan(events, (data) => data.provider_id === providerId, turnId);
  if (provider.reading) return { reading: provider.reading, anomaly: exact.anomaly ?? provider.anomaly };
  const any = readContextScan(events, () => true, turnId);
  return { reading: any.reading, anomaly: exact.anomaly ?? provider.anomaly ?? any.anomaly };
}

function readContextUsageWithFallback(events: ContextUsageEvent[], modelId: string, providerId: string, turnId?: string) {
  return readContextScanWithFallback(events, modelId, providerId, turnId).reading;
}

export function contextUsageDetails(snapshot: AgentThreadSnapshot | null, modelId: string, providerId: string, turnId = snapshot?.activeTurn?.id) {
  const items = snapshot?.items ?? [];
  for (let index = items.length - 1; index >= 0; index--) {
    const item = items[index];
    if (item.kind !== "context") continue;
    const details = readContextUsageWithFallback([item.data], modelId, providerId, turnId);
    if (details) return details;
  }
  return null;
}

export function historicalContextUsageDetails(messages: HistoricalMessage[], modelId: string, providerId: string, turnId?: string, persistedEvents: ContextUsageEvent[] = []) {
  if (!turnId) return null;
  return readContextUsageWithFallback([...messages.flatMap((message) => message.events || []), ...persistedEvents], modelId, providerId, turnId);
}

/**
 * Newest reading of any turn, used only as a display fallback while the current
 * turn has not reported yet (a fresh turn clears the live items). The ring and
 * the breakdown must keep the last measured numbers instead of collapsing to a
 * character estimate or to em dashes.
 */
export function latestContextUsageDetails(messages: HistoricalMessage[], modelId: string, providerId: string, persistedEvents: ContextUsageEvent[] = []) {
  return readContextUsageWithFallback([...messages.flatMap((message) => message.events || []), ...persistedEvents], modelId, providerId);
}

/** Billing can still refer to the previous turn after a queued turn starts. */
export function resolveContextUsageDetails(snapshot: AgentThreadSnapshot | null, messages: HistoricalMessage[], modelId: string, providerId: string, usageTurnId?: string, persistedEvents: ContextUsageEvent[] = []) {
  return resolveContextUsageScan(snapshot, messages, modelId, providerId, usageTurnId, persistedEvents).reading;
}

/**
 * Same turn-resolution chain as `resolveContextUsageDetails` (live items →
 * historical events → any-turn fallback), but also carries the newest
 * contradictory reading so the UI can show a 数据异常 marker instead of a
 * silent blank (P8-D).
 */
export function resolveContextUsageScan(snapshot: AgentThreadSnapshot | null, messages: HistoricalMessage[], modelId: string, providerId: string, usageTurnId?: string, persistedEvents: ContextUsageEvent[] = []): ContextUsageScan {
  let latestMessageTurn: string | undefined;
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index].turnId) { latestMessageTurn = messages[index].turnId; break; }
  }
  const activeTurn = snapshot?.activeTurn;
  const running = activeTurn?.status === "running";
  const turnId = running ? activeTurn.id : latestMessageTurn || activeTurn?.id || usageTurnId;
  const live = scanSnapshotContextItems(snapshot, modelId, providerId, turnId);
  if (live.reading) return live;
  const historical = turnId
    ? readContextScanWithFallback([...messages.flatMap((message) => message.events || []), ...persistedEvents], modelId, providerId, turnId)
    : { reading: null, anomaly: null };
  if (historical.reading) return historical;
  // A running turn stays authoritative: borrowing an older turn here would mix
  // the previous prompt with the new one. Once it stopped, the newest measured
  // breakdown of this session must stay on screen instead of turning into "—".
  // The chat page keeps that reading visible through latestContextUsageDetails.
  if (running) return { reading: null, anomaly: live.anomaly ?? historical.anomaly };
  const latest = readContextScanWithFallback([...messages.flatMap((message) => message.events || []), ...persistedEvents], modelId, providerId);
  return { reading: latest.reading, anomaly: live.anomaly ?? historical.anomaly ?? latest.anomaly };
}

/** `contextUsageDetails` 的镜像：逐条走快照 context item，同时保留最新一条矛盾读数。 */
function scanSnapshotContextItems(snapshot: AgentThreadSnapshot | null, modelId: string, providerId: string, turnId?: string): ContextUsageScan {
  const items = snapshot?.items ?? [];
  let anomaly: ContextReading | null = null;
  for (let index = items.length - 1; index >= 0; index--) {
    const item = items[index];
    if (item.kind !== "context") continue;
    const scan = readContextScanWithFallback([item.data], modelId, providerId, turnId);
    if (scan.reading) return scan;
    anomaly ??= scan.anomaly;
  }
  return { reading: null, anomaly };
}
