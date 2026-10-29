import { useEffect, useRef, useState } from "react";
import { ToolActivityStatus } from "./ToolActivityStatus";
import { NoticeBackgroundTask } from "@/components/ui/NoticeContent";
import { invoke, isTauri, listen } from "@/platform/runtime";
import { ComposerNoticeSlot } from "./ComposerNoticeDock";
import { visibleBackgroundTasks, type BackgroundTaskTurn } from "./backgroundTaskVisibility";

export interface BackgroundTask {
  id: string;
  sessionId: string;
  turnId?: string;
  command: string;
  title?: string;
  status: string;
  exitCode: number | null;
  logTail: string;
  error: string | null;
  logPath: string;
  startedAt: number;
  finishedAt?: number | null;
}

const statusLabels: Record<string, string> = {
  running: "后台运行中", done: "已完成", error: "执行失败", cancelled: "已取消",
  timeout: "监测超时", interrupted: "监测中断",
};

function backgroundTitle(task: BackgroundTask) {
  if (task.title?.trim() && /[\u4e00-\u9fff]/.test(task.title)) return task.title.trim();
  if (/\bbitbake\b/i.test(task.command)) return "Yocto 编译任务";
  if (/\b(lunch|ninja)\b/i.test(task.command)) return "Android 编译任务";
  if (/gradle/i.test(task.command)) return "Gradle 构建任务";
  return "后台执行任务";
}

function backgroundSummary(task: BackgroundTask) {
  const target = task.command.match(/\bbitbake\s+(?!-)([\w.+-]+)/i)?.[1];
  if (/\bbitbake\b/i.test(task.command)) return target ? `构建 ${target}` : "构建 Yocto 目标";
  const android = task.command.match(/\bninja\s+(?!-)([\w.+-]+)/i)?.[1];
  if (/\b(lunch|ninja)\b/i.test(task.command)) return android ? `编译 ${android}` : "编译 Android 目标";
  const gradle = task.command.match(/\bgradlew?\s+([\w:-]+)/i)?.[1];
  if (/gradle/i.test(task.command)) return gradle ? `构建 ${gradle}` : "执行 Gradle 构建";
  return task.title?.trim() && backgroundTitle(task) === task.title.trim() ? "执行任务并监测输出" : "执行后台命令并监测输出";
}

function taskElapsed(startedAt: number, now: number) {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds % 3600 / 60);
  return hours ? `${hours}小时${minutes}分${seconds % 60}秒` : minutes ? `${minutes}分${seconds % 60}秒` : `${seconds}秒`;
}

function backgroundState(status: string): "running" | "done" | "error" | "cancelled" | "unknown" {
  if (status === "running") return "running";
  if (status === "done") return "done";
  if (status === "cancelled") return "cancelled";
  if (status === "error" || status === "timeout" || status === "interrupted") return "error";
  return "unknown";
}

function BackgroundTaskStatus({ task }: { task: BackgroundTask }) {
  const [now, setNow] = useState(Date.now);
  const running = task.status === "running";
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running, task.startedAt]);
  return <small className="ui-notice-detail" title={statusLabels[task.status]}>{running ? taskElapsed(task.startedAt, now) : statusLabels[task.status] || "执行未完成"}</small>;
}

/** Shared production presentation; debug injects task data into this adapter. */
export function BackgroundTaskNotices({ tasks, error, stopping = [], onStop }: {
  tasks: BackgroundTask[]; error?: string; stopping?: string[]; onStop: (task: BackgroundTask) => void;
}) {
  const running = tasks.filter((task) => task.status === "running").length;
  const attention = tasks.some((task) => task.status !== "running" && task.status !== "done");
  const current = tasks.find((task) => task.status === "running") || tasks[tasks.length - 1];
  if (!tasks.length && !error) return null;
  // No numeric counter: the shared status ring already communicates progress,
  // and the count duplicated the "· N 项" text in the summary.
  return <ComposerNoticeSlot id="background" kind="background" label="后台任务" running={running > 0}
    state={current ? backgroundState(current.status) : "unknown"}
    badge={<ToolActivityStatus state={current ? backgroundState(current.status) : "unknown"} />}
    summary={current ? `${running ? "正在" : attention ? "需检查" : "已完成"}${backgroundTitle(current)}${tasks.length > 1 ? ` · ${tasks.length} 项` : ""}` : "后台任务加载失败"}>
    <div className="ui-notice-body-stack">
      {error && <p className="ui-notice-error" role="alert">{error}</p>}
      {tasks.map((task) => <NoticeBackgroundTask key={task.id} title={backgroundTitle(task)} log={task.logTail}
        running={task.status === "running"} stopping={stopping.includes(task.id)} onStop={() => onStop(task)} error={task.error}
        meta={<BackgroundTaskStatus task={task} />}>
        <details>
          <summary>任务详情 · {backgroundSummary(task)}</summary>
          <div>{statusLabels[task.status] || task.status}{task.finishedAt != null && ` · 执行了 ${taskElapsed(task.startedAt, task.finishedAt)}`}{task.exitCode != null && ` · 退出码 ${task.exitCode}`}</div>
          <pre className="ui-notice-code" aria-label="后台任务完整命令" tabIndex={0}><code>{task.command}</code></pre>
          <small title={task.logPath}>任务 {task.id} · 保留最近日志（最多 16 MB）</small>
        </details>
      </NoticeBackgroundTask>)}
    </div>
  </ComposerNoticeSlot>;
}

export function BackgroundTasks({ sessionId, turn }: { sessionId: string; turn?: BackgroundTaskTurn }) {
  const [allTasks, setTasks] = useState<BackgroundTask[]>([]);
  const tasks = visibleBackgroundTasks(allTasks, sessionId, turn);
  const [error, setError] = useState("");
  const [stopping, setStopping] = useState<string[]>([]);
  const revision = useRef(0);
  useEffect(() => {
    const current = ++revision.current;
    setTasks([]);
    setError("");
    setStopping([]);
    if (!sessionId || !isTauri()) return;
    let disposed = false;
    let inFlight = false;
    let refreshPending = false;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      if (disposed) return;
      if (inFlight) { refreshPending = true; return; }
      inFlight = true;
      try {
        const next = await invoke<BackgroundTask[]>("list_background_shells", { sessionId });
        if (!disposed) { setTasks(next); setError(""); }
      } catch (cause) {
        if (!disposed) setError(String(cause));
      } finally {
        inFlight = false;
        if (refreshPending && !disposed) { refreshPending = false; void refresh(); }
      }
    };
    void listen<{ sessionId: string }>("agent:background_task", (event) => {
      if (event.sessionId === sessionId) void refresh();
    }).then((off) => { if (disposed) off(); else { unlisten = off; void refresh(); } }).catch(() => { void refresh(); });
    return () => { disposed = true; unlisten?.(); if (revision.current === current) revision.current++; };
  }, [sessionId]);

  return <BackgroundTaskNotices tasks={tasks} error={error} stopping={stopping} onStop={async (task) => {
    const current = revision.current;
    setStopping((ids) => [...ids, task.id]);
    try { await invoke("cancel_background_shell", { sessionId, taskId: task.id }); }
    catch (cause) { if (revision.current === current) { setError(String(cause)); setStopping((ids) => ids.filter((id) => id !== task.id)); } }
  }} />;
}
