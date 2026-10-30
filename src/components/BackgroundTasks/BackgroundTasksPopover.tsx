/**
 * 后台任务浮层：展示 tasks.rs 任务管理器里的任务（SSH 连接、agent 任务等）
 * 并允许取消还在运行的任务。
 *
 * 数据面只有两个入口：
 * - `list_tasks` / `cancel_task` 命令（platform/runtime.ts 的 listTasks/cancelTask）；
 * - `task:progress` 事件（tasks.rs 每次任务变化都会 publish），收到即刷新，
 *   另配 5s 低频轮询兜底（事件窗口错过时靠它收敛）。
 */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { ListTodo, X } from "lucide-react";
import { cancelTask, formatInvokeError, isTauri, listen, listTasks } from "@/platform/runtime";
import type { Task } from "@/platform/runtime";

const POLL_INTERVAL_MS = 5000;

const STATUS_LABEL: Record<Task["status"], string> = {
  running: "运行中",
  done: "已完成",
  cancelled: "已取消",
  error: "失败",
};

function formatStartedAt(startedAt: number): string {
  const date = new Date(startedAt);
  return Number.isFinite(date.getTime())
    ? date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })
    : "";
}

export const BackgroundTasksPopover: React.FC = () => {
  const [open, setOpen] = useState(false);
  const [tasks, setTasks] = useState<Task[]>([]);
  const rootRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setTasks(await listTasks());
    } catch {
      // 后端不可达时保留上一次列表；不打断 UI。
    }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_INTERVAL_MS);
    let dispose: (() => void) | undefined;
    void listen("task:progress", () => void refresh()).then((unlisten) => {
      dispose = unlisten;
    });
    return () => {
      window.clearInterval(timer);
      dispose?.();
    };
  }, [refresh]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, [open]);

  const runningCount = tasks.filter((task) => task.status === "running").length;

  const handleCancel = useCallback(async (task: Task) => {
    try {
      await cancelTask(task.id);
    } catch (error) {
      // 取消失败只在控制台可见：任务本身很快会被 stale 清扫收敛。
      console.warn("取消后台任务失败", formatInvokeError(error));
    }
  }, []);

  return (
    <div className="background-tasks-root" ref={rootRef}>
      <button
        type="button"
        className="background-tasks-trigger"
        aria-haspopup="dialog"
        aria-expanded={open}
        title="后台任务"
        aria-label={`后台任务${runningCount ? `（${runningCount} 个运行中）` : ""}`}
        onClick={() => {
          setOpen((value) => !value);
          void refresh();
        }}
      >
        <ListTodo size={15} strokeWidth={1.7} />
        {runningCount > 0 && <span className="background-tasks-dot" aria-hidden />}
      </button>
      {open && (
        <div className="background-tasks-popover" role="dialog" aria-label="后台任务">
          <div className="background-tasks-header">
            <span>后台任务</span>
            <button
              type="button"
              className="background-tasks-close"
              aria-label="关闭"
              onClick={() => setOpen(false)}
            >
              <X size={13} />
            </button>
          </div>
          {tasks.length === 0 ? (
            <div className="background-tasks-empty">暂无后台任务</div>
          ) : (
            <ul className="background-tasks-list">
              {tasks.map((task) => (
                <li key={task.id} className="background-tasks-item" data-status={task.status}>
                  <div className="background-tasks-item-main">
                    <span className="background-tasks-kind">{task.kind}</span>
                    <span className="background-tasks-status">{STATUS_LABEL[task.status] ?? task.status}</span>
                    {task.status === "running" && (
                      <button
                        type="button"
                        className="background-tasks-cancel"
                        onClick={() => void handleCancel(task)}
                        aria-label={`取消任务 ${task.target || task.id}`}
                      >
                        取消
                      </button>
                    )}
                  </div>
                  <div className="background-tasks-item-sub" title={task.target || task.id}>
                    {task.target || task.id}
                    {task.message ? ` · ${task.message}` : ""}
                    {task.startedAt ? ` · ${formatStartedAt(task.startedAt)}` : ""}
                  </div>
                  {task.status === "running" && (
                    <div className="background-tasks-progress" aria-hidden>
                      <span style={{ width: `${Math.max(4, Math.min(100, task.progress))}%` }} />
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
};

export default BackgroundTasksPopover;
