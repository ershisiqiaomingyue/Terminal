import type { AgentEventPayload, AgentThreadSnapshot } from "@/platform/chatApi";
import type { ChatMessage, ExecutionTarget, LivePermissionRequest } from "./types";
import { DEFAULT_THINKING_COPY, type ThinkingCopy, type ThinkingCopyPhase } from "../../platform/thinkingCopy.ts";
import { timelineContextCompactions } from "./contextCompaction.ts";

type TargetProfile = { id: string; name: string; defaultRoot?: string | null };

export type { ThinkingCopyPhase } from "../../platform/thinkingCopy.ts";

export function thinkingStatusCopy(phase: ThinkingCopyPhase, seed = 0, copy: ThinkingCopy = DEFAULT_THINKING_COPY) {
  const choices = copy[phase]?.length ? copy[phase] : DEFAULT_THINKING_COPY[phase];
  const index = Number.isFinite(seed) ? Math.abs(Math.trunc(seed)) % choices.length : 0;
  return choices[index];
}

type PendingMessagePhase = "waiting" | "reasoning" | "writing" | "tool" | "toolFinished" | "permission" | "retry" | "status";
type PendingMessageState = {
  phase: PendingMessagePhase;
  activity: { text: string; toolName?: string; detail?: string };
  reasoning?: string;
};

