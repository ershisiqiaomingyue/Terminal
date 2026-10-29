import type { AgentEventPayload } from "@/platform/chatApi";
import { canonicalToolName } from "./toolIdentity";

const TOOL_EVENT_TYPES = new Set(["tool_call_pending", "tool_call_started", "tool_call_input_delta", "tool_call_finished", "command_output"]);

export function toolNamesByCallId(events: AgentEventPayload[]) {
  const names = new Map<string, string>();
  for (const event of events) {
    const id = String(event.callId || event.call_id || "");
    const name = String(event.toolName || event.tool_name || "");
    if (id && name) names.set(id, name);
  }
  return names;
}

export function eventToolName(event: AgentEventPayload, names: Map<string, string>) {
  return names.get(String(event.callId || event.call_id || "")) || String(event.toolName || event.tool_name || "工具调用");
}

export function hasTimelineTools(events: AgentEventPayload[]) {
  const names = toolNamesByCallId(events);
  return events.some((event) => TOOL_EVENT_TYPES.has(event.type || "") && canonicalToolName(eventToolName(event, names)) !== "update_task_plan");
}

function inputObject(value: unknown): Record<string, unknown> | null {
  if (typeof value === "string") {
    try { value = JSON.parse(value); } catch { return null; }
  }
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

export function submittedTaskIds(events: AgentEventPayload[]) {
  const ids = new Set<string>();
  for (const event of events) {
    if (event.type !== "tool_call_finished" || event.is_error === true) continue;
    const output = inputObject(event.output ?? event.result);
    if (["submitted", "queued", "running"].includes(String(output?.status || ""))) {
      const id = output?.taskId ?? output?.task_id;
      if (typeof id === "string" && id) ids.add(id);
    }
  }
  return ids;
}

export function isWorkspaceChangePath(path: string) {
  // 两个名字都跳过：迁移前写的是 .android-tool，迁移后是 .android-toolkit。
  return !/(?:^|[\\/])\.android-tool(?:kit)?(?:[\\/]|$)/i.test(path);
}
