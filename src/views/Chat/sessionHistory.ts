import { type AgentEventPayload, type HistoryRecord } from "@/platform/chatApi";
import {
  isDurableTerminalEvent,
  isRecoverySyntheticEvent,
  expandTurnDisplaySegments,
  restorePendingAssistantMessage,
  sanitizeAssistantDisplayText,
} from "./historyRecovery";
import { restoreHistoryUsage } from "./chatUsage";
import { readTimelineExpanded } from "./timelineState";
import { readBoundEnvironments } from "./boundEnvironmentsState.ts";
import { canonicalToolName } from "./toolIdentity";
import type { ChatMessage, ChatSession, ChatSource, ExecutionTarget } from "./types";
import { readChatSessions, readDeletedChatSessionIds } from "./chatNavStore";
import { retainedComposerSources } from "./composerSubmission";
import { readProjectSelection } from "./projectSelection";
import { conversationHistoryReadAhead } from "./historyReadAhead";
import { retainContextUsageEvents } from "./contextUsageDetails";
import { restoreFileChangeSummaries } from "./fileChangePresentation";

export const CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION = 3;
const restoredCachedSessions = new WeakMap<ChatSession, ChatSession>();

export function restoreCachedSession(session: ChatSession): ChatSession {
  const cached = restoredCachedSessions.get(session);
  if (cached) return cached;
  const restored: ChatSession = { ...session, messages: session.messages.map(message => ({ ...message })) };
  if (restored.contextCheckpoint?.evidencePolicyVersion !== CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION) restored.contextCheckpoint = undefined;
  for (const message of restored.messages) restorePendingAssistantMessage(message, restored.updatedAt);
  restored.messages = expandTurnDisplaySegments(restoreFileChangeSummaries(restored.messages, session.id));
  restoredCachedSessions.set(session, restored);
  restoredCachedSessions.set(restored, restored);
  return restored;
}

export function loadSessions(): ChatSession[] {
  try {
    const sessions = readChatSessions();
    const preparedSessions = sessions.map(session => {
      const prepared = conversationHistoryReadAhead.peek(session.id);
      const restored = restoreCachedSession(session);
      return prepared ? upsertHistorySession([restored], historyRecordToSession(prepared))[0] : restored;
    });
    const known = new Set(sessions.map(session => session.id));
    const deleted = readDeletedChatSessionIds();
    return [...preparedSessions, ...conversationHistoryReadAhead.ready().filter(record => !known.has(record.id) && !deleted.has(record.id)).map(historyRecordToSession)];
  } catch {
    return [];
  }
}

