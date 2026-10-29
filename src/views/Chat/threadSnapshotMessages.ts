import type { AgentEventPayload, AgentThreadItem, AgentThreadSnapshot } from "@/platform/chatApi";
import type { ChatMessage } from "./types";
import { liveToolOutputText } from "./threadSnapshotTools.ts";
import { projectThreadUserAttachments } from "./chatAttachments.ts";
import { isAssistantProcessBoundary } from "./assistantText.ts";

export function activeThreadTurnId(snapshot: AgentThreadSnapshot | null, messages: ChatMessage[]) {
  const pendingTurnId = [...messages].reverse().find((message) => message.role === "assistant" && message.pending)?.turnId;
  if (snapshot?.activeTurn?.status === "running" || snapshot?.status === "waiting_for_permission") {
    return snapshot.activeTurn?.id ?? pendingTurnId ?? null;
  }
  return pendingTurnId ?? snapshot?.activeTurn?.id ?? null;
}

function eventForItem(item: AgentThreadItem, sequence: number, retainAsResult = false): AgentEventPayload | null {
  if (item.kind === "agent_message") {
    return item.content ? {
      type: "snapshot_text",
      eventId: item.id,
      sequence,
      text: item.content,
      ...(retainAsResult ? { history_result: true } : {}),
    } : null;
  }
  if (item.kind === "tool_call") {
    return {
      type: item.status === "pending" ? "tool_call_pending" : "tool_call_started",
      eventId: item.id,
      callId: item.id,
      toolName: item.name,
      input: item.input_buffer || (typeof item.input === "string" ? item.input : item.input == null ? "" : JSON.stringify(item.input)),
      status: item.status,
      tool_group_id: item.tool_group_id,
      queued_at_ms: item.queued_at_ms,
      started_at_ms: item.started_at_ms,
      finished_at_ms: item.finished_at_ms,
      sequence,
    };
  }
  if (item.kind === "context" || item.kind === "final_report" || item.kind === "timeline_event") {
    return { ...item.data, eventId: item.id, sequence };
  }
  if (item.kind === "file_changes") {
    return { ...item.data, type: "file_changes", eventId: item.id, sequence };
  }
  return null;
}

