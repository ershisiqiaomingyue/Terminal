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

export interface AdbDevice {
  serial: string;
  state: string;
  model?: string;
  product?: string;
  device?: string;
  transportId?: string;
}

export function deviceLabel(device: AdbDevice): string {
  return device.model ? `${device.model} (${device.serial})` : device.serial;
}

/** Select 里只显示一个名称。括号中的序列号留给详情，避免选项被裁切。 */
export function deviceOptionLabel(device: AdbDevice): string {
  return device.model || device.product || device.serial;
}

export async function listAdbDevices(): Promise<AdbDevice[]> {
  return invoke<AdbDevice[]>("list_adb_devices");
}

/**
 * 被动刷新设备列表：ADB 服务没在跑就直接返回空数组，不会顺手把 adb 启起来。
 * 界面上的自动轮询（进页面、定时器）用这个；用户手动点「刷新」才用
 * {@link listAdbDevices}。
 */
export async function listAdbDevicesPassive(): Promise<AdbDevice[]> {
  return invoke<AdbDevice[]>("list_adb_devices_passive");
}

export interface AndroidTask {
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
  /** P5-E：提交时刻的所属 turn（后端权威下发）；旧任务缺失。 */
  turnId?: string | null;
}

export async function listAndroidTasks(): Promise<AndroidTask[]> {
  return invoke<AndroidTask[]>("list_android_tasks");
}

export async function cancelAndroidTask(id: string): Promise<void> {
  await invoke("cancel_android_task", { id });
}

export async function waitForTask(id: string, onUpdate?: (task: AndroidTask) => void, timeoutMs = 30 * 60 * 1000): Promise<AndroidTask> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const tasks = await listAndroidTasks();
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