export function historyRecordToSession(record: HistoryRecord): ChatSession {
  const metadata = record.metadata || {};
  // Current History stores an array, while records created by earlier builds
  // may have been exported/imported as a JSON string or wrapped object.
  let rawMessages: unknown = record.messages;
  if (typeof rawMessages === "string") {
    try { rawMessages = JSON.parse(rawMessages); } catch { rawMessages = []; }
  }
  const stored = Array.isArray(rawMessages)
    ? rawMessages
    : rawMessages && typeof rawMessages === "object" && Array.isArray((rawMessages as { messages?: unknown }).messages)
      ? (rawMessages as { messages: unknown[] }).messages
      : [];
  const restoredMessages = stored.flatMap((item, index): ChatMessage[] => {
    if (!item || typeof item !== "object") return [];
    const value = item as Partial<ChatMessage> & { text?: unknown; timestamp?: unknown };
    const rawContent = typeof value.content === "string" ? value.content : typeof value.text === "string" ? value.text : "";
    const content = value.role === "assistant" ? sanitizeAssistantDisplayText(rawContent) : rawContent;
    if (value.role !== "user" && value.role !== "assistant") return [];
    const message: ChatMessage = {
      ...value,
      id: typeof value.id === "string" && value.id ? value.id : `${record.id}-message-${index}`,
      role: value.role,
      content,
      createdAt: typeof value.createdAt === "number" ? value.createdAt : typeof value.timestamp === "number" ? value.timestamp : record.updatedAt,
    };
    if (message.role === "assistant") {
      // A persisted pending flag describes the previous WebView, not a live
      // worker. Current execution is projected from the native thread snapshot.
      message.pending = false;
      const run = record.agentRuns?.find((run) => run.turnId === message.turnId && run.status === "interrupted");
      if (run) {
        message.pending = false;
        const lastEventAt = Math.max(run.startedAt, ...(message.events || []).map((event) => Number(event.timestampMs ?? event.timestamp_ms) || 0));
        message.finishedAt ||= run.finishedAt || lastEventAt;
      }
    }
    return [message];
  });
  const messages = expandTurnDisplaySegments(restoreFileChangeSummaries(restoredMessages, record.id));
  const persistedEvents = Array.isArray(record.events) ? record.events.filter((event): event is AgentEventPayload => Boolean(event && typeof event === "object")) : [];
  const restoredSources = Array.isArray(metadata.sources)
    ? metadata.sources.filter((item): item is ChatSource => Boolean(item && typeof item === "object" && typeof (item as ChatSource).path === "string"))
    : [];
  const storedTarget = metadata.executionTarget as Partial<ExecutionTarget> | undefined;
  const storedBoundEnvironments = readBoundEnvironments(metadata.boundEnvironments);
  const storedTimelineExpanded = readTimelineExpanded(metadata.timelineExpanded);
  const completedCheckpoint = [...persistedEvents].reverse().find((event) => event.type === "completed" && isDurableTerminalEvent(event)
    && event.contextCheckpoint && typeof event.contextCheckpoint === "object")?.contextCheckpoint as ChatSession["contextCheckpoint"] | undefined;
  const checkpoint = completedCheckpoint || (metadata.contextCheckpoint && typeof metadata.contextCheckpoint === "object"
    ? metadata.contextCheckpoint as ChatSession["contextCheckpoint"]
    : undefined);
  const restoredUsage = restoreHistoryUsage(record, persistedEvents, messages);
  const storedModelSelection = metadata.modelSelection as Partial<ChatSession["modelSelection"]> | null | undefined;
  const modelSelection = storedModelSelection
    && typeof storedModelSelection.providerId === "string" && storedModelSelection.providerId
    && typeof storedModelSelection.modelId === "string" && storedModelSelection.modelId
    ? { providerId: storedModelSelection.providerId, modelId: storedModelSelection.modelId }
    : undefined;
  return {
    id: record.id,
    title: record.title,
    updatedAt: record.updatedAt,
    archived: record.archived,
    createdAt: typeof metadata.createdAt === "number" ? metadata.createdAt : undefined,
    pinned: metadata.pinned === true,
    scope: metadata.scope === "workspace" ? "workspace" : "standalone",
    workspacePath: typeof metadata.workspacePath === "string" ? metadata.workspacePath : undefined,
    // `_dshStorageId` 是改名前的键，老会话里存的是它；读新键失败时回退到旧键，
    // 免得升级后历史会话的存储绑定丢掉。
    dshStorageId:
      typeof metadata._storageBindingId === "string"
        ? metadata._storageBindingId
        : typeof metadata._dshStorageId === "string"
          ? metadata._dshStorageId
          : undefined,
    messages,
    contextUsageEvents: [
      ...persistedEvents.filter((event) => event.type === "context"),
      ...(Array.isArray(metadata.contextUsageEvents) ? metadata.contextUsageEvents.filter((event): event is AgentEventPayload => Boolean(event && typeof event === "object" && event.type === "context")) : []),
    ],
    contextCheckpoint: checkpoint?.evidencePolicyVersion === CONTEXT_CHECKPOINT_EVIDENCE_POLICY_VERSION
      ? checkpoint
      : undefined,
    sources: restoredSources,
    automationProjectIds: readProjectSelection(metadata.automationProjectIds),
    artifacts: Array.isArray(record.artifacts) ? record.artifacts.filter((item): item is Record<string, unknown> => Boolean(item && typeof item === "object")) : [],
    ...restoredUsage,
    usageTotals: record.usageTotals,
    executionTarget: storedTarget && (storedTarget.kind === "local" || storedTarget.kind === "ssh" || storedTarget.kind === "wsl")
      ? storedTarget as ExecutionTarget : undefined,
    boundEnvironments: storedBoundEnvironments,
    timelineExpanded: storedTimelineExpanded,
    dismissedTaskPlanKey: typeof metadata.dismissedTaskPlanKey === "string" ? metadata.dismissedTaskPlanKey : undefined,
    modelSelection,
    agentRuns: Array.isArray(record.agentRuns) ? record.agentRuns : [],
  };
}

