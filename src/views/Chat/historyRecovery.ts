import type { AgentArtifact, AgentEventPayload, AgentImageAttachment, HistorySummary } from "@/platform/chatApi";
import type { ChatMessage, ChatSession } from "./types";
import { projectThreadUserAttachments } from "./chatAttachments.ts";

export type TurnTerminalStatus = "completed" | "cancelled" | "failed";

export type TurnTerminal = {
  status: TurnTerminalStatus;
  timestampMs?: number;
  message?: string;
  responseText?: string;
  responseTextIsDisplay?: boolean;
};

export type TurnDisplaySegment = {
  id: string;
  role: "user" | "assistant";
  content: string;
  attachments?: AgentImageAttachment[];
  artifacts?: AgentArtifact[];
  processStart?: number;
  processEnd?: number;
};

export const RECOVERY_INTERRUPTED_MESSAGE = "任务在应用关闭或重启时中断。";

export function mergeHistorySummary(summary: HistorySummary, existing?: ChatSession): ChatSession {
  // Summary and full-record reads race on startup. A summary owns navigation
  // fields only; it must not erase usage or messages already hydrated.
  return {
    ...existing,
    id: summary.id,
    title: summary.title,
    updatedAt: summary.updatedAt,
    archived: summary.archived,
    scope: summary.metadata?.scope === "workspace" ? "workspace" : "standalone",
    workspacePath: typeof summary.metadata?.workspacePath === "string" ? summary.metadata.workspacePath : undefined,
    messages: existing?.messages || [],
    historyMessageCount: summary.messageCount,
  };
}

const INTERNAL_CONTEXT_ENVELOPE = /^\s*\[(?:历史助手陈述(?:已隔离)?，非工具证据|历史成功工具证据|历史摘要)[^\]]*\][ \t]*(?:\r?\n)?/;

export function sanitizeAssistantDisplayText(text: string) {
  // Providers can echo context provenance headers into a new response. The
  // header is internal metadata, not a reason to discard the generated body.
  while (INTERNAL_CONTEXT_ENVELOPE.test(text)) text = text.replace(INTERNAL_CONTEXT_ENVELOPE, "");
  return text;
}

export function mergeAssistantText(streamed: string, finalText: string) {
  const safeStreamed = sanitizeAssistantDisplayText(streamed);
  const safeFinal = sanitizeAssistantDisplayText(finalText);
  const streamedText = safeStreamed.trim();
  const returnedText = safeFinal.trim();
  if (!streamedText) return safeFinal;
  if (!returnedText || streamedText === returnedText) return safeStreamed;
  if (returnedText.startsWith(streamedText)) return safeFinal;
  if (streamedText.startsWith(returnedText)) return safeStreamed;
  return safeFinal;
}

export function eventTurnId(event: AgentEventPayload) {
  const turnId = event.turnId || event.turn_id;
  return typeof turnId === "string" && turnId ? turnId : "";
}

function eventTimestampMs(event: AgentEventPayload) {
  const value = event.timestampMs ?? event.timestamp_ms;
  const timestamp = typeof value === "number" ? value : typeof value === "string" ? Number(value) : 0;
  return Number.isFinite(timestamp) && timestamp > 0 ? timestamp : undefined;
}

export function terminalForEvent(event: AgentEventPayload): TurnTerminal | null {
  const type = String(event.type || "").toLowerCase();
  const timestampMs = eventTimestampMs(event);
  const displayText = typeof event.display_text === "string"
    ? event.display_text
    : typeof event.displayText === "string" ? event.displayText : undefined;
  if (type === "completed") return { status: "completed", timestampMs,
    responseText: displayText ?? (typeof event.text === "string" ? event.text : undefined),
    responseTextIsDisplay: displayText !== undefined,
  };
  if (type === "cancelled") return { status: "cancelled", timestampMs,
    responseText: displayText, responseTextIsDisplay: displayText !== undefined,
  };
  if (type === "error") return { status: "failed", timestampMs,
    message: typeof event.message === "string" ? event.message : undefined,
    responseText: displayText, responseTextIsDisplay: displayText !== undefined,
  };
  return null;
}

