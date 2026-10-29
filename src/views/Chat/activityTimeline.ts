import type { AgentEventPayload } from "@/platform/chatApi";
import { appendAssistantStreamText, projectAssistantSnapshotText } from "./assistantText.ts";

export type ActivityTimelineEntry<T> =
  | { type: "text"; id: string; text: string; order?: number }
  | { type: "reasoning"; id: string; text: string; order?: number }
  | { type: "user"; id: string; text: string; order?: number }
  | { type: "tools"; id: string; activities: T[]; order?: number };

/** 时间线条目按事件序号排位；没有序号的条目（补录的活动）沉到最后。 */
export const TRAILING_TIMELINE_ORDER = Number.MAX_SAFE_INTEGER;

export function timelineEntryOrder(entry: ActivityTimelineEntry<unknown>): number {
  return typeof entry.order === "number" && Number.isFinite(entry.order) ? entry.order : TRAILING_TIMELINE_ORDER;
}

export function timelineTextIsStreaming<T>(entry: ActivityTimelineEntry<T>, timeline: ActivityTimelineEntry<T>[], pending: boolean) {
  return pending && entry.type === "text" && timeline.at(-1)?.id === entry.id;
}

/** One owner for the mutable tail. Settled entries never disappear on a new call. */
export function splitActivityTimeline<T extends { id: string; status: string }>(entries: ActivityTimelineEntry<T>[], pending: boolean) {
  const committed: ActivityTimelineEntry<T>[] = [];
  const live: ActivityTimelineEntry<T>[] = [];
  for (const entry of entries) {
    if (pending && entry.type === "reasoning" && entry === entries.at(-1)) {
      live.push(entry);
    } else if (pending && entry.type === "tools") {
      const running = entry.activities.filter(a => a.status === "running" || a.status === "pending");
      const settled = entry.activities.filter(a => a.status !== "running" && a.status !== "pending");
      if (settled.length) committed.push({ ...entry, id: `${entry.id}:settled`, activities: settled });
      if (running.length) live.push({ ...entry, id: `${entry.id}:live`, activities: running });
    } else {
      committed.push(entry.type === "tools" ? { ...entry, id: `${entry.id}:settled` } : entry);
    }
  }
  return { committed, live };
}

/** Keep model explanations beside the tool round they describe, including history pages. */
export function buildActivityTimeline<T extends { id: string }>(
  events: AgentEventPayload[],
  activities: T[],
  options: { pending: boolean; failed: boolean; includeResultText?: boolean },
  resolveActivity: (event: AgentEventPayload) => T | undefined,
): ActivityTimelineEntry<T>[] {
  const entries: ActivityTimelineEntry<T>[] = [];
  const rendered = new Set<string>();
  let prose: { type: "text" | "reasoning"; id: string; text: string; order: number } | undefined;
  let tools: T[] = [];
  let toolsOrder = TRAILING_TIMELINE_ORDER;
  const flushProse = () => {
    if (prose?.text.trim()) entries.push(prose);
    prose = undefined;
  };
  const flushTools = () => {
    if (tools.length) entries.push({ type: "tools", id: tools[0].id, activities: tools, order: toolsOrder });
    tools = [];
    toolsOrder = TRAILING_TIMELINE_ORDER;
  };
  const ordered = [...events].sort((a, b) => Number(a.sequence ?? 0) - Number(b.sequence ?? 0));
  const resultSnapshots = new Set(projectAssistantSnapshotText(ordered, options.failed || !options.pending).resultEvents);
  // A snapshot item is complete text. Overlapping pages may contain several
  // revisions of it; only its newest revision belongs in the timeline.
  const latestSnapshots = new Map<unknown, AgentEventPayload>();
  for (const event of ordered) {
    if (event.type === "snapshot_text") latestSnapshots.set(event.eventId ?? event.event_id ?? event, event);
  }
  ordered.forEach((event, index) => {
    const id = String(event.eventId || event.event_id || `entry-${event.sequence ?? index}`);
    // 条目在 turn 内的时间位置：事件序号（历史分页与实时流都带），缺失时退回遍历下标。
    const position = typeof event.sequence === "number" && Number.isFinite(event.sequence) ? event.sequence : index;
    if (event.type === "snapshot_user_message" && typeof event.text === "string") {
      flushProse();
      flushTools();
      entries.push({ type: "user", id, text: event.text, order: position });
      return;
    }
    if (["text_delta", "snapshot_text", "reasoning_delta"].includes(event.type || "") && typeof event.text === "string") {
      if (event.origin === "runtime_fallback") return;
      if (event.type === "text_delta" && !options.includeResultText && (!options.pending || !activities.length)) return;
      if (event.type === "snapshot_text" && ((!options.includeResultText && resultSnapshots.has(event))
        || latestSnapshots.get(event.eventId ?? event.event_id ?? event) !== event)) return;
      // Older snapshots merged runtime notes into provider summaries. Remove
      // only the exact generated phrases, preserving genuine model text.
      const displayText = event.type === "reasoning_delta" && event.origin == null
        ? event.text.replace(/(?:按你选择的快捷指令，)?我先(?:运行一个受控命令，获取需要的结果|执行一个工具，补充必要信息)。/g, "") : event.text;
      if (!displayText.trim()) return;
      flushTools();
      const type = event.type === "reasoning_delta" ? "reasoning" : "text";
      if (prose?.type !== type) flushProse();
      if (!prose) prose = { type, id, text: "", order: position };
      if (event.paragraph_start === true && prose.text) prose.text += "\n\n";
      prose.text = appendAssistantStreamText(prose.text, displayText);
      return;
    }
    if (event.type !== "tool_call_pending" && event.type !== "tool_call_started") return;
    const activity = resolveActivity(event);
    if (!activity || rendered.has(activity.id)) return;
    if (!tools.length) toolsOrder = position;
    flushProse();
    rendered.add(activity.id);
    tools.push(activity);
  });
  flushProse();
  flushTools();
  for (const activity of activities) {
    if (!rendered.has(activity.id)) entries.push({ type: "tools", id: activity.id, activities: [activity], order: TRAILING_TIMELINE_ORDER });
  }
  return entries;
}
