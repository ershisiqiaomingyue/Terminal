import { createPortal } from "react-dom";
import React, { memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, CircleHelp, MessageCircle, Wrench, XCircle } from "lucide-react";
import { Spinner } from "@/components/ui";
import type { AgentEventPayload } from "@/platform/chatApi";
import { MarkdownMessage } from "./MarkdownMessage";
import { eventToolName, toolNamesByCallId } from "./activityState";
import { buildActivityTimeline, splitActivityTimeline, timelineEntryOrder, timelineTextIsStreaming } from "./activityTimeline";
import { activitiesFromThreadItems, formatToolOutput, type ToolCallItem } from "./threadSnapshotTools";
import { commandGroupLabel, compactToolGroupLabel, disclosureIsOpen, timelineDisclosureKey, toolGroupPresentationState, transferSummary, type ToolGroupPresentationState } from "./timelinePresentation";
import { canonicalToolName, toolEnvironmentLabel } from "./toolIdentity";
import { TOOL_LABELS, localizeToolIds, toolLabel } from "./toolLabels";
import { sanitizeAssistantDisplayText } from "./historyRecovery";
import { ToolActivityStatus } from "./ToolActivityStatus";
import { ReasoningPreview } from "./ReasoningPreview";
import { ChatCompactionProgress } from "./ChatCompactionProgress";
import { timelineContextCompactions } from "./contextCompaction";

type ActivityStatus = "pending" | "running" | "done" | "error" | "cancelled" | "unknown" | "interrupted" | "skipped";
type Activity = {
  id: string;
  name: string;
  status: ActivityStatus;
  command: string;
  output: string;
  progress: string[];
  progressUpdatedAt?: number;
  queuedAt?: number;
  startedAt?: number;
  finishedAt?: number;
};

function LiveProgressLog({ progress }: { progress: string[] }) {
  const elementRef = useRef<HTMLPreElement>(null);
  const followLatest = useRef(true);
  const text = progress.join("\n");

  useLayoutEffect(() => {
    const element = elementRef.current;
    if (element && followLatest.current) element.scrollTop = element.scrollHeight;
  }, [text]);

  return <pre className="agent-command-live-output" ref={elementRef} tabIndex={0}
    onScroll={(event) => {
      const element = event.currentTarget;
      followLatest.current = element.scrollHeight - element.clientHeight - element.scrollTop < 28;
    }}><code>{text}</code></pre>;
}

function eventText(event: AgentEventPayload, keys: string[]) {
  for (const key of keys) {
    const value = event[key];
    if (typeof value === "string" && value.trim()) return value;
    if (value && typeof value === "object") return JSON.stringify(value);
  }
  return "";
}

function eventOutputText(event: AgentEventPayload, keys: string[]) {
  for (const key of keys) {
    const value = event[key];
    if (typeof value === "string" && value.trim()) return value;
    if (value !== undefined && value !== null) return formatToolOutput(value);
  }
  return "";
}

