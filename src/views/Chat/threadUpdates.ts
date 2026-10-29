import type { AgentEventPayload, AgentThreadItem, AgentThreadSnapshot } from "@/platform/chatApi";

type TurnStatus = "running" | "completed" | "failed" | "cancelled";
type ThreadStatus = "idle" | "running" | "waiting_for_permission" | "completed" | "failed" | "cancelled";

type ThreadEventBase = { thread_id: string; turn_id: string };
export type ThreadEvent =
  | (ThreadEventBase & { event: "turn_started" })
  | (ThreadEventBase & { event: "item_added"; item: AgentThreadItem })
  | (ThreadEventBase & { event: "tool_call_input_delta"; call_id: string; delta: string })
  | (ThreadEventBase & { event: "tool_call_output"; call_id: string; output: unknown })
  | (ThreadEventBase & { event: "agent_delta"; item_id: string; text: string })
  | (ThreadEventBase & { event: "agent_final"; item_id: string; content: string; preserve_streamed?: boolean })
  | (ThreadEventBase & { event: "permission_requested"; request_id: string; tool_name: string; expires_at_ms?: number | null })
  | (ThreadEventBase & { event: "permission_resolved"; request_id: string })
  | (ThreadEventBase & { event: "turn_completed"; status: TurnStatus; error?: string | null })
  | (ThreadEventBase & { event: "runtime_updated"; activity: string; tool_call_count: number })
  | (ThreadEventBase & { event: "usage_updated"; usage: Record<string, unknown> })
  | (ThreadEventBase & { event: "execution_target_updated"; execution_target?: unknown });

export type AgentThreadDeltaUpdate = {
  kind: "delta";
  id: string;
  base_revision: number;
  revision: number;
  metadata: AgentThreadSnapshot;
  events: ThreadEvent[];
};

export type AgentThreadUpdate =
  | { kind: "snapshot"; snapshot: AgentThreadSnapshot }
  | AgentThreadDeltaUpdate;

const MAX_SNAPSHOT_TOOL_OUTPUT_BYTES = 128 * 1024;
const MAX_BUFFERED_DELTAS = 256;
const MAX_BUFFERED_DELTA_BYTES = 1024 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const hasOwn = (value: object, key: PropertyKey) => Object.prototype.hasOwnProperty.call(value, key);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRevision(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}

function isTurnStatus(value: unknown): value is TurnStatus {
  return value === "running" || value === "completed" || value === "failed" || value === "cancelled";
}

function isThreadStatus(value: unknown): value is ThreadStatus {
  return value === "idle" || value === "running" || value === "waiting_for_permission"
    || value === "completed" || value === "failed" || value === "cancelled";
}

function isThreadSnapshot(value: unknown): value is AgentThreadSnapshot {
  if (!isRecord(value) || typeof value.id !== "string" || !isRevision(value.revision)
    || !isThreadStatus(value.status) || !Array.isArray(value.items)) return false;
  if (value.activeTurn !== undefined && value.activeTurn !== null) {
    if (!isRecord(value.activeTurn) || typeof value.activeTurn.id !== "string"
      || !isTurnStatus(value.activeTurn.status)) return false;
  }
  return true;
}

function isThreadItem(value: unknown): value is AgentThreadItem {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.kind !== "string") return false;
  switch (value.kind) {
    case "user_message":
      return typeof value.content === "string"
        && (!hasOwn(value, "attachments") || Array.isArray(value.attachments))
        && (!hasOwn(value, "artifacts") || Array.isArray(value.artifacts));
    case "agent_message":
      return typeof value.content === "string" && typeof value.pending === "boolean";
    case "tool_call":
      return typeof value.name === "string" && typeof value.is_error === "boolean" && hasOwn(value, "input");
    case "task_plan":
      return typeof value.goal === "string" && Array.isArray(value.steps);
    case "file_changes":
    case "context":
    case "final_report":
    case "timeline_event":
      return hasOwn(value, "data");
    default:
      return false;
  }
}

function boundedLog(text: string, remaining: number) {
  const bytes = encoder.encode(text);
  if (bytes.length <= remaining) return { text, truncated: false };
  let end = Math.max(0, Math.min(remaining, bytes.length));
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
  return { text: decoder.decode(bytes.subarray(0, end)), truncated: true };
}