export function pendingMessageState(events: AgentEventPayload[], runtimeActivity: string, turnId = "", copy: ThinkingCopy = DEFAULT_THINKING_COPY): PendingMessageState {
  let seed = 0;
  for (let index = 0; index < turnId.length; index += 1) seed = (seed * 31 + turnId.charCodeAt(index)) >>> 0;
  const calls = new Map<string, AgentEventPayload>();
  const permissions = new Map<string, AgentEventPayload>();
  let permissionSettled = false;
  let latestPhase = "";
  let latestStatus = "";
  let reasoning = "";
  for (const event of [...events].sort((a, b) => Number(a.sequence ?? 0) - Number(b.sequence ?? 0))) {
    const callId = String(event.callId || event.call_id || event.toolName || event.tool_name || "");
    if (["tool_call_pending", "tool_call_started", "tool_call_input_delta"].includes(event.type || "")) {
      if (event.type === "tool_call_input_delta") {
        if (!calls.has(callId)) calls.set(callId, event);
      } else calls.set(callId, event);
      latestPhase = "tool";
    } else if (event.type === "tool_call_finished") {
      calls.delete(callId);
      latestPhase = "tool_finished";
    } else if (["snapshot_text", "text_delta", "reasoning_delta"].includes(event.type || "") && event.text?.trim() && event.origin !== "runtime_fallback") {
      latestPhase = event.type === "reasoning_delta" ? "reasoning" : "text";
      if (event.type === "reasoning_delta") reasoning = event.text;
    } else if (event.type === "permission_requested") {
      permissions.set(String(event.requestId || event.request_id || ""), event);
    } else if (event.type === "permission_decision") {
      permissions.delete(String(event.requestId || event.request_id || ""));
      permissionSettled = true;
    } else if (event.type === "status" && event.message?.trim()) {
      latestStatus = event.message.trim();
      latestPhase = "status";
    }
  }
  const state = (phase: PendingMessagePhase, text: string): PendingMessageState => ({ phase, activity: { text }, ...(phase === "reasoning" && reasoning ? { reasoning } : {}) });
  const modelState = (phase: ThinkingCopyPhase) => state(phase, thinkingStatusCopy(phase, seed, copy));
  // Runtime copy can outlive its corresponding status event. New model output
  // is authoritative once that event has been followed by another phase.
  const suppliedRuntime = runtimeActivity.trim();
  const staleRuntime = (suppliedRuntime === latestStatus && latestPhase !== "status")
    || (permissionSettled && permissions.size === 0 && /批准|权限|等待用户回答/.test(suppliedRuntime));
  const runtime = staleRuntime ? "" : suppliedRuntime;
  const permission = [...permissions.values()].at(-1);
  if (permission) return state("permission", String(permission.toolName || permission.tool_name) === "ask_user" ? "等待用户回答" : "等待工具权限批准");
  if (/批准|权限|等待用户回答/.test(runtime)) return state("permission", runtime);
  if (timelineContextCompactions(events, true).some(record => record.status === "running")) return state("status", "正在压缩上下文");
  const call = [...calls.values()].at(-1);
  if (call) {
    let input = call.input ?? call.arguments;
    if (typeof input === "string") {
      try { input = JSON.parse(input); } catch { /* Input may still be streaming. */ }
    }
    const args = input && typeof input === "object" && !Array.isArray(input) ? input as Record<string, unknown> : {};
    const detail = [args.command, args.cmd, args.path, args.file_path, args.query, args.pattern]
      .find((value): value is string => typeof value === "string" && Boolean(value.trim()));
    return { phase: "tool", activity: { text: "", toolName: String(call.toolName || call.tool_name || "工具调用"), detail } };
  }
  // General runtime narration (for example a steer receipt) can lag behind
  // model deltas; actual reasoning/reply output determines the current phase.
  if (!/重试/.test(runtime)) {
    if (latestPhase === "text") return modelState("writing");
    if (latestPhase === "reasoning") return modelState("reasoning");
  }
  // Completed-tool activity is not the current phase of a still-running turn.
  const genericRuntime = /等待模型响应|正在思考|生成回复|模型正在分析并生成回答/.test(runtime);
  // 工具生命周期的状态文案（「已接收 X 调用」「正在执行 X」「已完成 X」）由上面的
  // calls 分支负责呈现，这里排除掉，避免把工具名当成本轮的当前旁白。
  const toolLifecycleRuntime = /^(?:已接收|正在执行|已完成)\s*\S/.test(runtime);
  if (runtime && !genericRuntime && !toolLifecycleRuntime) {
    return state(/重试/.test(runtime) ? "retry" : "status", runtime);
  }
  if (latestPhase === "status" && latestStatus) {
    if (/等待模型响应/.test(latestStatus)) return modelState("waiting");
    if (/正在思考/.test(latestStatus)) return modelState("reasoning");
    // A provider status saying "generating" is not proof that reply SSE has arrived.
    if (/生成回复|模型正在分析并生成回答/.test(latestStatus)) return state("waiting", thinkingStatusCopy("writing", seed, copy));
    return state(/批准|权限|等待用户回答/.test(latestStatus) ? "permission" : /重试/.test(latestStatus) ? "retry" : "status", latestStatus);
  }
  if (latestPhase === "reasoning") return modelState("reasoning");
  if (latestPhase === "text") return modelState("writing");
  if (latestPhase === "tool_finished") return modelState("toolFinished");
  if (/正在思考/.test(runtime)) return modelState("reasoning");
  if (/生成回复/.test(runtime)) return state("waiting", thinkingStatusCopy("writing", seed, copy));
  return modelState("waiting");
}

export function pendingMessageActivity(events: AgentEventPayload[], runtimeActivity: string, turnId = "", copy: ThinkingCopy = DEFAULT_THINKING_COPY) {
  return pendingMessageState(events, runtimeActivity, turnId, copy).activity;
}

export function snapshotIsSending(snapshot: AgentThreadSnapshot | null) {
  return snapshot?.activeTurn?.status === "running" || snapshot?.status === "waiting_for_permission";
}

export function conversationIsSending(snapshot: AgentThreadSnapshot | null, messages: ChatMessage[]) {
  return snapshotIsSending(snapshot) || messages.some((message) => message.pending
    && (!snapshot?.activeTurn || message.turnId !== snapshot.activeTurn.id));
}