export function isNativeTerminalEvent(event: AgentEventPayload) {
  return Boolean(terminalForEvent(event))
    && typeof event.sequence === "number"
    && typeof (event.eventId || event.event_id) === "string";
}

export function isDurableTerminalEvent(event: AgentEventPayload) {
  if (!terminalForEvent(event)) return false;
  if (event.terminal_persistence_failed === true) return false;
  if (event.durable_terminal === true) return true;
  return isNativeTerminalEvent(event);
}

export function isRecoverySyntheticEvent(event: AgentEventPayload) {
  if (event.recoverySynthetic === true || event.recovery_synthetic === true) return true;
  if (event.type === "status" && event.message === "应用重启后，上次运行已中断。") return true;
  return event.type === "tool_call_finished"
    && (event.output === "应用关闭或重启时中断。" || event.output === "任务已结束，工具结果未持久化。")
    && (event.is_error === true || event.error === true);
}

export function terminalForMessage(message: ChatMessage): TurnTerminal | null {
  const messageTurnId = message.turnId || "";
  let fallbackTerminal: TurnTerminal | null = null;
  for (const event of [...(message.events || [])].reverse()) {
    const eventTurn = eventTurnId(event);
    if (messageTurnId && eventTurn && eventTurn !== messageTurnId) continue;
    const terminal = terminalForEvent(event);
    if (!terminal) continue;
    if (isDurableTerminalEvent(event)) return terminal;
    fallbackTerminal ||= terminal;
  }
  if (fallbackTerminal) return fallbackTerminal;
  if (message.events?.some(isRecoverySyntheticEvent)) return null;
  if (typeof message.finishedAt === "number" && message.finishedAt > 0) {
    return { status: message.error ? "failed" : "completed", timestampMs: message.finishedAt };
  }
  return null;
}

function completedResponseTextForMessage(message: ChatMessage) {
  const messageTurnId = message.turnId || "";
  for (const event of [...(message.events || [])].reverse()) {
    const eventTurn = eventTurnId(event);
    if (messageTurnId && eventTurn && eventTurn !== messageTurnId) continue;
    if (event.type === "completed") {
      const text = typeof event.display_text === "string" ? event.display_text : event.text;
      if (typeof text === "string" && text.trim()) return text;
    }
  }
  return undefined;
}

function optionalIndex(value: unknown) {
  const index = typeof value === "number" ? value : Number(value);
  return Number.isSafeInteger(index) && index >= 0 ? index : undefined;
}

function displaySegmentsForEvent(event: AgentEventPayload): TurnDisplaySegment[] {
  const raw = event.display_segments ?? event.displaySegments;
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item, index): TurnDisplaySegment[] => {
    if (!item || typeof item !== "object") return [];
    const value = item as Record<string, unknown>;
    const role = value.role === "user" || value.role === "assistant" ? value.role : null;
    const content = typeof value.content === "string" ? value.content : typeof value.text === "string" ? value.text : null;
    if (!role || content === null) return [];
    return [{
      id: typeof value.id === "string" && value.id ? value.id : `${role}-segment-${index}`,
      role,
      content,
      attachments: Array.isArray(value.attachments) ? value.attachments as AgentImageAttachment[] : undefined,
      artifacts: Array.isArray(value.artifacts) ? value.artifacts as AgentArtifact[] : undefined,
      processStart: optionalIndex(value.process_start ?? value.processStart),
      processEnd: optionalIndex(value.process_end ?? value.processEnd),
    }];
  });
}

export function displaySegmentsForMessage(message: ChatMessage) {
  const turnId = message.turnId || "";
  for (const event of [...(message.events || [])].reverse()) {
    const eventTurn = eventTurnId(event);
    if (turnId && eventTurn && eventTurn !== turnId) continue;
    const segments = displaySegmentsForEvent(event);
    if (segments.length) return segments;
  }
  return [];
}

