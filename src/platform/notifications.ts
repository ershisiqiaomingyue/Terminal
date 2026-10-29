export type NotificationKind = "info" | "success" | "error" | "warning";

export interface GlobalNotification {
  id: string;
  kind: NotificationKind;
  message: string;
  durationMs?: number;
}

export const GLOBAL_NOTIFICATION_EVENT = "android-tool-global-notification";
export const AI_TASK_ACTIVITY_EVENT = "android-tool-ai-task-activity";

export function notify(message: string, kind: NotificationKind = "info", durationMs = 3600): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<GlobalNotification>(GLOBAL_NOTIFICATION_EVENT, {
    detail: { id: `notification-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`, kind, message, durationMs },
  }));
}

export interface AiTaskActivity {
  running: boolean;
  title?: string;
  detail?: string;
  progress?: number | null;
  sessionId?: string;
  actionId?: string;
  status?: "running" | "done" | "error";
}

export function publishAiTaskActivity(activity: AiTaskActivity): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<AiTaskActivity>(AI_TASK_ACTIVITY_EVENT, { detail: activity }));
}

export function notifyDesktopRequired(actionId?: string): void {
  const message = "需要桌面环境；尚未连接设备或执行本地操作。";
  notify(message, "warning");
  if (actionId) publishAiTaskActivity({
    running: false, status: "error", title: "操作未执行", detail: message, actionId,
  });
}
