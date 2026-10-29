import type { AgentThreadItem } from "@/platform/chatApi";
import { canonicalToolName } from "./toolIdentity.ts";

export type ToolCallItem = Extract<AgentThreadItem, { kind: "tool_call" }>;

export type SnapshotToolActivity = {
  id: string;
  name: string;
  status: "pending" | "running" | "done" | "error" | "cancelled" | "skipped" | "unknown";
  command: string;
  output: string;
  toolGroupId?: string;
  queuedAt?: number;
  startedAt?: number;
  finishedAt?: number;
};

export function formatToolOutput(value: unknown) {
  if (typeof value === "string") return value;
  if (value === undefined || value === null) return "";
  if (typeof value === "object" && !Array.isArray(value)) {
    const record = value as Record<string, unknown>;
    const details = record.details && typeof record.details === "object" && !Array.isArray(record.details)
      ? record.details as Record<string, unknown>
      : {};
    const stdout = typeof record.stdout === "string" ? record.stdout : typeof details.stdout === "string" ? details.stdout : "";
    const stderr = typeof record.stderr === "string" ? record.stderr : typeof details.stderr === "string" ? details.stderr : "";
    const sections: string[] = [];
    for (const key of ["stdout", "stderr"]) {
      const stream = key === "stdout" ? stdout : stderr;
      if (stream) sections.push(`${key}:\n${stream}`);
    }
    if (sections.length) {
      const exitCode = record.exitCode ?? record.exit_code;
      const metadata = exitCode === undefined ? [] : [`退出码：${String(exitCode)}`];
      return [...metadata, ...sections].join("\n");
    }
  }
  try { return JSON.stringify(value, null, 2); } catch { return String(value); }
}

function valueText(value: unknown) {
  if (typeof value === "string") return value;
  if (value === undefined || value === null) return "";
  try { return JSON.stringify(value); } catch { return String(value); }
}

export function liveToolOutputText(item: ToolCallItem) {
  const text = valueText(item.live_output?.text);
  return item.live_output?.truncated === true ? `${text}\n[实时输出已截断]` : text;
}

function toolStatus(item: ToolCallItem, turnPending: boolean): SnapshotToolActivity["status"] {
  if (item.status && ["pending", "running", "done", "cancelled", "skipped"].includes(item.status)) {
    return item.status as SnapshotToolActivity["status"];
  }
  if (item.is_error || item.status === "error") return "error";
  if (item.output !== undefined) return "done";
  return turnPending ? "running" : "unknown";
}

export function activitiesFromThreadItems(items: ToolCallItem[], pending: boolean): SnapshotToolActivity[] {
  return items
    .filter((item) => canonicalToolName(item.name) !== "update_task_plan")
    .map((item) => ({
      id: item.id,
      name: item.name,
      status: toolStatus(item, pending),
      command: valueText(item.input_buffer || item.input),
      output: item.output === undefined ? liveToolOutputText(item) : formatToolOutput(item.output),
      ...(item.tool_group_id ? { toolGroupId: item.tool_group_id } : {}),
      ...(item.queued_at_ms ? { queuedAt: item.queued_at_ms } : {}),
      ...(item.started_at_ms ? { startedAt: item.started_at_ms } : {}),
      ...(item.finished_at_ms ? { finishedAt: item.finished_at_ms } : {}),
    }));
}