function mergeLiveToolOutput(current: unknown, incoming: unknown): unknown {
  if (!isRecord(incoming)) return incoming;
  const previous = isRecord(current) ? current : {};
  let text = typeof previous.text === "string" ? previous.text : "";
  let truncated = previous.truncated === true;
  if (!truncated && incoming.phase === "chunk" && typeof incoming.text === "string") {
    const next = boundedLog(incoming.text, Math.max(0, MAX_SNAPSHOT_TOOL_OUTPUT_BYTES - encoder.encode(text).length));
    text += next.text;
    truncated = next.truncated;
  }
  return { ...incoming, text, truncated };
}

function validEventEnvelope(event: unknown, threadId: string, activeTurnId: string | null) {
  if (!isRecord(event) || event.thread_id !== threadId || typeof event.event !== "string") return false;
  if (event.event === "turn_started") return typeof event.turn_id === "string" && event.turn_id.length > 0;
  return typeof event.turn_id === "string" && event.turn_id === activeTurnId;
}

/** Replay one Rust ThreadUpdate::Delta with the same visible item semantics as ThreadSnapshot::apply. */
export function applyThreadUpdate(current: AgentThreadSnapshot | null, update: AgentThreadUpdate): AgentThreadSnapshot | null {
  if (!isRecord(update) || (update.kind !== "snapshot" && update.kind !== "delta")) return null;
  if (update.kind === "snapshot") return isThreadSnapshot(update.snapshot) ? update.snapshot : null;
  if (!isThreadSnapshot(update.metadata)) return null;
  if (!current || current.id !== update.id || !isRevision(update.base_revision) || !isRevision(update.revision)
    || current.revision !== update.base_revision || update.revision <= update.base_revision
    || !Array.isArray(update.events) || update.events.length !== update.revision - update.base_revision
    || update.metadata.id !== update.id || update.metadata.revision !== update.revision) return null;

  let items = [...current.items];
  let activeTurnId = current.activeTurn?.id ?? null;
  let activeTurnStatus: TurnStatus | null = current.activeTurn?.status ?? null;
  let pendingPermissionId = current.pendingPermission?.requestId ?? null;
  const replace = (id: string, item: AgentThreadItem) => {
    const index = items.findIndex((value) => value.id === id);
    if (index < 0) items.push(item); else items[index] = item;
  };

  for (const rawEvent of update.events) {
    if (!validEventEnvelope(rawEvent, current.id, activeTurnId)) return null;
    const event = rawEvent as unknown as Record<string, unknown>;
      // N1-B：permission_resolved 在非 running 态放行——迟到/超时的 resolved 事件
      // 只用于清权限条（request_id 仍须与 pending 匹配），不允许复活终态回合；
      // 其余事件仍要求 running。
      if (activeTurnStatus !== "running" && ["agent_delta", "tool_call_input_delta", "tool_call_output",
        "permission_requested", "runtime_updated"].includes(String(event.event))) return null;
    switch (event.event) {
      case "turn_started": {
        activeTurnId = event.turn_id as string;
        activeTurnStatus = "running";
        pendingPermissionId = null;
        items = [];
        break;
      }
      case "item_added": {
        if (!isThreadItem(event.item)) return null;
        const item = event.item;
        const previous = items.at(-1);
        if (item.kind === "timeline_event" && isRecord(item.data) && item.data.type === "reasoning_delta"
          && previous?.kind === "timeline_event" && previous.id !== item.id && isRecord(previous.data)
          && previous.data.type === "reasoning_delta" && previous.data.origin === item.data.origin && typeof previous.data.text === "string"
          && typeof item.data.text === "string") {
          items[items.length - 1] = { ...previous, data: { ...previous.data, text: previous.data.text + item.data.text } as AgentEventPayload };
        } else {
          replace(item.id, item);
        }
        break;
      }
      case "agent_delta": {
        if (typeof event.item_id !== "string" || typeof event.text !== "string" || event.text.length === 0) return null;
        const previous = items.at(-1);
        if (previous?.kind === "agent_message") {
          items[items.length - 1] = { ...previous, content: previous.content + event.text, pending: true };
        } else {
          items.push({ kind: "agent_message", id: `${event.item_id}:${items.length}`, content: event.text, pending: true });
        }
        break;
      }
      case "agent_final": {
        if (typeof event.item_id !== "string" || typeof event.content !== "string"
          || (event.preserve_streamed !== undefined && typeof event.preserve_streamed !== "boolean")) return null;
        const index = items.findIndex((item) => item.kind === "agent_message" && item.id === event.item_id);
        const hasBoundary = index >= 0 && items.slice(index + 1)
          .some((item) => item.kind === "tool_call" || item.kind === "user_message");
        const aggregate = items.filter((item) => item.kind === "agent_message").map((item) => item.content).join("");
        if (index >= 0 && aggregate.length > 0 && (event.preserve_streamed === true || aggregate === event.content)) {
          items = items.map((item) => item.kind === "agent_message" ? { ...item, pending: false } : item);
        } else if (!hasBoundary) {
          const item = index >= 0 ? items[index] : undefined;
          if (item?.kind === "agent_message") {
            items[index] = { ...item, content: event.content, pending: false };
            items = items.filter((candidate, position) => position <= index || candidate.kind !== "agent_message");
          }
        } else {
          const tail = items.at(-1);
          if (tail?.kind === "agent_message") {
            items[items.length - 1] = { ...tail, content: event.content, pending: false };
          } else {
            items.push({ kind: "agent_message", id: `${event.item_id}:final`, content: event.content, pending: false });
          }
        }
        break;
      }
      case "tool_call_input_delta": {
        if (typeof event.call_id !== "string" || typeof event.delta !== "string" || event.delta.length === 0) return null;
        const index = items.findIndex((item) => item.id === event.call_id && item.kind === "tool_call");
        const item = items[index];
        if (item?.kind !== "tool_call" || (hasOwn(item, "output") && item.output !== undefined)) return null;
        const inputBuffer = `${item.input_buffer || ""}${event.delta}`;
        // The authoritative tool_call item replaces this buffer when input is
        // complete. Parsing every growing prefix makes long arguments O(n^2).
        items[index] = { ...item, input_buffer: inputBuffer };
        break;
      }
      case "tool_call_output": {
        if (typeof event.call_id !== "string" || !hasOwn(event, "output")) return null;
        const index = items.findIndex((item) => item.id === event.call_id && item.kind === "tool_call");
        const item = items[index];
        if (item?.kind !== "tool_call" || (hasOwn(item, "output") && item.output !== undefined)) return null;
        items[index] = { ...item, live_output: mergeLiveToolOutput(item.live_output, event.output) as AgentEventPayload };
        break;
      }
      case "turn_completed": {
        if (!isTurnStatus(event.status) || activeTurnStatus !== "running") return null;
        activeTurnStatus = event.status;
        pendingPermissionId = null;
        items = items.map((item) => item.kind === "agent_message" ? { ...item, pending: false } : item);
        break;
      }
      case "permission_requested":
        if (typeof event.request_id !== "string" || typeof event.tool_name !== "string") return null;
        pendingPermissionId = event.request_id;
        break;
      case "permission_resolved":
        if (typeof event.request_id !== "string" || event.request_id !== pendingPermissionId) return null;
        pendingPermissionId = null;
        break;
      case "runtime_updated":
        if (typeof event.activity !== "string" || !Number.isSafeInteger(event.tool_call_count) || Number(event.tool_call_count) < 0) return null;
        break;
      case "usage_updated":
        if (!isRecord(event.usage)) return null;
        break;
      case "execution_target_updated":
        break;
      default:
        return null;
    }
  }
  return { ...update.metadata, items };
}

