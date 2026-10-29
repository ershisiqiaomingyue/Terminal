import { type AgentEventPayload } from "@/platform/chatApi";
import { sanitizeAssistantDisplayText } from "./historyRecovery";
import { canonicalToolName } from "./toolIdentity";
import { TOOL_LABELS, localizeToolIds } from "./toolLabels";

// Kept as the historical public name so existing importers stay untouched.
export { TOOL_LABELS as TOOL_STEP_TITLES, localizeToolIds };

export function toolStepTitle(name: string) {
  const canonical = canonicalToolName(name);
  return TOOL_LABELS[canonical] || canonical.replace(/_/g, " ");
}
import { appendAssistantStreamText, projectAssistantSnapshotText } from "./assistantText";
import type { ChatMessage, ComposerTaskStep } from "./types";
import { assistantTerminalNotice } from "./chatErrors";



export function turnTaskSteps(message?: ChatMessage): ComposerTaskStep[] {
  if (!message) return [];
  const steps = new Map<string, ComposerTaskStep>();
  for (const event of message.events || []) {
    if (event.type !== "tool_call_pending" && event.type !== "tool_call_started" && event.type !== "tool_call_finished") continue;
    const name = String(event.toolName || event.tool_name || "工具调用");
    const id = String(event.callId || event.call_id || `${name}-${steps.size}`);
    const step = steps.get(id) || { id, title: toolStepTitle(name), status: event.type === "tool_call_pending" ? "pending" as const : "running" as const };
    if (event.type === "tool_call_started") step.status = "running";
    if (event.type === "tool_call_finished") step.status = event.is_error === true || event.error === true ? "error" : "done";
    steps.set(id, step);
  }
  return [...steps.values()];
}

export function AnalysisReport({ event }: { event?: AgentEventPayload }) {
  const report = event?.report;
  if (!report || typeof report !== "object") return null;
  const value = report as Record<string, unknown>;
  const conclusion = typeof value.conclusion === "string" ? value.conclusion : "";
  const lists = [
    ["疑似原因", value.suspectedCauses],
    ["关键证据", value.evidence],
    ["下一步", value.nextSteps],
  ] as const;
  const timeline = Array.isArray(value.timeline) ? value.timeline.filter((item): item is string => typeof item === "string") : [];
  const hasListItems = lists.some(([, items]) => Array.isArray(items) && items.some((item) => typeof item === "string" && item.trim()));
  if (!conclusion && !timeline.length && !hasListItems) return null;
  return <div className="analysis-report">
    {conclusion && <section><h3>结论</h3><p>{conclusion}</p></section>}
    {lists.map(([title, items]) => Array.isArray(items) && items.length ? <section key={title}><h3>{title}</h3><ul>{items.filter((item): item is string => typeof item === "string").map((item) => <li key={item}>{item}</li>)}</ul></section> : null)}
    {timeline.length > 0 && <section><h3>时间线</h3><ol>{timeline.map((item) => <li key={item}>{item}</li>)}</ol></section>}
  </div>;
}

export function visibleAssistantText(content: string) {
  return sanitizeAssistantDisplayText(content)
    .replace(/^\[历史助手输出[^\]]*\]\s*/u, "")
    .trim();
}

export function messageCopyText(message: ChatMessage) {
  const content = assistantResultText(message);
  if (content) return content;
  const streamed = (message.events || [])
    .filter((event) => event.type === "text_delta" && typeof event.text === "string")
    .map((event) => event.text)
    .join("")
    .trim();
  if (streamed) return streamed;
  const toolSummaries = (message.events || [])
    .filter((event) => event.type === "tool_call_started")
    .map((event) => `已执行：${toolStepTitle(String(event.toolName || event.tool_name || "工具调用"))}`);
  return [...new Set(toolSummaries)].join("\n");
}

export function assistantResultText(message: ChatMessage) {
  const events = message.events || [];
  const hasSnapshots = events.some((event) => event.type === "snapshot_text" && typeof event.text === "string");
  if (hasSnapshots) {
    return visibleAssistantText(projectAssistantSnapshotText(events, Boolean(message.error) || !message.pending).resultText);
  }
  const content = assistantTerminalNotice(message)?.contentIsDiagnostic ? "" : visibleAssistantText(message.content);
  if (content) return content;
  return visibleAssistantText(events
    .filter((event) => event.type === "text_delta" && typeof event.text === "string")
    .reduce((text, event) => appendAssistantStreamText(text, String(event.text || "")), ""));
}

export function formatHistoryTime(value: number) {
  return new Date(value).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}
