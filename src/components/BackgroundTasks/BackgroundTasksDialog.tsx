/**
 * 后台任务弹窗：展示 tasks.rs 任务管理器里的任务（SSH 连接、agent 任务等）
 * 并允许取消还在运行的任务。挂在顶栏「设置」菜单的「后台任务…」入口上。
 *
 * 数据面只有两个入口：
 * - `list_tasks` / `cancel_task` 命令（platform/runtime.ts 的 listTasks/cancelTask）；
 * - `task:progress` 事件（tasks.rs 每次任务变化都会 publish），收到即刷新，
 *   另配 5s 低频轮询兜底（事件窗口错过时靠它收敛）。
 */
import React, { useCallback, useEffect, useState } from "react";
import { ListTodo } from "lucide-react";
import { Button, Modal } from "rain-ui";
import { cancelTask, formatInvokeError, isTauri, listen, listTasks } from "@/platform/runtime";
import type { Task } from "@/platform/runtime";
import "./background-tasks.css";

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

export const BackgroundTasksDialog: React.FC<{ open: boolean; onClose: () => void }> = ({ open, onClose }) => {
  const [tasks, setTasks] = useState<Task[]>([]);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setTasks(await listTasks());
    } catch {
      // 后端不可达时保留上一次列表；不打断 UI。
    }
  }, []);

  useEffect(() => {
    if (!open || !isTauri()) return;
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
  }, [open, refresh]);

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
    <Modal
      open={open}
      onClose={onClose}
      title="后台任务"
      icon={<ListTodo size={16} />}
      description={runningCount ? `${runningCount} 个任务运行中` : "暂无运行中的任务"}
      footer={<Button variant="secondary" onClick={onClose}>关闭</Button>}
    >
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
                  <Button
                    type="button"
                    variant="ghost"
                    className="background-tasks-cancel btn-danger-hover"
                    onClick={() => void handleCancel(task)}
                    aria-label={`取消任务 ${task.target || task.id}`}
                  >
                    取消
                  </Button>
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
    </Modal>
  );
};

export default BackgroundTasksDialog;