export function upsertHistorySession(previous: ChatSession[], restored: ChatSession): ChatSession[] {
  const existing = previous.find((item) => item.id === restored.id);
  // Message content, events, terminal state, and run state are hydrated by
  // Rust. Keep UI attachment/selection state and a bounded context-telemetry
  // fallback for older records whose native event tail omitted the readings.
  const hydrated = existing ? {
    ...restored,
    contextUsageEvents: retainContextUsageEvents([
      ...existing.messages.flatMap((message) => (message.events || []).filter((event) => event.type === "context")),
      ...(existing.contextUsageEvents || []),
      ...(restored.contextUsageEvents || []),
    ]),
    sources: retainedComposerSources(existing.sources, restored.sources || []),
    automationProjectIds: existing.automationProjectIds ?? restored.automationProjectIds,
    artifacts: existing.artifacts?.length ? existing.artifacts : restored.artifacts,
    executionTarget: existing.executionTarget || restored.executionTarget,
    boundEnvironments: existing.boundEnvironments?.length ? existing.boundEnvironments : restored.boundEnvironments,
    timelineExpanded: existing.timelineExpanded || restored.timelineExpanded,
    dismissedTaskPlanKey: existing.dismissedTaskPlanKey ?? restored.dismissedTaskPlanKey,
    modelSelection: existing.modelSelection ?? restored.modelSelection,
  } : restored;
  const next = existing
    ? previous.map((item) => item.id === restored.id ? hydrated : item)
    : [hydrated, ...previous];
  return next.sort((left, right) => right.updatedAt - left.updatedAt);
}

export const TIMELINE_EVENT_TYPES = new Set([
  "tool_call_pending",
  "tool_call_started",
  "tool_call_finished",
  "command_output",
  "permission_requested",
  "permission_decision",
  "context",
  "file_changes",
]);

export function hasVisibleMessageContent(message: ChatMessage) {
  return Boolean(
    message.content.trim()
    || message.attachments?.length
    || message.events?.some((event) => TIMELINE_EVENT_TYPES.has(event.type || "")),
  );
}

export function historyEventKey(event: AgentEventPayload) {
  const id = event.eventId || event.event_id;
  if (typeof id === "string" && id) return `id:${id}`;
  const sequence = event.sequence;
  const turnId = event.turnId || event.turn_id || "";
  if (typeof sequence === "number") return `sequence:${turnId}:${sequence}`;
  return `${event.type || "event"}:${event.callId || event.call_id || ""}:${event.timestampMs || ""}:${String(event.message || event.text || "")}`;
}