export function snapshotQueuedMessages(snapshot: AgentThreadSnapshot | null) {
  return (snapshot?.queue || []).map((item) => ({
    id: item.id,
    content: item.content,
    ...(item.images?.length ? { images: item.images } : {}),
    ...(item.artifacts?.length ? { artifacts: item.artifacts } : {}),
  }));
}

export function snapshotPermission(snapshot: AgentThreadSnapshot | null, sessionId: string): LivePermissionRequest | null {
  if (!snapshot || snapshot.id !== sessionId || !snapshotIsSending(snapshot)) return null;
  const unresolved = new Map<string, AgentEventPayload>();
  for (const item of snapshot.items) {
    if (item.kind !== "timeline_event") continue;
    const event = item.data;
    const requestId = String(event.requestId || event.request_id || "");
    if (!requestId) continue;
    if (event.type === "permission_requested") unresolved.set(requestId, event);
    if (event.type === "permission_decision") unresolved.delete(requestId);
  }
  // Recover a real request from the event stream if permission metadata is
  // absent. An explicit cleared field on a running thread stays authoritative.
  const fallback = snapshot.pendingPermission === undefined || snapshot.status === "waiting_for_permission"
    ? [...unresolved.values()].at(-1) : undefined;
  const permission = snapshot.pendingPermission || (fallback ? {
    requestId: String(fallback.requestId || fallback.request_id),
    toolName: String(fallback.toolName || fallback.tool_name || ""),
    expiresAtMs: Number(fallback.expiresAtMs ?? fallback.expires_at_ms ?? 0) || undefined,
  } : null);
  if (!permission?.requestId || !permission.toolName) return null;
  const event = unresolved.get(permission.requestId);
  const callId = event?.callId || event?.call_id;
  const candidates = snapshot.items.filter((item) => item.kind === "tool_call" && item.output === undefined
    && (callId ? item.id === callId : item.name === permission.toolName && (item.status === "running" || item.status === "pending")));
  const running = candidates.filter(item => item.kind === "tool_call" && item.status === "running");
  const tool = running.length === 1 ? running[0] : candidates.length === 1 ? candidates[0] : undefined;
  let input: unknown = event?.input ?? (tool?.kind === "tool_call" ? tool.input_buffer || tool.input : undefined);
  if (typeof input === "string") { try { input = JSON.parse(input); } catch { input = null; } }
  const data = input && typeof input === "object" && !Array.isArray(input) ? input as Record<string, unknown> : {};
  const question = permission.toolName === "ask_user" ? {
    question: typeof data.question === "string" ? data.question : "AI 需要补充信息，请输入回答。",
    options: Array.isArray(data.options) ? data.options.filter((value): value is string => typeof value === "string") : [],
  } : undefined;
  return {
    sessionId,
    requestId: permission.requestId,
    toolName: permission.toolName,
    ...(permission.expiresAtMs != null ? { expiresAtMs: permission.expiresAtMs } : {}),
    ...(!question && Object.keys(data).length ? { input: data } : {}),
    ...(question ? { question } : {}),
  };
}

export function snapshotExecutionTarget(
  snapshot: AgentThreadSnapshot | null,
  sending: boolean,
  sshConnections: TargetProfile[],
  wslProfiles: TargetProfile[],
): ExecutionTarget | null {
  const target = snapshot?.runtime?.executionTarget;
  if (!sending || !target) return null;
  if (target.kind === "local") return { kind: "local", label: "本机工作区", root: target.root || undefined };
  if (target.kind === "ssh") {
    const connection = sshConnections.find((item) => item.id === target.connectionId);
    return { kind: "ssh", id: target.connectionId || undefined, label: connection ? `SSH · ${connection.name}` : "SSH", root: target.root || connection?.defaultRoot || undefined };
  }
  const profile = wslProfiles.find((item) => item.id === target.profileId);
  return { kind: "wsl", id: target.profileId || undefined, label: profile ? `WSL · ${profile.name}` : "WSL", root: target.root || profile?.defaultRoot || undefined };
}