export function expandTurnDisplaySegments(inputMessages: ChatMessage[]) {
  const messages = inputMessages.map((message) => {
    if (message.role !== "assistant") return message;
    const terminal = terminalForMessage(message);
    if (terminal?.status !== "failed" || !terminal.message
      || message.events?.some((event) => event.terminalDiagnostic === true && event.status === "failed")) return message;
    return { ...message, events: [...(message.events || []), {
      type: "status", status: "failed", message: terminal.message, terminalDiagnostic: true,
      ...(message.turnId ? { turn_id: message.turnId } : {}),
    }] };
  });
  const definitions = new Map<string, { source: ChatMessage; segments: TurnDisplaySegment[] }>();
  for (const message of messages) {
    if (message.role !== "assistant" || !message.turnId || definitions.has(message.turnId)) continue;
    const segments = displaySegmentsForMessage(message);
    if (segments.length) definitions.set(message.turnId, { source: message, segments });
  }
  if (!definitions.size) return messages;

  const groups = new Map<string, ChatMessage[]>();
  for (const message of messages) {
    if (!message.turnId || !definitions.has(message.turnId)) continue;
    const group = groups.get(message.turnId) || [];
    group.push(message);
    groups.set(message.turnId, group);
  }
  const emitted = new Set<string>();
  const output: ChatMessage[] = [];
  for (const message of messages) {
    const turnId = message.turnId;
    const definition = turnId ? definitions.get(turnId) : undefined;
    if (!turnId || !definition) {
      output.push(message);
      continue;
    }
    if (emitted.has(turnId)) continue;
    emitted.add(turnId);
    const retained = groups.get(turnId) || [];
    const retainedById = new Map(retained.map((item) => [item.id, item]));
    const firstRetainedAssistant = retained.findIndex((item) => item.role === "assistant");
    const initialUser = retained
      .slice(0, firstRetainedAssistant < 0 ? retained.length : firstRetainedAssistant)
      .find((item) => item.role === "user");
    const segmentUserIds = new Set(definition.segments.filter((segment) => segment.role === "user").map((segment) => segment.id));
    const firstSegment = definition.segments[0];
    const assistantCount = definition.segments.filter((segment) => segment.role === "assistant").length;
    const terminalDiagnostics = definition.source.events?.filter((event) => event.terminalDiagnostic === true);
    const fileChange = retained.flatMap(item => item.role === "assistant" ? item.events || [] : [])
      .filter(event => event.type === "file_changes").at(-1);
    const ownerEvents = definition.source.events?.filter((event) => event.type !== "file_changes"
      && !["snapshot_text", "text_delta"].includes(event.type || ""));
    let assistantIndex = 0;
    let createdAt = Math.min(...retained.map((item) => item.createdAt), definition.source.startedAt ?? definition.source.createdAt);
    const usedIds = new Set<string>();
    // Legacy process history started at the first assistant item and omitted
    // the original prompt. Preserve only that leading prompt; later retained
    // user rows may be old hoisted steer messages and are represented by the
    // ordered terminal segments instead.
    const initialUserRepresented = initialUser && (segmentUserIds.has(initialUser.id)
      || (firstSegment?.role === "user" && firstSegment.content === initialUser.content));
    if (initialUser && !initialUserRepresented) {
      output.push(initialUser);
      usedIds.add(initialUser.id);
      createdAt = Math.max(createdAt, initialUser.createdAt) + 1;
    }
    for (const [index, segment] of definition.segments.entries()) {
      let id = segment.id;
      if (usedIds.has(id)) id = `${id}:segment-${index}`;
      usedIds.add(id);
      const initialUserFallback = index === 0 && segment.role === "user" && initialUser
        && (segment.id === initialUser.id || segment.content === initialUser.content)
        ? initialUser
        : undefined;
      const previous = retainedById.get(segment.id) || initialUserFallback;
      const segmentCreatedAt = Math.max(createdAt, previous?.createdAt ?? 0);
      createdAt = segmentCreatedAt + 1;
      if (segment.role === "user") {
        output.push({
          ...(previous || {}),
          id,
          role: "user",
          content: segment.content,
          createdAt: segmentCreatedAt,
          turnId,
          pending: false,
          attachments: projectThreadUserAttachments(segment, previous?.attachments),
          processStart: segment.processStart,
          processEnd: segment.processEnd,
        });
        continue;
      }
      const owner = assistantIndex === 0;
      output.push({
        id,
        role: "assistant",
        content: segment.content,
        createdAt: segmentCreatedAt,
        startedAt: owner ? definition.source.startedAt : undefined,
        finishedAt: owner ? definition.source.finishedAt : undefined,
        pending: false,
        error: definition.source.error,
        attachments: owner ? definition.source.attachments : undefined,
        events: owner
          ? [...(ownerEvents || []).filter(event => event.terminalDiagnostic !== true || assistantIndex === assistantCount - 1), ...(fileChange ? [fileChange] : [])]
          : assistantIndex === assistantCount - 1 && terminalDiagnostics?.length ? terminalDiagnostics : undefined,
        turnId,
        turnSegmentIndex: assistantIndex,
        turnSegmentCount: assistantCount,
        processStart: segment.processStart,
        processEnd: segment.processEnd,
      });
      assistantIndex += 1;
    }
  }
  return output;
}

