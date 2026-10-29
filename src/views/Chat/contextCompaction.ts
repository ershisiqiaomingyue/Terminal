import type { AgentEventPayload, AgentThreadSnapshot } from "../../platform/chatApi.ts";

export type ContextCompaction = {
  status: "running" | "completed" | "failed" | "interrupted";
  progress: number | undefined;
  processedMessages?: number;
  summaryCharacters?: number;
  before?: number;
  after?: number;
};

/** 时间线里的压缩记录：额外带一次压缩发生的事件序号，便于插回发生位置。 */
export type TimelineContextCompaction = ContextCompaction & { id: string; order: number };

const tokenCount = (value: unknown) => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;

function progressValue(event: AgentEventPayload, status: ContextCompaction["status"], previous?: number): number | undefined {
  if (status === "completed") return 100;
  const total = tokenCount(event.compaction_total_units ?? event.compactionTotalUnits);
  const processed = tokenCount(event.compaction_completed_units ?? event.compactionCompletedUnits);
  const value = total && processed != null ? processed / total * 100 : event.compaction_progress ?? event.compactionProgress;
  if (typeof value !== "number" || !Number.isFinite(value)) return previous;
  return Math.min(99, Math.max(0, previous ?? 0, Math.round(value)));
}

/** A compression's start and result share one status record across snapshots. */
export function timelineContextCompactions(events: AgentEventPayload[], pending: boolean): TimelineContextCompaction[] {
  const records: TimelineContextCompaction[] = [];
  for (const [index, event] of events.entries()) {
    if (event.type !== "context") continue;
    const status = event.compaction_status ?? event.compactionStatus
      ?? ((event.context_compressed === true || event.contextCompressed === true) ? "completed" : undefined);
    if (status !== "running" && status !== "completed" && status !== "failed" && status !== "interrupted") continue;
    const suppliedId = event.compaction_id ?? event.compactionId;
    const previous = suppliedId ? records.find(record => record.id === String(suppliedId))
      : records.at(-1)?.status === "running" ? records.at(-1) : undefined;
    const record: TimelineContextCompaction = previous ?? {
      id: String(suppliedId ?? event.eventId ?? event.sequence ?? `compaction-${records.length}`),
      // 插入位置用「这次压缩第一条事件」的序号；缺序号时退回事件下标。
      order: typeof event.sequence === "number" && Number.isFinite(event.sequence) ? event.sequence : index,
      status,
      progress: undefined,
    };
    // Replayed/stale running events must not restart a finished compression.
    if (previous && previous.status !== "running" && status === "running") continue;
    record.status = status;
    record.progress = progressValue(event, status, record.progress);
    const processedMessages = tokenCount(event.compaction_processed_messages ?? event.compactionProcessedMessages);
    const summaryCharacters = tokenCount(event.compaction_summary_chars ?? event.compactionSummaryChars);
    if (processedMessages != null) record.processedMessages = Math.max(record.processedMessages ?? 0, processedMessages);
    if (summaryCharacters != null) record.summaryCharacters = Math.max(record.summaryCharacters ?? 0, summaryCharacters);
    record.before = tokenCount(event.before_context_tokens ?? event.beforeContextTokens) ?? record.before;
    record.after = status === "completed" ? tokenCount(event.after_context_tokens ?? event.afterContextTokens ?? event.context_tokens ?? event.contextTokens) : undefined;
    if (!previous) records.push(record);
  }
  return records.map(record => record.status === "running" && !pending ? { ...record, status: "interrupted" } : record);
}

export function snapshotContextCompaction(snapshot: AgentThreadSnapshot | null): ContextCompaction | null {
  if (!snapshot?.activeTurn) return null;
  const activeTurn = snapshot.activeTurn;
  const events = snapshot.items.flatMap(item => {
    if (item.kind !== "context") return [];
    const event = item.data;
    const turnId = event.turn_id ?? event.turnId;
    return !turnId || turnId === activeTurn.id ? [{ ...event, type: "context" }] : [];
  });
  const latest = timelineContextCompactions(events, activeTurn.status === "running").at(-1);
  if (!latest) return null;
  const { id: _id, order: _order, ...compaction } = latest;
  return compaction;
}