export interface ThreadUpdateReconcileResult {
  snapshot: AgentThreadSnapshot | null;
  changed: boolean;
  needsRefresh: boolean;
}

type BufferedDelta = { update: AgentThreadDeltaUpdate; bytes: number };

function bufferedDeltaBytes(update: AgentThreadDeltaUpdate) {
  try {
    const serialized = JSON.stringify(update);
    return typeof serialized === "string" ? encoder.encode(serialized).length : Number.POSITIVE_INFINITY;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

/** Buffers out-of-order deltas until an authoritative baseline closes the revision gap. */
export class ThreadUpdateReconciler {
  private readonly threadId: string;
  private current: AgentThreadSnapshot | null = null;
  private pending: BufferedDelta[] = [];
  private pendingBytes = 0;
  private authoritativeRefreshRequired = false;

  constructor(threadId: string) {
    this.threadId = threadId;
  }

  get snapshot() { return this.current; }
  get needsRefresh() { return this.authoritativeRefreshRequired || this.pending.length > 0; }
  get refreshAfterRevision() { return this.authoritativeRefreshRequired ? undefined : this.current?.revision; }

  acceptSnapshot(incoming: AgentThreadSnapshot): ThreadUpdateReconcileResult {
    const candidate: unknown = incoming;
    if (!isThreadSnapshot(candidate)) {
      if (isRecord(candidate) && candidate.id === this.threadId) this.requireAuthoritativeRefresh();
      return this.result(false);
    }
    let changed = false;
    if (candidate.id === this.threadId && (!this.current || candidate.revision > this.current.revision
      || (this.authoritativeRefreshRequired && candidate.revision === this.current.revision))) {
      this.current = candidate;
      this.authoritativeRefreshRequired = false;
      changed = true;
    }
    return this.drain(changed);
  }

  acceptUpdate(update: AgentThreadUpdate): ThreadUpdateReconcileResult {
    if (!isRecord(update) || (update.kind !== "snapshot" && update.kind !== "delta")) return this.result(false);
    if (update.kind === "snapshot") return this.acceptSnapshot(update.snapshot);
    if (update.id !== this.threadId) return this.result(false);
    if (!isThreadSnapshot(update.metadata) || update.metadata.id !== this.threadId
      || !isRevision(update.base_revision) || !isRevision(update.revision)
      || update.revision <= update.base_revision || !Array.isArray(update.events)
      || update.events.length !== update.revision - update.base_revision) {
      this.requireAuthoritativeRefresh();
      return this.result(false);
    }
    if (this.current && update.revision <= this.current.revision) return this.drain(false);

    // The common in-order path never serializes a potentially large delta.
    if (this.current && update.base_revision === this.current.revision) {
      const next = applyThreadUpdate(this.current, update);
      if (!next) {
        this.requireAuthoritativeRefresh();
        return this.result(false);
      }
      this.current = next;
      return this.drain(true);
    }
    if (this.current && update.base_revision < this.current.revision) {
      this.requireAuthoritativeRefresh();
      return this.result(false);
    }
    if (this.authoritativeRefreshRequired) return this.result(false);

    const bytes = bufferedDeltaBytes(update);
    const duplicate = this.pending.findIndex((candidate) => candidate.update.base_revision === update.base_revision
      && candidate.update.revision === update.revision);
    const replacedBytes = duplicate >= 0 ? this.pending[duplicate].bytes : 0;
    const nextBytes = this.pendingBytes - replacedBytes + bytes;
    const nextCount = this.pending.length + (duplicate >= 0 ? 0 : 1);
    if (!Number.isFinite(bytes) || nextBytes > MAX_BUFFERED_DELTA_BYTES || nextCount > MAX_BUFFERED_DELTAS) {
      this.requireAuthoritativeRefresh();
      return this.result(false);
    }
    const entry = { update, bytes };
    if (duplicate >= 0) this.pending[duplicate] = entry;
    else this.pending.push(entry);
    this.pendingBytes = nextBytes;
    this.pending.sort((left, right) => left.update.base_revision - right.update.base_revision
      || left.update.revision - right.update.revision);
    return this.drain(false);
  }

  private drain(initiallyChanged: boolean): ThreadUpdateReconcileResult {
    let changed = initiallyChanged;
    while (this.current) {
      this.removeStalePending();
      const index = this.pending.findIndex((entry) => entry.update.base_revision === this.current!.revision);
      if (index < 0) break;
      const entry = this.pending[index];
      const next = applyThreadUpdate(this.current, entry.update);
      if (!next) {
        this.requireAuthoritativeRefresh();
        break;
      }
      this.pending.splice(index, 1);
      this.pendingBytes -= entry.bytes;
      this.current = next;
      changed = true;
    }
    return this.result(changed);
  }

  private removeStalePending() {
    if (!this.current) return;
    const revision = this.current.revision;
    const retained = this.pending.filter((entry) => entry.update.revision > revision);
    if (retained.length === this.pending.length) return;
    this.pending = retained;
    this.pendingBytes = retained.reduce((total, entry) => total + entry.bytes, 0);
  }

  private requireAuthoritativeRefresh() {
    this.pending = [];
    this.pendingBytes = 0;
    this.authoritativeRefreshRequired = true;
  }

  private result(changed: boolean): ThreadUpdateReconcileResult {
    return { snapshot: this.current, changed, needsRefresh: this.needsRefresh };
  }
}
