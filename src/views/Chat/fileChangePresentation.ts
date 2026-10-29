import type { AgentEventPayload, AgentThreadSnapshot } from "@/platform/chatApi";
import type { ChatMessage } from "./types";
import { snapshotIsSending } from "./threadSnapshotRuntime.ts";
import { canonicalToolName } from "./toolIdentity.ts";

function object(value: unknown): Record<string, unknown> | null {
  if (typeof value === "string") { try { return object(JSON.parse(value)); } catch { return null; } }
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

// Older builds scanned the tool cache instead of source files. A successful
// explicit write still proves which file changed, but cannot recreate a backup
// or exact line totals. Never infer edits from shell text or assistant prose.
const recoveredWrites = new WeakMap<ChatMessage, string[]>();
function writtenPaths(message: ChatMessage) {
  const cached = recoveredWrites.get(message);
  if (cached) return cached;
  const paths = new Set<string>();
  for (const event of message.events || []) {
    if (event.type !== "tool_call_finished" || event.is_error || event.isError || event.error) continue;
    const tool = canonicalToolName(String(event.toolName || event.tool_name || ""));
    if (!["apply_patch", "write_local_file"].includes(tool)) continue;
    const output = object(event.output);
    if (!output || output.error || output.skipped || output.interrupted) continue;
    if (tool === "apply_patch" && !(Number(output.appliedHunks) > 0)) continue;
    const path = output.path;
    if (typeof path === "string" && path.trim()) paths.add(path);
  }
  const result = [...paths];
  recoveredWrites.set(message, result);
  return result;
}

function hasFiles(event?: AgentEventPayload) {
  return [event?.entries, event?.created, event?.modified, event?.deleted].some(value => Array.isArray(value) && value.length);
}

function lastFileChange(message: ChatMessage) {
  return [...(message.events || [])].reverse().find((event) => event.type === "file_changes");
}

// One card belongs to the last assistant segment of each turn, even when its
// change event was emitted before a steer split that turn into more segments.
export function messageFileChanges(messages: ChatMessage[], sessionId = "") {
  const turns = new Map<string, { messageId: string; event?: AgentEventPayload; paths: Set<string> }>();
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    const key = message.turnId || message.id;
    const previous = turns.get(key);
    const paths = previous?.paths || new Set<string>();
    writtenPaths(message).forEach(path => paths.add(path));
    turns.set(key, { messageId: message.id, event: lastFileChange(message) || previous?.event, paths });
  }
  return new Map([...turns].map(([turnId, turn]) => {
    const event = turn.event;
    return [turn.messageId, hasFiles(event) || event?.snapshot_complete === true || !turn.paths.size ? event : {
      type: "file_changes", session_id: event?.session_id || event?.sessionId || sessionId, turn_id: turnId,
      modified: [...turn.paths], review_available: false, line_counts_available: false,
    } satisfies AgentEventPayload];
  }));
}

export function restoreFileChangeSummaries(messages: ChatMessage[], sessionId: string) {
  const changes = messageFileChanges(messages, sessionId);
  return messages.map(message => {
    const event = changes.get(message.id);
    if (!event || (message.events || []).includes(event)) return message;
    return { ...message, events: [...(message.events || []).filter(event => event.type !== "file_changes"), event] };
  });
}

// A completed snapshot can linger while the next optimistic turn is pending.
// Its changes already have a reply card and must never return to the composer.
export function pendingFileChanges(snapshot: AgentThreadSnapshot | null, messages: ChatMessage[]) {
  const pending = [...messages].reverse().find((message) => message.role === "assistant" && message.pending);
  const turnId = snapshotIsSending(snapshot) ? snapshot?.activeTurn?.id : pending?.turnId;
  if (!turnId) return undefined;
  if (snapshotIsSending(snapshot) && snapshot?.activeTurn?.id === turnId) {
    const item = snapshot.items.find((item) => item.kind === "file_changes");
    if (item?.kind === "file_changes") return item.data;
  }
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "assistant" || message.turnId !== turnId) continue;
    const event = lastFileChange(message);
    if (event) return event;
  }
  return undefined;
}