export function collectSessionEvents(session: ChatSession) {
  const seen = new Set<string>();
  return session.messages.flatMap((message) => message.events || []).filter((event) => {
    if (isRecoverySyntheticEvent(event)) return false;
    const key = historyEventKey(event);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function contextExcerpt(value: unknown, limit: number) {
  const text = typeof value === "string" ? value : JSON.stringify(value ?? "");
  const normalized = text.replace(/\s+/g, " ").trim();
  if (normalized.length <= limit) return normalized;
  const head = Math.max(160, Math.floor(limit * 0.65));
  return `${normalized.slice(0, head)} [中间内容已省略] ${normalized.slice(-(limit - head - 12))}`;
}

export function asContextRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

export function contextString(...values: unknown[]) {
  return values.find((value): value is string => typeof value === "string" && value.trim().length > 0)?.trim();
}

export function contextFlag(value: unknown) {
  return value === true || value === 1 || value === "true";
}

export function contextError(value: unknown) {
  return value !== undefined && value !== null && value !== false && value !== "";
}

export function isHistoricalToolSuccess(event: AgentEventPayload) {
  if (event.type !== "tool_call_finished" || event.output === undefined
    || contextFlag(event.resultMissing) || contextFlag(event.result_missing)
    || isRecoverySyntheticEvent(event)) return false;
  const output = asContextRecord(event.output);
  return !contextFlag(event.is_error)
    && !contextError(event.error)
    && !contextError(output?.error)
    && !contextFlag(event.skipped)
    && !contextFlag(output?.skipped)
    && !contextFlag(event.steered)
    && !contextFlag(output?.steered);
}

export function historicalToolPath(event: AgentEventPayload) {
  const input = asContextRecord(event.input);
  const output = asContextRecord(event.output);
  return contextString(
    output?.path,
    output?.filePath,
    output?.file_path,
    input?.path,
    input?.filePath,
    input?.file_path,
    event.path,
  );
}

export function isHistoricalFileRead(event: AgentEventPayload) {
  return event.type === "tool_call_finished"
    && canonicalToolName(contextString(event.toolName, event.tool_name) || "") === "read_local_file";
}

export function historicalFileReadEvidenceKeys(event: AgentEventPayload) {
  if (!isHistoricalFileRead(event)) return [];
  const keys = new Set<string>();
  const nativeKey = contextString(event.evidencePathKey, event.evidence_path_key);
  if (nativeKey) keys.add(`native:${nativeKey}`);

  // Raw aliases bridge events saved before the native key was introduced.
  // Match them exactly; frontend code must not guess how a local, SSH, or WSL
  // path resolves.
  const input = asContextRecord(event.input);
  const output = asContextRecord(event.output);
  for (const path of [
    output?.path,
    output?.filePath,
    output?.file_path,
    input?.path,
    input?.filePath,
    input?.file_path,
    event.path,
  ]) {
    if (typeof path === "string" && path.trim()) keys.add(`legacy:${path.trim()}`);
  }
  return [...keys];
}

export function activeHistoricalFileReadSuccesses(messages: ChatMessage[]) {
  const latestSuccessByKey = new Map<string, AgentEventPayload>();
  const successfulReadKeys = new Map<AgentEventPayload, string[]>();

  for (const message of messages) {
    for (const event of message.events || []) {
      if (!isHistoricalFileRead(event)) continue;
      // Restart recovery only closes an incomplete visual item. It is not a
      // file-system failure and must not invalidate independently persisted
      // read evidence from the same conversation.
      if (isRecoverySyntheticEvent(event)) continue;
      const keys = historicalFileReadEvidenceKeys(event);
      if (!isHistoricalToolSuccess(event)) {
        if (keys.length) {
          for (const key of keys) latestSuccessByKey.delete(key);
        } else {
          // A legacy failure without any path cannot be safely associated with
          // one file, so invalidate earlier read evidence until a later read
          // produces a usable key.
          latestSuccessByKey.clear();
        }
        continue;
      }
      successfulReadKeys.set(event, keys);
      for (const key of keys) latestSuccessByKey.set(key, event);
    }
  }

  const active = new Set<AgentEventPayload>();
  for (const [event, keys] of successfulReadKeys) {
    if (keys.length && keys.every((key) => latestSuccessByKey.get(key) === event)) {
      active.add(event);
    }
  }
  return active;
}

export function historicalToolReference(event: AgentEventPayload) {
  const turnId = contextString(event.turnId, event.turn_id) || "未知";
  const callId = contextString(event.callId, event.call_id) || "未知";
  const toolName = contextString(event.toolName, event.tool_name) || "工具";
  const path = historicalToolPath(event) || "未提供";
  return `turnId: ${turnId}\ncallId: ${callId}\ntoolName: ${toolName}\npath: ${path}`;
}

export function historicalToolFailureReason(event: AgentEventPayload) {
  const output = asContextRecord(event.output);
  const outputError = output?.error;
  if (contextFlag(event.steered) || contextFlag(output?.steered)) return "已因追加请求跳过";
  if (contextFlag(event.skipped) || contextFlag(output?.skipped)) {
    return contextString(output?.reason, event.reason, output?.message, event.message) || "调用被跳过";
  }
  const nestedError = asContextRecord(output?.error);
  const code = contextString(nestedError?.code, output?.code, event.code);
  const message = contextString(nestedError?.message, output?.message, typeof outputError === "string" ? outputError : undefined, typeof event.error === "string" ? event.error : undefined, event.message);
  if (code || message) return [code ? `code=${code}` : "", message ? contextExcerpt(message, 360) : ""].filter(Boolean).join("; ");
  return "未返回可验证的成功结果";
}

export function hasFailedHistoricalFileRead(events: AgentEventPayload[]) {
  return events.some((event) => String(event.type || "") === "tool_call_finished"
    && canonicalToolName(contextString(event.toolName, event.tool_name) || "") === "read_local_file"
    && !isRecoverySyntheticEvent(event)
    && !isHistoricalToolSuccess(event));
}

export function sessionToHistoryInput(session: ChatSession) {
  return {
    id: session.id,
    title: session.title,
    updatedAt: session.updatedAt,
    archived: session.archived,
    schemaVersion: 3,
    artifacts: session.artifacts || [],
    messages: session.messages.map(({ events: _events, ...message }) => message),
    metadata: {
      _storageBindingId: session.dshStorageId,
      scope: session.scope || "standalone",
      workspacePath: session.workspacePath || null,
      createdAt: session.createdAt ?? session.updatedAt ?? null,
      pinned: session.pinned === true,
      contextCheckpoint: session.contextCheckpoint || null,
      sources: session.sources || [],
      automationProjectIds: session.automationProjectIds,
      executionTarget: session.executionTarget || { kind: "local", label: "本机工作区" },
      boundEnvironments: session.boundEnvironments || [],
      timelineExpanded: session.timelineExpanded || {},
      dismissedTaskPlanKey: session.dismissedTaskPlanKey,
      usageTurnId: session.usageTurnId || null,
      modelSelection: session.modelSelection || null,
    },
    usage: session.usage || null,
    events: collectSessionEvents(session),
  };
}