export function hasRecoveryInterruptedPlaceholder(message: Pick<ChatMessage, "content" | "events">) {
  return message.content.trim() === RECOVERY_INTERRUPTED_MESSAGE
    && Boolean(message.events?.some(isRecoverySyntheticEvent));
}

function hasUnresolvedToolCalls(message: ChatMessage) {
  if (message.role !== "assistant" || !message.events?.length) return false;
  const started = new Map<string, AgentEventPayload>();
  const finished = new Set<string>();
  for (const event of message.events) {
    const callId = typeof event.callId === "string" ? event.callId : typeof event.call_id === "string" ? event.call_id : "";
    if (!callId) continue;
    if (event.type === "tool_call_started") started.set(callId, event);
    if (event.type === "tool_call_finished") finished.add(callId);
  }
  return [...started.keys()].some((callId) => !finished.has(callId));
}

export function restorePendingAssistantMessage(message: ChatMessage, timestampMs: number): "none" | "terminal" | "interrupted" {
  if (message.role !== "assistant") return "none";
  message.content = sanitizeAssistantDisplayText(message.content);
  const terminal = terminalForMessage(message);
  if (terminal) {
    const hadRecoveryPlaceholder = hasRecoveryInterruptedPlaceholder(message);
    message.events = message.events?.filter((event) => !isRecoverySyntheticEvent(event));
    if (hadRecoveryPlaceholder) message.content = "";
    const responseText = terminal.responseText
      || (terminal.status === "completed" ? completedResponseTextForMessage(message) : undefined);
    if (responseText?.trim()) message.content = terminal.responseTextIsDisplay
      ? sanitizeAssistantDisplayText(responseText)
      : mergeAssistantText(message.content, responseText);
    const unresolvedToolCalls = hasUnresolvedToolCalls(message);
    const wasPending = Boolean(message.pending);
    message.pending = false;
    message.finishedAt = message.finishedAt || terminal.timestampMs || timestampMs;
    if (terminal.status === "completed") message.error = false;
    if (!message.content.trim() && terminal.status === "cancelled") message.content = "已停止。";
    if (!message.content.trim() && terminal.status === "failed") {
      message.content = terminal.message || "任务执行失败。";
      message.error = true;
    }
    return wasPending || unresolvedToolCalls ? "terminal" : "none";
  }
  if (!message.pending) return "none";
  message.pending = false;
  message.finishedAt = message.finishedAt || timestampMs;
  message.content = message.content || RECOVERY_INTERRUPTED_MESSAGE;
  message.events = [...(message.events || []), {
    type: "status",
    message: "应用重启后，上次运行已中断。",
    recoverySynthetic: true,
    recovery_synthetic: true,
    timestampMs,
  }];
  return "interrupted";
}
