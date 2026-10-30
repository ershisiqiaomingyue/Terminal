import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { reportFrontendError } from "./frontendHealth.ts";

export function isTauri(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const started = performance.now();
  try { return await tauriInvoke<T>(command, args); }
  catch (error) {
    // Arguments may contain prompts, credentials or file contents. Log only the operation and failure.
    if (!command.startsWith("report_")) reportFrontendError("ipc_failure", error, { operation: command, durationMs: Math.round(performance.now() - started) });
    throw error;
  }
}

export async function listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  const unlisten = await tauriListen<T>(event, (message) => handler(message.payload));
  return () => {
    unlisten();
  };
}

export interface DialogFilter {
  name: string;
  extensions: string[];
}

async function dialogPlugin() {
  return import("@tauri-apps/plugin-dialog");
}

export async function openFile(title: string, filters?: DialogFilter[]): Promise<string | null> {
  const dialog = await dialogPlugin();
  const selected = await dialog.open({ title, multiple: false, directory: false, filters });
  return typeof selected === "string" ? selected : null;
}

export async function openDirectory(title: string): Promise<string | null> {
  const dialog = await dialogPlugin();
  const selected = await dialog.open({ title, multiple: false, directory: true });
  return typeof selected === "string" ? selected : null;
}

export async function saveFile(title: string, defaultPath: string, filters?: DialogFilter[]): Promise<string | null> {
  const dialog = await dialogPlugin();
  const selected = await dialog.save({ title, defaultPath, filters });
  return selected ?? null;
}

export interface Task {
  id: string;
  kind: string;
  target: string;
  status: string;
  progress: number;
  message: string;
  startedAt: number;
  finishedAt?: number | null;
  artifactPath?: string | null;
  error?: string | null;
  /** 提交时刻的所属 turn（后端权威下发）；旧任务缺失。 */
  turnId?: string | null;
}

export async function listTasks(): Promise<Task[]> {
  return invoke<Task[]>("list_tasks");
}

export async function cancelTask(id: string): Promise<void> {
  await invoke("cancel_task", { id });
}

export async function waitForTask(id: string, onUpdate?: (task: Task) => void, timeoutMs = 30 * 60 * 1000): Promise<Task> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const tasks = await listTasks();
    const task = tasks.find((item) => item.id === id);
    if (task) {
      onUpdate?.(task);
      if (task.status !== "running" && task.status !== "queued") return task;
    }
    await new Promise((resolve) => window.setTimeout(resolve, 800));
  }
  throw new Error("任务等待超时。");
}

// 归一化逻辑在 ./errorText.ts（不依赖 Tauri，可单测），这里再导出一次给全仓使用。
export { formatErrorText as formatInvokeError } from "./errorText.ts";
