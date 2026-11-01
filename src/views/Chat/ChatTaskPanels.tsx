import { useEffect, useRef, useState, type ReactNode } from "react";
import { Trash2 } from "lucide-react";
import { Button } from "rain-ui";
import { ComposerNoticeSlot } from "./ComposerNoticeDock";
import { TaskStepList } from "rain-ui";
import { isWorkspaceChangePath } from "./activityState";
import type { AgentEventPayload } from "@/platform/chatApi";
import type { ComposerFileChanges, ComposerFileEntry, ComposerTaskStep } from "./types";

function asPathList(value: unknown) {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string" && item.trim().length > 0) : [];
}

function asFileChangeEntries(value: unknown): ComposerFileEntry[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const entry = item as Record<string, unknown>;
    const path = typeof entry.path === "string" ? entry.path.trim() : "";
    const status = entry.status;
    if (!path || !isWorkspaceChangePath(path) || (status !== "modified" && status !== "created" && status !== "deleted")) return [];
    return [{
      path,
      status,
      addedLines: Math.max(0, Number(entry.addedLines ?? entry.added_lines ?? 0) || 0),
      removedLines: Math.max(0, Number(entry.removedLines ?? entry.removed_lines ?? 0) || 0),
    }];
  });
}

export function composerFileChanges(event?: AgentEventPayload): ComposerFileChanges | null {
  if (!event || event.type !== "file_changes") return null;
  const created = asPathList(event.created).filter(isWorkspaceChangePath);
  const modified = asPathList(event.modified).filter(isWorkspaceChangePath);
  const deleted = asPathList(event.deleted).filter(isWorkspaceChangePath);
  const entries = asFileChangeEntries(event.entries);
  if (!created.length && !modified.length && !deleted.length && !entries.length) return null;
  return {
    sessionId: String(event.session_id || event.sessionId || ""),
    turnId: String(event.turn_id || event.turnId || ""),
    created,
    modified,
    deleted,
    addedLines: Number(event.added_lines ?? event.addedLines ?? entries.reduce((total, entry) => total + (entry.addedLines || 0), 0)) || 0,
    removedLines: Number(event.removed_lines ?? event.removedLines ?? entries.reduce((total, entry) => total + (entry.removedLines || 0), 0)) || 0,
    entries,
  };
}

export function fileChangeEntries(changes: ComposerFileChanges): ComposerFileEntry[] {
  if (changes.entries?.length) return changes.entries;
  return [
    ...changes.modified.map((path) => ({ path, status: "modified" as const })),
    ...changes.created.map((path) => ({ path, status: "created" as const })),
    ...changes.deleted.map((path) => ({ path, status: "deleted" as const })),
  ];
}

function formatTaskElapsed(startedAt: number, now: number) {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  const minutes = Math.floor(seconds / 60);
  return minutes ? `${minutes}m ${seconds % 60}s` : `${seconds}s`;
}

/** A stopped plan keeps its card so the user can read the outcome. The delete
 *  action only appears after the run is paused; a running plan is stopped from
 *  the composer's own stop button. */
export function ComposerTaskList({ steps, goal, startedAt, finishedAt, active, onOpenPlanner, onCollapse, onDelete, children }: { steps: ComposerTaskStep[]; goal: string; startedAt?: number; finishedAt?: number; active: boolean; onOpenPlanner: (open: boolean) => void; onCollapse: () => void; onDelete: () => void; children?: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  const originRef = useRef(startedAt && startedAt > 0 ? startedAt : Date.now());
  useEffect(() => { if (startedAt && startedAt > 0) originRef.current = startedAt; }, [startedAt]);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  if (!steps.length && !goal) return null;
  const current = steps.find((step) => step.status === "running") || steps.find((step) => step.status === "pending") || steps[steps.length - 1];
  const label = active ? "进行中的目标" : steps.some((step) => step.status === "cancelled") ? "任务已暂停" : steps.length && steps.every((step) => step.status === "done") ? "目标已完成" : "任务已结束";
  const state = active ? "running" : steps.some((step) => step.status === "error") ? "error" : steps.some((step) => step.status === "cancelled") ? "cancelled" : "done";
  const completed = steps.filter((step) => step.status === "done").length;
  const elapsed = formatTaskElapsed(originRef.current, finishedAt || now);
  const paused = !active && steps.some((step) => step.status === "cancelled");
  return <ComposerNoticeSlot id="plan" kind="plan" label="任务编排" summary={goal || current?.title || label} running={active}
    onActivate={onOpenPlanner}
    onCollapse={onCollapse}
    state={state}
    meta={<span className="composer-task-progress-pill">{elapsed} · {completed}/{steps.length}</span>}
    actions={paused
      ? <Button variant="ghost" className="composer-task-delete" icon={<Trash2 size={13} />} aria-label="删除任务编排" title="删除任务编排" onClick={onDelete} />
      : undefined}
    >
    <TaskStepList compact steps={steps.map((step) => ({ status: step.status, content: step.title }))} />
    {children}
  </ComposerNoticeSlot>;
}
