import type { SupportedToolKey } from "@/views/ToolWindows";
import { captureToolWindowState, restoreToolWindowState } from "./toolWindowState";
import { captureGlobalEnvironment, restoreGlobalEnvironment } from "./globalEnvironment";
import { invoke, isTauri, listen } from "./runtime";

export interface ToolWindowInfo { label: string; toolKey: SupportedToolKey; title: string; state: Record<string, unknown> }
export const isToolWindow = () => new URLSearchParams(window.location.search).has("toolWindow");
export async function initializeToolWindow(): Promise<ToolWindowInfo | null> {
  if (!isToolWindow() || !isTauri()) return null;
  const info = await invoke<ToolWindowInfo>("get_tool_window");
  restoreToolWindowState(info.state);
  restoreGlobalEnvironment(info.state.environment);
  return info;
}

export async function detachToolWindow(toolKey: SupportedToolKey, title: string, point?: { x: number; y: number }): Promise<void> {
  const state = { ...captureToolWindowState(toolKey), environment: captureGlobalEnvironment() };
  return createToolWindow(toolKey, title, state, point);
}

export async function openToolWindow(toolKey: SupportedToolKey, title: string): Promise<void> {
  return createToolWindow(toolKey, title, { environment: captureGlobalEnvironment() });
}

export async function openToolWindowWithState(toolKey: SupportedToolKey, title: string, state: Record<string, unknown>): Promise<void> {
  return createToolWindow(toolKey, title, { ...state, environment: captureGlobalEnvironment() });
}

async function createToolWindow(toolKey: SupportedToolKey, title: string, state: Record<string, unknown>, point?: { x: number; y: number }): Promise<void> {
  if (!isTauri()) throw new Error("独立工具窗口仅在桌面应用中可用。");
  let label: string | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let complete: (label: string) => void = () => undefined;
  const readyLabels = new Set<string>();
  const ready = new Promise<string>((resolve) => { complete = resolve; });
  const stop = await listen<{ label: string }>("tool-window:ready", (event) => {
    readyLabels.add(event.label);
    if (event.label === label) complete(event.label);
  });
  try {
    label = await invoke<string>("create_tool_window", { toolKey, title, state, position: point || null });
    if (readyLabels.has(label)) complete(label);
    await Promise.race([ready, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("独立窗口未能完成加载，原标签已保留。")), 20_000); })]);
  } catch (error) {
    if (label && await invoke<boolean>("abort_tool_window", { label }).catch(() => false)) return;
    throw error;
  } finally { stop(); if (timer) clearTimeout(timer); }
}