// Rust owns the ordered item stream for the active turn. User items introduced
// by steer requests split assistant output into stable chronological segments.
// The first assistant segment owns the one turn timeline; the final segment
// alone owns the live typing state.
export function projectLiveThreadMessages(messages: ChatMessage[], snapshot: AgentThreadSnapshot | null): ChatMessage[] {
  const turn = snapshot?.activeTurn;
  if (!turn) return messages;

  const replacedMessages = messages.filter((message) => message.turnId === turn.id);
  const optimisticAssistant = replacedMessages.find((message) => message.role === "assistant");
  const optimisticUser = replacedMessages.find((message) => message.role === "user");
  const retainedById = new Map(replacedMessages.map((message) => [message.id, message]));
  const liveMessages = messages.filter((message) => message.turnId !== turn.id);
  const failed = turn.status === "failed";
  const cancelled = turn.status === "cancelled";
  const failureMessage = turn.error || "任务执行失败。";
  let createdAt = Math.max(turn.startedAtMs ?? optimisticUser?.createdAt ?? optimisticAssistant?.createdAt ?? 0, ...liveMessages.map((message) => message.createdAt)) + 1;
  const projected: ChatMessage[] = [];
  const assistantMessages: ChatMessage[] = [];
  let eventCount = 0;
  let segmentEvents: AgentEventPayload[] = [];
  let hasAssistantContent = false;
  let paragraphStart = false;
  let sequence = 0;
  const retainTerminalText = failed || cancelled || snapshot.items.some((item) => item.kind === "timeline_event"
    && item.data.terminalDiagnostic === true && item.data.status === "repetitive_output");

  const flushAssistant = (allowEmpty = false) => {
    if (!allowEmpty && segmentEvents.length === 0) return;
    const segmentIndex = assistantMessages.length;
    const id = segmentIndex === 0 ? `assistant:${turn.id}` : `assistant:${turn.id}:segment-${segmentIndex}`;
    const retained = retainedById.get(id) || (segmentIndex === 0 ? optimisticAssistant : undefined);
    const message: ChatMessage = {
      id,
      role: "assistant",
      content: "",
      createdAt: Math.max(createdAt, retained?.createdAt ?? 0),
      pending: false,
      error: failed,
      turnId: turn.id,
      attachments: segmentIndex === 0 ? retained?.attachments : undefined,
      events: segmentEvents,
      turnSegmentIndex: segmentIndex,
    };
    createdAt = message.createdAt + 1;
    projected.push(message);
    assistantMessages.push(message);
    segmentEvents = [];
  };

  const appendEvent = (event: AgentEventPayload) => {
    segmentEvents.push(event);
    eventCount += 1;
  };

  for (const item of snapshot.items) {
    if (item.kind === "user_message") {
      flushAssistant();
      const retained = retainedById.get(item.id) || (item.id === `user:${turn.id}` ? optimisticUser : undefined);
      const messageCreatedAt = Math.max(createdAt, retained?.createdAt ?? 0);
      createdAt = Math.max(createdAt, messageCreatedAt + 1);
      projected.push({
        id: item.id,
        role: "user",
        content: item.content,
        createdAt: messageCreatedAt,
        turnId: turn.id,
        attachments: projectThreadUserAttachments(item, retained?.attachments),
      });
      paragraphStart = false;
      continue;
    }
    const event = eventForItem(item, sequence++, retainTerminalText);
    if (event && item.kind === "file_changes") {
      event.session_id ||= event.sessionId || snapshot.id;
      event.turn_id ||= event.turnId || turn.id;
    }
    if (event) appendEvent(item.kind === "agent_message" && paragraphStart
      ? { ...event, paragraph_start: true } : event);
    if (item.kind === "agent_message" && item.content) paragraphStart = false;
    if (event && isAssistantProcessBoundary(event)) paragraphStart = true;
    hasAssistantContent ||= item.kind === "agent_message" && Boolean(item.content.trim());
    if (item.kind === "tool_call" && item.output === undefined && item.live_output) {
      appendEvent({
        ...item.live_output,
        type: "command_output",
        text: liveToolOutputText(item),
        eventId: `${item.id}:live`,
        callId: item.id,
        toolName: item.name,
        sequence: sequence++,
      });
    }
    if (item.kind === "tool_call" && (item.output !== undefined || ["done", "error", "cancelled", "skipped"].includes(item.status || ""))) {
      appendEvent({
        type: "tool_call_finished",
        eventId: `${item.id}:finished`,
        callId: item.id,
        toolName: item.name,
        input: typeof item.input === "string" ? item.input : JSON.stringify(item.input ?? {}),
        output: typeof item.output === "string" ? item.output : JSON.stringify(item.output),
        isError: item.is_error,
        status: item.status,
        tool_group_id: item.tool_group_id,
        queued_at_ms: item.queued_at_ms,
        started_at_ms: item.started_at_ms,
        finished_at_ms: item.finished_at_ms,
        sequence: sequence++,
      });
    }
  }

  // PermissionRequested is also a snapshot state transition; it need not be
  // present as a timeline item. Project the real outstanding request so the
  // footer and call details cannot claim execution while approval is pending.
  const permission = snapshot.pendingPermission;
  if (turn.status === "running" && permission && !snapshot.items.some((item) => item.kind === "timeline_event"
    && item.data.type === "permission_requested" && (item.data.requestId || item.data.request_id) === permission.requestId)) {
    const candidates = snapshot.items.filter((item) => item.kind === "tool_call"
      && item.name === permission.toolName && (item.status === "running" || item.status === "pending") && item.output === undefined);
    const call = candidates.length === 1 ? candidates[0] : undefined;
    appendEvent({ type: "permission_requested", requestId: permission.requestId, toolName: permission.toolName,
      ...(call?.kind === "tool_call" ? { callId: call.id, input: call.input } : {}), sequence: sequence++ });
  }

  if (failed && !segmentEvents.some((event) => event.terminalDiagnostic === true && event.status === "failed")) {
    appendEvent({
      type: "status",
      status: "failed",
      message: failureMessage,
      terminalDiagnostic: true,
      sequence: sequence++,
    });
  }

  if (cancelled) {
    appendEvent({
      type: "cancelled",
      timestampMs: turn.finishedAtMs ?? undefined,
      sequence: sequence++,
    });
  }

  if (segmentEvents.length > 0 || hasAssistantContent || eventCount > 0 || failed || turn.status === "running") {
    flushAssistant(true);
  }

  if (assistantMessages.length > 0) {
    const count = assistantMessages.length;
    const owner = assistantMessages[0];
    owner.startedAt = turn.startedAtMs ?? optimisticAssistant?.startedAt;
    owner.finishedAt = turn.status === "running" ? undefined : turn.finishedAtMs ?? optimisticAssistant?.finishedAt;
    if (failed && !hasAssistantContent) owner.content = failureMessage;
    assistantMessages.forEach((message, index) => {
      message.turnSegmentIndex = index;
      message.turnSegmentCount = count;
      message.pending = turn.status === "running" && index === count - 1;
    });
  }
  if (!projected.length) return messages;
  // A stale snapshot can survive for a few frames after the user starts the
  // next turn. Replace that turn at its original position so the new user
  // message never flashes before the previous assistant response.
  const firstTurnIndex = messages.findIndex((message) => message.turnId === turn.id);
  if (firstTurnIndex < 0) return [...liveMessages, ...projected];
  const before = messages.slice(0, firstTurnIndex).filter((message) => message.turnId !== turn.id);
  const after = messages.slice(firstTurnIndex).filter((message) => message.turnId !== turn.id);
  return [...before, ...projected, ...after];
}