function inputRecord(command: string) {
  if (!command.trim()) return null;
  try {
    const value = JSON.parse(command) as Record<string, unknown>;
    return value && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

function inputValue(input: Record<string, unknown> | null, keys: string[]) {
  if (!input) return "";
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

function commandText(command: string, fallback: string) {
  if (!command.trim()) return fallback;
  const input = inputRecord(command);
  if (!input) return command;
  const executable = inputValue(input, ["command", "cmd", "script", "input"]);
  const args = Array.isArray(input.args)
    ? input.args
      .filter((value): value is string | number => typeof value === "string" || typeof value === "number")
      .map((value) => {
        const text = String(value);
        return /\s/.test(text) ? JSON.stringify(text) : text;
      })
    : [];
  return [executable, ...args].filter(Boolean).join(" ") || fallback;
}



function argumentPath(command: string) {
  return inputValue(inputRecord(command), [
    "path",
    "file_path",
    "filePath",
    "target",
    "directory",
    // Transfer tools name the source and destination explicitly; without these
    // keys the heading fell back to dumping the whole argument JSON.
    "local_path",
    "remote_path",
  ]);
}

/**
 * Sums up transfer arguments as "source → destination" so a file move reads as
 * a sentence instead of a serialized tool payload.
 */
function argumentTransfer(command: string) {
  const input = inputRecord(command);
  return input ? transferSummary(input) : "";
}

function eventTime(event: AgentEventPayload) {
  const value = event.timestampMs ?? event.timestamp_ms;
  const timestamp = typeof value === "number" ? value : Number(value);
  return Number.isFinite(timestamp) && timestamp > 0 ? timestamp : undefined;
}

function eventTimestamp(event: AgentEventPayload, ...keys: string[]) {
  for (const key of keys) {
    const value = event[key];
    const timestamp = typeof value === "number" ? value : Number(value);
    if (Number.isFinite(timestamp) && timestamp > 0) return timestamp;
  }
  return undefined;
}

function eventString(event: AgentEventPayload, keys: string[]) {
  for (const key of keys) {
    const value = event[key];
    if (typeof value === "string" && value.trim()) return value;
  }
  return "";
}

function remoteProgressText(event: AgentEventPayload) {
  const progress = event.progress;
  if (typeof progress === "string") return progress.trim();
  if (!progress || typeof progress !== "object" || Array.isArray(progress)) return "";
  const record = progress as Record<string, unknown>;
  const message = typeof record.message === "string" ? record.message.trim() : "";
  if (message) return message;
  const itemPath = typeof record.itemPath === "string" ? record.itemPath.trim() : "";
  const path = typeof record.path === "string" ? record.path.trim() : "";
  const stage = typeof record.stage === "string" ? record.stage : "";
  if (stage === "found" && itemPath) return `找到匹配项：${itemPath}`;
  if (stage === "scanning" && (itemPath || path)) return `正在检查：${itemPath || path}`;
  if (stage === "completed") return "操作完成，正在整理结果";
  return itemPath || path || stage;
}

function progressAge(updatedAt: number | undefined, now: number) {
  if (!updatedAt) return "";
  const seconds = Math.max(0, Math.floor((now - updatedAt) / 1_000));
  if (seconds < 5) return "刚刚更新";
  return `连续 ${seconds} 秒未收到新结果`;
}

function skippedToolEvent(event: AgentEventPayload) {
  const output = event.output ?? event.result;
  if (output && typeof output === "object") return (output as Record<string, unknown>).skipped === true;
  if (typeof output !== "string") return false;
  try {
    return (JSON.parse(output) as Record<string, unknown>).skipped === true;
  } catch {
    return false;
  }
}

function activityDuration(activity: Activity, now: number) {
  if (!activity.startedAt) return "";
  const elapsed = Math.max(0, (activity.finishedAt || now) - activity.startedAt);
  return `${Math.max(1, Math.ceil(elapsed / 1_000))}s`;
}

function isCommandActivity(activity: Activity) {
  return ["run_local_command", "run_local_shell", "run_interactive_shell"].includes(canonicalToolName(activity.name));
}

function activitySummary(activity: Activity, status: ActivityStatus) {
  const title = toolLabel(activity.name);
  if (canonicalToolName(activity.name) === "ask_user") {
    if (status === "running") return "等待用户回答";
    if (status === "done") return inputRecord(activity.output)?.status === "skipped" ? "用户跳过了询问" : "已收到用户回答";
  }
  if (isCommandActivity(activity)) {
    if (status === "pending") return "命令等待执行";
    if (status === "running") return "正在运行命令";
    if (status === "error") return "命令执行失败";
    if (status === "cancelled") return "命令已取消";
    if (status === "unknown") return "命令结果未保存";
    if (status === "interrupted") return "上次命令运行中断";
    if (status === "skipped") return "指令未执行";
    return "已运行命令";
  }
  if (status === "pending") return `${title}等待执行`;
  if (status === "running") return `正在${title}`;
  if (status === "error") return `${title}失败`;
  if (status === "cancelled") return `${title}已取消`;
  if (status === "unknown") return `${title}结果未保存`;
  if (status === "interrupted") return `上次${title}中断`;
  if (status === "skipped") return `已跳过${title}`;
  return `已${title}`;
}

function activityLiveStatus(activity: Activity, status: ActivityStatus, now: number) {
  if (status !== "running") return "";
  const latestProgress = activity.progress[activity.progress.length - 1];
  if (latestProgress) return latestProgress;
  const duration = activityDuration(activity, now);
  return duration ? `已持续 ${duration}` : "";
}

function activityDetail(activity: Activity) {
  if (isCommandActivity(activity)) return commandText(activity.command, activity.name);
  const input = inputRecord(activity.command);
  if (canonicalToolName(activity.name) === "ask_user") return inputValue(input, ["question"]);
  return input ? JSON.stringify(input, null, 2) : activity.command;
}

function activityCompactPreview(activity: Activity) {
  const input = inputRecord(activity.command);
  // Input deltas can contain incomplete JSON. Wait for a complete argument
  // record rather than exposing its wire format in the live heading.
  if (!input && /^[\s]*[\[{]/.test(activity.command)) return toolLabel(activity.name);
  const transfer = argumentTransfer(activity.command);
  if (transfer) return transfer;
  const target = argumentPath(activity.command)
    || inputValue(input, ["query", "pattern", "keyword", "url", "host"]);
  if (target) return target;
  if (isCommandActivity(activity)) return commandText(activity.command, toolLabel(activity.name));
  const detail = activityDetail(activity).trim();
  // Only prose arguments belong in the heading; a raw JSON blob is unreadable
  // and made the running line read "正在上传文件 {"local_path": ...".
  if (!detail || detail === "{}" || /^[\[{]/.test(detail)) return toolLabel(activity.name);
  return detail.replace(/\s+/g, " ");
}

function groupPresentationState(activities: Activity[], running: boolean): ToolGroupPresentationState {
  return toolGroupPresentationState(activities.map((activity) => activity.status), running);
}

function activityGroupLabel(activity: Activity, state: ToolGroupPresentationState) {
  const preview = activityCompactPreview(activity).replace(/\s+/g, " ").trim();
  const title = toolLabel(activity.name);
  if (canonicalToolName(activity.name) === "ask_user") return activitySummary(activity, state);
  if (isCommandActivity(activity)) return commandGroupLabel(state, preview !== title ? preview : "");
  if (state === "running") return preview && preview !== title ? `正在${title} ${preview}` : `正在${title}`;
  if (state === "error") return `${title}失败`;
  if (state === "pending") return `${title}等待执行`;
  if (state === "cancelled") return `${title}已取消`;
  if (state === "skipped") return `${title}未执行`;
  if (state === "interrupted") return `${title}中断`;
  if (state === "unknown") return `${title}结果未保存`;
  return `已${title}`;
}

function hasVisibleInput(activity: Activity) {
  const input = activity.command.trim();
  return Boolean(input && input !== "{}");
}

function outputPreview(output: string, expanded: boolean) {
  if (expanded || output.length <= 8_000) return output;
  return `${output.slice(0, 8_000)}\n\n… 输出较长，展开后查看完整内容`;
}

function formatDuration(startedAt?: number, finishedAt?: number) {
  if (!startedAt || !finishedAt) return "";
  const seconds = Math.max(0, Math.floor((finishedAt - startedAt) / 1_000));
  if (seconds < 1) return "<1秒";
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  if (hours) return `${hours}小时 ${minutes}分钟 ${remainder}秒`;
  if (minutes) return `${minutes}分钟 ${remainder}秒`;
  return `${remainder}秒`;
}

function ToolEnvironmentBadges({ names }: { names: string[] }) {
  const environments = [...new Set(names.map(toolEnvironmentLabel).filter(Boolean))];
  return <>{environments.map(environment => <small key={environment} className="tool-environment-badge" title={`执行环境：${environment}`}>
    {environment === "Windows" ? "Win" : environment}
  </small>)}</>;
}

type AgentActivityTimelineProps = { liveStatusHost?: HTMLElement | null; liveStatusLabel?: string; events: AgentEventPayload[]; toolItems?: ToolCallItem[]; startedAt?: number; finishedAt?: number; pending?: boolean; failed?: boolean; paused?: boolean; expanded?: boolean; includeResultText?: boolean; historyAvailable?: boolean; showSummary?: boolean; linkedDetails?: boolean; onExpandedChange?: (expanded: boolean) => void; onDetailsIntent?: () => void; onLayoutChange?: (anchor?: HTMLElement) => void; onPathMenu?: (path: string, x: number, y: number) => void };

function AgentActivityTimelineComponent({ liveStatusHost, liveStatusLabel, events, toolItems, startedAt, finishedAt, pending = false, failed = false, paused = false, expanded, includeResultText = false, historyAvailable = false, showSummary = true, linkedDetails = false, onExpandedChange, onDetailsIntent, onLayoutChange, onPathMenu }: AgentActivityTimelineProps) {
  const reactId = useId().replace(/:/g, "");
  const turnDetailsId = `agent-turn-details-${reactId}`;
  const [groupOpen, setGroupOpen] = useState(() => disclosureIsOpen(expanded));
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [toolGroupOpen, setToolGroupOpen] = useState<Record<string, boolean>>({});
  const [reasoningOpen, setReasoningOpen] = useState<Record<string, boolean>>({});
  const [fullOutputOpen, setFullOutputOpen] = useState<Record<string, boolean>>({});
  const [now, setNow] = useState(() => Date.now());
  const eventActivities = useMemo(() => {
    const grouped = new Map<string, Activity>();
    const names = toolNamesByCallId(events);
    for (const event of events) {
      if (!["tool_call_pending", "tool_call_started", "tool_call_input_delta", "tool_call_finished", "command_output"].includes(event.type || "")) continue;
      const name = eventToolName(event, names);
      if (canonicalToolName(name) === "update_task_plan") continue;
      const suppliedId = String(event.callId || event.call_id || "");
      // Some stream adapters omit callId on output and input deltas. Reuse
      // the matching live call so one instruction cannot render twice.
      const id = suppliedId || [...grouped.values()].reverse()
        .find((activity) => activity.name === name && activity.status === "running")?.id
        || `${name}-${grouped.size}`;
      const initialStatus: ActivityStatus = event.type === "tool_call_pending" || event.status === "pending" ? "pending" : "running";
      const activity: Activity = grouped.get(id) || { id, name, status: initialStatus, command: "", output: "", progress: [] };
      if (event.type === "tool_call_pending") {
        const input = eventText(event, ["arguments", "input", "command", "message"]);
        if (input && input.trim() !== "{}") activity.command = input;
        activity.status = "pending";
        activity.queuedAt ||= eventTimestamp(event, "queued_at_ms", "queuedAtMs") || eventTime(event);
      }
      if (event.type === "tool_call_started") {
        const input = eventText(event, ["arguments", "input", "command", "message"]);
        if (input && input.trim() !== "{}") activity.command = input;
        activity.status = event.status === "cancelled" ? "cancelled" : event.status === "skipped" ? "skipped" : "running";
        activity.queuedAt ||= eventTimestamp(event, "queued_at_ms", "queuedAtMs");
        activity.startedAt ||= eventTimestamp(event, "started_at_ms", "startedAtMs") || eventTime(event);
      }
      if (event.type === "tool_call_input_delta") {
        const delta = eventText(event, ["delta", "input_delta", "arguments", "input", "text"]);
        if (delta) activity.command += delta;
      }
      if (event.type === "command_output") {
        const progress = remoteProgressText(event);
        if (progress) {
          if (activity.progress[activity.progress.length - 1] !== progress) {
            activity.progress = [...activity.progress, progress].slice(-80);
          }
          activity.progressUpdatedAt = eventTime(event) || activity.progressUpdatedAt || Date.now();
        } else {
          activity.output += eventOutputText(event, ["data", "output", "text", "message"]);
        }
      }
      if (event.type === "tool_call_finished") {
        const explicitStatus = String(event.status || "");
        const nextStatus: ActivityStatus = explicitStatus === "cancelled" ? "cancelled"
          : explicitStatus === "skipped" ? "skipped"
          : explicitStatus === "interrupted" ? "interrupted"
          : explicitStatus === "unknown" ? "unknown"
          : explicitStatus === "pending" ? "pending"
          : explicitStatus === "error" ? "error"
          : event.resultMissing === true || event.result_missing === true
          ? "unknown"
          : event.is_error === true || event.isError === true || event.error === true ? "error" : skippedToolEvent(event) ? "skipped" : "done";
        // Recovery may synthesize a missing-result event after a stale
        // snapshot. A real successful result for the same call remains the
        // authoritative status and must not turn into a red failure marker.
        const recoverySynthetic = event.recoverySynthetic === true || event.recovery_synthetic === true;
        if (recoverySynthetic) {
          if (activity.status !== "done") activity.status = "interrupted";
        } else {
          activity.status = nextStatus;
        }
        const input = eventText(event, ["input", "arguments", "command", "message"]);
        if (input) activity.command = input;
        activity.queuedAt ||= eventTimestamp(event, "queued_at_ms", "queuedAtMs");
        activity.startedAt ||= eventTimestamp(event, "started_at_ms", "startedAtMs");
        activity.finishedAt = eventTimestamp(event, "finished_at_ms", "finishedAtMs") || eventTime(event) || activity.finishedAt;
        const result = eventOutputText(event, ["output", "result", "message"]);
        if (result && !activity.output) activity.output = result;
      }
      grouped.set(id, activity);
    }
    return [...grouped.values()];
  }, [events]);
  const snapshotActivities = useMemo(() => {
    const liveById = new Map((toolItems || []).map((item) => [item.id, item.live_output]));
    return activitiesFromThreadItems(toolItems || [], pending).map((activity) => {
      const live = liveById.get(activity.id);
      const progress = live ? remoteProgressText(live) : "";
      return { ...activity, progress: progress ? [progress] : [] };
    });
  }, [pending, toolItems]);
  const activities = useMemo(() => {
    const source = toolItems === undefined ? eventActivities : snapshotActivities;
    return paused && !pending ? source.map(activity => activity.status === "running" || activity.status === "pending"
      ? { ...activity, status: "interrupted" as const } : activity) : source;
  }, [eventActivities, snapshotActivities, toolItems, paused, pending]);
  const timeline = useMemo(() => {
    const names = toolNamesByCallId(events);
    const activityById = new Map(activities.map((activity) => [activity.id, activity]));
    return buildActivityTimeline(events, activities, { pending, failed, includeResultText }, (event) => {
      const id = String(event.callId || event.call_id || "");
      const name = eventToolName(event, names);
      const path = argumentPath(eventText(event, ["arguments", "input", "command", "message"]));
      return activityById.get(id) || activities.find((item) => item.name === name && argumentPath(item.command) === path);
    });
  }, [activities, events, failed, pending, includeResultText]);
  const projection = useMemo(() => splitActivityTimeline(timeline, pending), [timeline, pending]);
  // A null host means the footer is mounting; do not briefly duplicate its
  // active trace in the historical body before the ref becomes available.
  const committedTimeline = liveStatusHost !== undefined ? projection.committed : timeline;
  const liveTools = projection.live.flatMap(entry => entry.type === "tools" ? entry.activities : []);
  const liveReasoning = projection.live.find(entry => entry.type === "reasoning");
  const liveEntry = liveTools.length
    ? { type: "tools" as const, id: projection.live.find(entry => entry.type === "tools")!.id, activities: liveTools }
    : liveReasoning;
  const contextEvents = useMemo(() => timelineContextCompactions(events, pending), [events, pending]);
  const liveCompaction = pending ? [...contextEvents].reverse().find(record => record.status === "running") : undefined;
  const committedCompactions = liveStatusHost !== undefined ? contextEvents.filter(record => record !== liveCompaction) : contextEvents;
  const snapshotErrors = events.filter((event) => event.type === "file_snapshot_error");
  const permissionEvents = events.filter((event) => event.type === "permission_requested" && (event.requestId || event.request_id));
  const permissionDecisions = new Map(events.filter((event) => event.type === "permission_decision").map((event) => [String(event.requestId || event.request_id || ""), event]));
  const steerEvents = events.filter((event) => event.type === "steered" && eventText(event, ["message", "text"]).trim());
  // 追加要求的即时回执。追加不会打断当前轮（与 DSH 的 steer 语义一致），
  // 要等模型下一次步骤边界才生效；长命令期间 pending_calls 长期非空，
  // "steered" 迟迟不发，用户点完追加后看不到任何反馈，会以为指令丢了。
  // steer_accepted 在受理瞬间发出，这里把它提升到折叠区之外常驻显示。
  // 后端真正 apply 之后（"steered" 到达）或本轮结束时都不再需要这条回执。
  const acceptedSteerMessages = new Set(steerEvents.map((event) => eventText(event, ["message", "text"]).trim()));
  // N6-C：turn 终态时后端回收未送达的追加要求并广播 steer_rejected；
  // 对应的「已收到追加要求」回执翻成「未送达」提示，而不是凭空消失。
  const rejectedSteerIds = new Set(events.filter((event) => event.type === "steer_rejected").map((event) => String(event.steerId || event.steer_id || "")));
  const pendingSteerNotice = pending
    ? events.filter((event) => event.type === "steer_accepted" && eventText(event, ["message", "text"]).trim() && !acceptedSteerMessages.has(eventText(event, ["message", "text"]).trim())).at(-1)
    : undefined;
  const rejectedSteerNotices = events.filter((event) => event.type === "steer_accepted"
    && eventText(event, ["message", "text"]).trim()
    && (rejectedSteerIds.has(String(event.steerId || event.steer_id || ""))
      || (String(event.steerId || event.steer_id || "") === "" && !pending && !acceptedSteerMessages.has(eventText(event, ["message", "text"]).trim()))));
  const turnFinished = !pending;
  const running = !turnFinished && (pending || activities.some((activity) => activity.status === "running"));

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);
  const elapsed = formatDuration(startedAt, finishedAt || (running ? now : undefined));
  const statusEvents = events.filter((event) => event.type === "status" && eventText(event, ["message", "status", "text"]).trim());
  const timelineStatusEvents = statusEvents.filter((event) => event.terminalDiagnostic !== true);
  // Generic runtime copy belongs in the live footer; retain actual retry/wait records.
  const visibleStatusEvents = timelineStatusEvents.filter((event) => ["provider_waiting", "provider_retry"].includes(String(event.status || "")));
  const latestEvent = events.at(-1);
  // 「可展开」只由过程内容决定：工具指令、思考、权限/状态记录等。正文条目不参与判断，
  // 因为正文无论展开与否都必须显示（见下方 committedBodyEntries 的渲染分支）。
  // 另外只看已落定（committed）的部分：流式进行中时，正在跑的思考与工具会被
  // splitActivityTimeline 摘到 live 部分、由消息尾部的活动条展示，正文里一条都没有，
  // 若按全量 timeline 判断，点开只会把箭头翻转一下、下面依旧空着。
  const committedBodyEntries = committedTimeline.filter((entry) => entry.type === "text");
  const committedProcessCount = committedTimeline.length - committedBodyEntries.length;
  const hasProcessDetails = committedProcessCount > 0 || committedCompactions.length > 0
    || visibleStatusEvents.length > 0 || permissionEvents.length > 0 || snapshotErrors.length > 0 || steerEvents.length > 0;
  // historyAvailable 代表「历史轮次有详情、还没拉下来」，点开要触发懒加载，保留可点；
  // linkedDetails 只说明本条是跨段 turn 的归属者，本身不代表有内容，得有过程条目才算。
  const hasDetails = historyAvailable || hasProcessDetails || (linkedDetails && timeline.some((entry) => entry.type !== "text"));
  const hasBodyContent = committedBodyEntries.length > 0;
  if (showSummary && !elapsed && !hasDetails && !hasBodyContent && !pendingSteerNotice) return null;
  if (!showSummary && !hasDetails && !hasBodyContent && !pendingSteerNotice) return null;
  const disclosureOpen = Boolean(expanded ?? groupOpen);
  const detailsOpen = hasDetails && disclosureOpen;
  // 正文条目始终渲染，过程内容只在展开时渲染，所以正文不会因为「没有指令可展开」而消失。
  // 但正文若由消息正文（answer）承担（timelineOwnsText 为假或未展开），就不能再渲染一遍。
  const bodyRenderedInTimeline = hasBodyContent && includeResultText && disclosureOpen;
  const processVisible = (detailsOpen && hasProcessDetails) || bodyRenderedInTimeline;
  if (!showSummary && !processVisible && !pendingSteerNotice && !(liveStatusHost && liveEntry)) return null;
  const awaitingToolPermission = (activity: Activity) => pending && (activity.status === "running" || activity.status === "pending") && permissionEvents.some((event) => !permissionDecisions.has(String(event.requestId || event.request_id))
    && (String(event.callId || event.call_id || "") === activity.id
      || (!(event.callId || event.call_id) && canonicalToolName(String(event.toolName || event.tool_name || "")) === canonicalToolName(activity.name))));
  const renderActivity = (activity: Activity, child = false, scope: "live" | "committed" = "committed") => {
    const awaitingPermission = awaitingToolPermission(activity);
    const awaitingAnswer = awaitingPermission && ["ask_user", "request_ssh_connection"].includes(canonicalToolName(activity.name));
    const status: ActivityStatus = activity.status === "running" && turnFinished ? "unknown" : awaitingPermission ? "pending" : activity.status;
    const stateKey = timelineDisclosureKey(scope, activity.id);
    const expanded = disclosureIsOpen(open[stateKey]);
    const outputExpanded = Boolean(fullOutputOpen[stateKey]);
    const detail = activityDetail(activity);
    const command = isCommandActivity(activity);
    const visibleInput = hasVisibleInput(activity);
    const visibleOutput = Boolean(activity.output);
    const visibleProgress = Boolean(activity.progress.length);
    const waitingText = awaitingPermission ? (awaitingAnswer ? "等待你的回答。" : "等待权限批准，请在权限申请弹窗中允许或拒绝。")
      : activity.command.trim() === "{}" ? (status === "running" ? "此操作无需调用参数，正在执行。" : "此操作无需调用参数，正在等待调度执行。")
      : status === "running" ? "正在等待工具返回结果。"
      : "正在等待模型补全调用参数，尚未执行。";
    const progressStatus = status === "running" ? progressAge(activity.progressUpdatedAt, now) : "";
    const liveStatus = activityLiveStatus(activity, status, now);
    const summary = awaitingPermission ? `${toolLabel(activity.name)}等待${awaitingAnswer ? "回答" : "批准"}`
      : child && status !== "running" ? activitySummary(activity, status) : activityGroupLabel(activity, groupPresentationState([{ ...activity, status }], status === "running"));
    const environment = toolEnvironmentLabel(activity.name);
    const detailId = `agent-command-${reactId}-${stateKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
    return <div key={stateKey} data-state={status} className={`agent-command ${status}${expanded ? " expanded" : ""}`}>
      <button type="button" className="agent-command-summary" title={summary} aria-busy={status === "running"} aria-controls={detailId} aria-expanded={expanded} onClick={(event) => { onLayoutChange?.(event.currentTarget); setOpen((previous) => ({ ...previous, [stateKey]: !expanded })); }}>
        <ToolActivityStatus state={status} />
        <span><b className="agent-execution-label">{summary}</b>{liveStatus && <small className="agent-command-live-status">{liveStatus}</small>}</span>
        <ChevronDown size={14} className="agent-disclosure-chevron" />
      </button>
      {expanded && <div id={detailId} className="agent-command-detail" role="region" aria-label={`${summary}详情`}>
        {visibleInput && <><div className="agent-command-shell">{command ? "命令" : "调用参数"}{environment && ` · ${environment}`}</div><pre className="agent-command-input"><code>{command && <span className="agent-command-prompt">$ </span>}{detail}</code></pre></>}
        {((!visibleInput && (status === "running" || status === "pending")) || awaitingPermission) && <div className="agent-command-waiting" role="status">{waitingText}</div>}
        {visibleProgress && <><div className="agent-command-shell agent-command-live-label"><span>实时进度</span>{progressStatus && <small>{progressStatus}</small>}</div><LiveProgressLog progress={activity.progress} /></>}
        {visibleOutput && <section className="agent-command-output-card" aria-label="工具输出">
          <header className="agent-command-output-card-header"><span>工具输出</span>{activity.output.length > 8_000 && <small>长输出</small>}</header>
          <div className="agent-command-output-card-body">
            <pre className="agent-command-output"><code>{outputPreview(activity.output, outputExpanded)}</code></pre>
            {activity.output.length > 8_000 && <button type="button" className="agent-command-output-toggle" onClick={() => setFullOutputOpen((previous) => ({ ...previous, [stateKey]: !outputExpanded }))}>{outputExpanded ? "收起完整输出" : "查看完整输出"}</button>}
          </div>
        </section>}
        {status !== "running" && <div className={`agent-command-result ${status}`}>{status === "pending" ? "等待执行" : status === "error" ? "失败" : status === "cancelled" ? "已取消" : status === "unknown" ? "结果未保存" : status === "interrupted" ? "上次运行中断" : status === "skipped" ? "已跳过" : "成功"}</div>}
      </div>}
    </div>;
  };
  const renderEntry = (entry: (typeof timeline)[number], live = false) => {
        const scope: "live" | "committed" = live ? "live" : "committed";
        const entryKey = timelineDisclosureKey(scope, entry.id);
        if (entry.type === "reasoning") {
          const content = sanitizeAssistantDisplayText(entry.text);
          const active = live;
          return <ReasoningPreview key={entryKey} content={content} label={live ? liveStatusLabel : undefined} active={active} expanded={Boolean(reasoningOpen[entryKey])} onToggle={(anchor) => {
              onLayoutChange?.(anchor);
              setReasoningOpen((previous) => ({ ...previous, [entryKey]: !previous[entryKey] }));
            }} />;
        }
        if (entry.type === "text") return <MarkdownMessage key={entryKey} content={localizeToolIds(sanitizeAssistantDisplayText(entry.text))} streaming={timelineTextIsStreaming(entry, timeline, pending)} onPathMenu={onPathMenu} />;
        if (entry.type === "user") return <div key={entryKey} className="agent-turn-inline-user"><MessageCircle size={14} /><span>{entry.text}</span></div>;
        const disclosureKey = entryKey;
        const groupExpanded = disclosureIsOpen(toolGroupOpen[disclosureKey]);
        const presentedActivities = entry.activities.map(activity => awaitingToolPermission(activity) ? { ...activity, status: "pending" as const } : activity);
        const activeActivities = presentedActivities.filter((activity) => activity.status === "running");
        const groupRunning = !turnFinished && activeActivities.length > 0;
        // The question itself and the composer waiting status already describe
        // this interaction. Keep its tool trace available after it settles.
        if (groupRunning && entry.activities.every(activity => canonicalToolName(activity.name) === "ask_user")) return null;
        const groupError = entry.activities.some((activity) => activity.status === "error");
        const groupState = groupPresentationState(presentedActivities, groupRunning);
        const groupTitle = groupRunning
          ? activeActivities.map(activity => activityGroupLabel(activity, "running")).join("；")
          : entry.activities.some(awaitingToolPermission) ? "等待权限批准" : compactToolGroupLabel(entry.activities.map(activity => activity.name), groupState);
        const groupDetailId = `agent-tool-group-${reactId}-${entryKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
        return <section key={entryKey} data-state={groupState} className={`agent-tool-group${groupExpanded ? " expanded" : ""}${groupRunning ? " running" : ""}${groupError ? " error" : ""}`}>
          <button type="button" className="agent-tool-group-summary" title={groupTitle} aria-busy={groupRunning} aria-controls={groupDetailId} aria-expanded={groupExpanded} onClick={(event) => {
            onLayoutChange?.(event.currentTarget);
            setToolGroupOpen((previous) => ({ ...previous, [disclosureKey]: !groupExpanded }));
          }}>
            <ToolActivityStatus state={groupState} />
            <span><b className="agent-execution-label">{groupTitle}</b></span>
            <ChevronDown size={14} className="agent-disclosure-chevron" />
          </button>
          {groupExpanded && <div id={groupDetailId} className="agent-tool-group-detail" role="region" aria-label={`${groupTitle}详情`}>{entry.activities.map((activity) => renderActivity(activity, true, scope))}</div>}
        </section>;
  };
  const Summary = hasDetails ? "button" : "div";
  // 压缩条按发生顺序插回时间线（与工具组共用同一套事件序号），不再固定挂在详情顶部。
  const orderedTurnItems = () => {
    const items = [
      ...committedTimeline.map(entry => ({ order: timelineEntryOrder(entry), key: `entry:${entry.id}`, node: renderEntry(entry) })),
      ...committedCompactions.map(compaction => ({
        order: Number.isFinite(compaction.order) ? compaction.order : Number.MAX_SAFE_INTEGER,
        key: `compaction:${compaction.id}`,
        node: <ChatCompactionProgress compaction={compaction} />,
      })),
    ];
    return items
      .map((item, index) => ({ ...item, index }))
      .sort((left, right) => (left.order - right.order) || (left.index - right.index))
      .map(item => <React.Fragment key={item.key}>{item.node}</React.Fragment>);
  };
  return <>
    {pending && liveStatusHost && (liveCompaction || liveEntry) && createPortal(liveCompaction
      ? <ChatCompactionProgress key={liveCompaction.id} compaction={liveCompaction} />
      : liveEntry && renderEntry(liveEntry, true), liveStatusHost)}
    <section className={`agent-turn${detailsOpen ? " expanded" : ""}${running ? " is-running" : ""}`}>
    {showSummary && <Summary type={hasDetails ? "button" : undefined} className="agent-turn-summary" aria-controls={hasDetails ? turnDetailsId : undefined} aria-expanded={hasDetails ? detailsOpen : undefined} title={hasDetails ? (detailsOpen ? "只看结果" : "查看完整执行过程") : undefined} onPointerEnter={hasDetails ? onDetailsIntent : undefined} onFocus={hasDetails ? onDetailsIntent : undefined} onClick={hasDetails ? (event) => {
      if (!hasDetails) return;
      const next = !detailsOpen;
      onLayoutChange?.(event.currentTarget);
      setGroupOpen(next);
      onExpandedChange?.(next);
    } : undefined}>
      <span>{running ? `正在处理 ${elapsed || "<1秒"}` : `${paused ? "已暂停" : "已处理"}${elapsed ? ` ${elapsed}` : ""}`}</span>
      {running && <ToolActivityStatus state={permissionEvents.some(event => !permissionDecisions.has(String(event.requestId || event.request_id))) ? "pending" : "running"} />}
      {hasDetails && (detailsOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />)}
    </Summary>}
    {pendingSteerNotice && <div className="agent-turn-note agent-steer-accepted" role="status"><MessageCircle size={14} /><span><b>已收到追加要求：</b>{eventText(pendingSteerNotice, ["message", "text"])}<small className="agent-steer-accepted-hint">当前步骤结束后生效</small></span></div>}
    {/* N6-C：turn 终态时未送达的追加要求——后端已回收（steer_rejected），
        回执就地翻成「未送达」而不是凭空消失，用户知道需要重发。 */}
    {rejectedSteerNotices.map((event, index) => <div key={`steer-rejected-${event.sequence ?? index}`} className="agent-turn-note agent-steer-rejected" role="status"><MessageCircle size={14} /><span><b>追加要求未送达：</b>{eventText(event, ["message", "text"])}<small className="agent-steer-accepted-hint">当前轮次已结束，该消息未被 AI 处理，请重新发送</small></span></div>)}
    {/* Applied steer receipts follow the whole-turn disclosure, independently of reasoning. */}
    {detailsOpen && steerEvents.map((event, index) => <div key={`steer-visible-${event.sequence ?? index}`} className="agent-turn-note agent-steer-applied"><MessageCircle size={14} /><span><b>已应用追加要求：</b>{eventText(event, ["message", "text"])}</span></div>)}
    {processVisible && <div id={turnDetailsId} className="agent-turn-body" role="region" aria-label="AI 执行过程">
      {detailsOpen && <>
        {visibleStatusEvents.map((event, index) => {
        const message = eventText(event, ["message", "status", "text"]);
        const retrying = String(event.status || "") === "provider_retry";
        const active = pending && event === latestEvent;
        return <div key={`status-${event.sequence ?? index}`} className={`agent-turn-note agent-turn-status${retrying ? " retrying" : ""}`}>{active ? <Spinner size={14} /> : retrying ? <CircleHelp size={14} /> : <Check size={14} />}<span>{message}</span></div>;
        })}
        {permissionEvents.map((event, index) => {
        const requestId = String(event.requestId || event.request_id || "");
        const decision = permissionDecisions.get(requestId);
        const rawToolName = String(event.toolName || event.tool_name || "工具");
        const toolName = toolLabel(rawToolName);
        const environment = toolEnvironmentLabel(rawToolName);
        const label = rawToolName === "ask_user"
          ? decision ? "用户询问（已处理）" : pending ? "等待用户回答" : "用户询问（已中断）"
          : decision ? `权限请求：${toolName}（已处理）` : pending ? `等待批准：${toolName}` : `权限请求：${toolName}（已中断）`;
        return <div key={`permission-${requestId || index}`} className={`agent-turn-note${decision ? "" : pending ? "" : " error"}`}><Wrench size={14} /><span>{environment && <><small className="tool-environment-badge">{environment}</small>{" "}</>}{label}</span></div>;
        })}
      </>}
      {/* 正文条目始终渲染；工具、思考等过程条目只在展开时渲染，顺序保持原样。
          展开时压缩条按发生顺序并入同一序列（不再固定在详情顶部）。 */}
      {detailsOpen
        ? orderedTurnItems()
        : committedTimeline.filter(entry => entry.type === "text").map(entry => renderEntry(entry))}
      {detailsOpen && snapshotErrors.map((event, index) => <div key={`snapshot-${index}`} className="agent-turn-note error"><XCircle size={14} /><span>{String(event.message || "文件快照未完成")}</span></div>)}
    </div>}
  </section></>;
}

export const AgentActivityTimeline = memo(AgentActivityTimelineComponent);
