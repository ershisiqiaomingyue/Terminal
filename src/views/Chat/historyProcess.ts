import type { AgentEventPayload, AgentThreadItem } from "@/platform/chatApi";
import { projectLiveThreadMessages } from "./threadSnapshotMessages.ts";

function withoutLegacyAggregate(ordered: AgentEventPayload[]) {
  const final = ordered.at(-1)?.item as AgentThreadItem | undefined;
  if (final?.kind !== "agent_message" || !final.id.endsWith(":final")) return ordered;
  const preceding = ordered.slice(0, -1).map((event) => event.item as AgentThreadItem);
  // Only repair the old host's exact aggregate signature. Do not deduplicate
  // ordinary repeated text, partial pages or outputs spanning a steer.
  const users = preceding.filter((item) => item.kind === "user_message");
  if (users.length > 1 || users.some((item) => item.id.includes(":steer:"))) return ordered;
  const messages = preceding.filter((item): item is Extract<AgentThreadItem, { kind: "agent_message" }> => item.kind === "agent_message" && Boolean(item.content));
  const modelText = messages.filter((item) => item.content !== "我先执行一个工具，补充必要信息。");
  return modelText.length >= 2 && modelText.map((item) => item.content).join("") === final.content
    ? ordered.slice(0, -1) : ordered;
}

export function historyProcessEvents(events: AgentEventPayload[]): AgentEventPayload[] {
  const executions = new Map<number, Map<string, AgentEventPayload>>();
  for (const event of events) {
    if (event.type !== "process_item" || !event.item || typeof event.item !== "object") continue;
    const item = event.item as AgentThreadItem;
    if (typeof item.id !== "string") continue;
    const startedAt = Number(event.execution_started_at ?? 0);
    let items = executions.get(startedAt);
    if (!items) executions.set(startedAt, items = new Map());
    const previous = items.get(item.id);
    if (!previous || Number(event.snapshot_revision ?? 0) > Number(previous.snapshot_revision ?? 0)) items.set(item.id, event);
  }
  if (!executions.size) return events.filter((event) => event.type !== "process_item");
  const executionOrder = [...executions].sort(([a], [b]) => a - b);
  const latestExecution = executionOrder.at(-1)?.[0];
  let sequence = 0;
  const projected: AgentEventPayload[] = [];
  for (const [startedAt, items] of executionOrder) {
    const ordered = withoutLegacyAggregate([...items.values()].sort((a, b) => Number(a.process_order ?? 0) - Number(b.process_order ?? 0)));
    const snapshotItems = ordered.map((event) => {
      const item = event.item as AgentThreadItem;
      return { ...item, id: `${startedAt}:${item.id}` };
    });
    const sourceByItemId = new Map(ordered.map((event) => [
      `${startedAt}:${(event.item as AgentThreadItem).id}`,
      event,
    ]));
    const sourceForProjectedEvent = (event: AgentEventPayload) => {
      const ids = [event.callId, event.call_id, event.eventId, event.event_id]
        .filter((value): value is string => typeof value === "string" && Boolean(value));
      for (const id of ids) {
        const direct = sourceByItemId.get(id);
        if (direct) return direct;
        const base = id.replace(/:(?:finished|live)$/, "");
        const normalized = sourceByItemId.get(base);
        if (normalized) return normalized;
      }
      return undefined;
    };
    const resultItems = new Map(ordered.map((event) => [`${startedAt}:${(event.item as AgentThreadItem).id}`,
      startedAt === latestExecution ? event.result_item : false]));
    // The initial request already has its own conversation row. Keep steer
    // messages, including identical text and steers on partial history pages.
    const initialUserIds = new Set(ordered.filter((event) => {
      const item = event.item as AgentThreadItem;
      return item.kind === "user_message" && !item.id.includes(":steer:")
        && (item.id === `user:${event.turn_id}` || /^user:[^:]+$/.test(item.id)
          || event.process_order === 0);
    }).map((event) => `${startedAt}:${(event.item as AgentThreadItem).id}`));
    const messages = projectLiveThreadMessages([], {
      id: String(ordered[0]?.session_id || ordered[0]?.sessionId || ""), revision: 1, status: "completed",
      activeTurn: { id: String(ordered[0]?.turn_id || ordered[0]?.turnId || ""), status: "completed" }, items: snapshotItems,
    });
    for (const message of messages) {
      if (message.role === "user") {
        if (initialUserIds.has(message.id)) continue;
        const source = sourceByItemId.get(message.id);
        projected.push({
          type: "snapshot_user_message",
          eventId: message.id,
          text: message.content,
          execution_started_at: startedAt,
          process_order: source?.process_order,
          sequence: sequence++,
        });
        continue;
      }
      for (const event of message.events || []) {
        const source = sourceForProjectedEvent(event);
        projected.push({
          ...event,
          ...(event.type === "snapshot_text" && typeof resultItems.get(String(event.eventId)) === "boolean"
            ? { history_result: resultItems.get(String(event.eventId)) } : {}),
          execution_started_at: startedAt,
          process_order: source?.process_order,
          sequence: sequence++,
        });
      }
    }
  }
  const terminals = events.filter((event) => ["completed", "cancelled", "error"].includes(event.type || ""));
  const failure = [...terminals].reverse().find((event) => event.type === "error" && typeof event.message === "string");
  if (failure && !projected.some((event) => event.terminalDiagnostic === true && event.status === "failed")) {
    const lastProcessOrder = [...projected].reverse().find((event) => Number.isSafeInteger(event.process_order))?.process_order;
    projected.push({ ...failure, type: "status", status: "failed", terminalDiagnostic: true, sequence: sequence++, process_order: lastProcessOrder });
  }
  return [...projected, ...terminals];
}

export function historyProcessEventsForRange(
  events: AgentEventPayload[],
  processStart?: number,
  processEnd?: number,
) {
  if (processStart === undefined && processEnd === undefined) return events;
  return events.filter((event) => {
    const value = event.process_order ?? event.processOrder;
    const order = typeof value === "number" ? value : Number(value);
    if (!Number.isSafeInteger(order) || order < 0) return false;
    return (processStart === undefined || order >= processStart)
      && (processEnd === undefined || order < processEnd);
  });
}

function positiveTimestamp(value: unknown) {
  const number = typeof value === "number" ? value : Number(value);
  return Number.isFinite(number) && number > 0 ? number : undefined;
}

export function historyProcessTiming(events: AgentEventPayload[]) {
  let startedAt: number | undefined;
  let finishedAt: number | undefined;
  for (const event of events) {
    const start = positiveTimestamp(event.execution_started_at ?? event.executionStartedAt);
    if (start && (!startedAt || start < startedAt)) startedAt = start;
    if (!["completed", "cancelled", "error"].includes(String(event.type || ""))) continue;
    const finish = positiveTimestamp(event.timestampMs ?? event.timestamp_ms);
    if (finish && (!finishedAt || finish > finishedAt)) finishedAt = finish;
  }
  return { startedAt, finishedAt };
}

export function mergeHistoryProcessPage(current: AgentEventPayload[], incoming: AgentEventPayload[]) {
  const byId = new Map<string, AgentEventPayload>();
  [...current, ...incoming].forEach((event, index) => {
    const key = String(event.eventId || event.event_id || `legacy:${event.sequence ?? index}:${event.type}`);
    const previous = byId.get(key);
    if (!previous || Number(event.snapshot_revision ?? 0) >= Number(previous.snapshot_revision ?? 0)) byId.set(key, event);
  });
  return [...byId.values()];
}
