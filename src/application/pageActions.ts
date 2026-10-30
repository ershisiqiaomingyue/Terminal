import type { SupportedToolKey } from "@/views/ToolWindows";

export const APP_PAGE_ACTION_EVENT = "android-tool-page-action";

type ActionParameter = {
  name: string;
  description: string;
};

export type AppPageTarget =
  | { kind: "home" }
  | { kind: "chat" }
  | { kind: "components" }
  | { kind: "settings"; section: "general" | "agent" | "ssh" | "wsl" | "skills" | "about" }
  | { kind: "tool"; toolKey: SupportedToolKey };

export type ApplicationAction = {
  id: string;
  label: string;
  destination: string;
  description: string;
  target: AppPageTarget;
  parameters: readonly ActionParameter[];
};

export type ApplicationActionInvocation = {
  actionId: string;
  summary: string;
  parameters: Record<string, string>;
};

const TOOL_ACTIONS: Record<SupportedToolKey, Omit<ApplicationAction, "id" | "target">> = {
  "code-viewer": { label: "打开代码查看", destination: "代码查看", description: "浏览本地工程或全局 SSH 连接的远程目录树，预览代码和 Markdown。", parameters: [] },
  shell: { label: "在终端中执行命令", destination: "终端", description: "把待执行命令预填到终端，命令仍由终端确认发送。", parameters: [{ name: "command", description: "待确认的命令，可省略" }] },
};

const TOOL_KEYS = Object.keys(TOOL_ACTIONS) as SupportedToolKey[];

export const APP_ACTION_CATALOG: readonly ApplicationAction[] = [
  { id: "open.home", label: "打开主页", destination: "主页", description: "返回工具主页。", target: { kind: "home" }, parameters: [] },
  { id: "open.chat", label: "在对话中继续", destination: "对话", description: "把任务交给 Agent 对话继续分析或执行。", target: { kind: "chat" }, parameters: [{ name: "draft", description: "预填给对话的任务说明，可省略" }] },
  { id: "open.components", label: "打开组件库", destination: "组件库", description: "查看应用组件与设计规范。", target: { kind: "components" }, parameters: [] },
  { id: "open.settings.ssh", label: "配置 SSH", destination: "设置 / SSH", description: "新增或编辑 SSH 连接。", target: { kind: "settings", section: "ssh" }, parameters: [] },
  { id: "open.settings.agent", label: "配置 AI Provider", destination: "设置 / Agent", description: "配置模型供应商和默认模型。", target: { kind: "settings", section: "agent" }, parameters: [] },
  ...TOOL_KEYS.map((toolKey) => ({
    ...TOOL_ACTIONS[toolKey],
    id: `open.tool.${toolKey}`,
    target: { kind: "tool" as const, toolKey },
  })),
];

const ACTION_BY_ID = new Map(APP_ACTION_CATALOG.map((action) => [action.id, action]));
const activePageActionListeners = new Map<string, number>();
const pendingPageActions = new Map<string, ApplicationActionInvocation[]>();
const MAX_PENDING_PAGE_ACTIONS = 32;

export function resolveApplicationAction(actionId: string): ApplicationAction | undefined {
  return ACTION_BY_ID.get(actionId);
}

export function normalizeActionParameters(action: ApplicationAction, value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const allowed = new Set(action.parameters.map((parameter) => parameter.name));
  const result: Record<string, string> = {};
  for (const [key, raw] of Object.entries(value)) {
    if (!allowed.has(key) || typeof raw !== "string") continue;
    const trimmed = raw.trim();
    if (trimmed) result[key] = trimmed.slice(0, 1000);
  }
  return result;
}

export function applicationActionCatalogForPrompt(): string {
  return APP_ACTION_CATALOG.map((action) => JSON.stringify({
    actionId: action.id,
    destination: action.destination,
    description: action.description,
    parameters: action.parameters,
  })).join("\n");
}

export function dispatchApplicationPageAction(invocation: ApplicationActionInvocation): void {
  if (!activePageActionListeners.get(invocation.actionId)) {
    // Lazy mounting must not overwrite an earlier handoff to the same destination.
    const pending = pendingPageActions.get(invocation.actionId) || [];
    pending.push(invocation);
    if (pending.length > MAX_PENDING_PAGE_ACTIONS) pending.splice(0, pending.length - MAX_PENDING_PAGE_ACTIONS);
    pendingPageActions.set(invocation.actionId, pending);
    return;
  }
  window.dispatchEvent(new CustomEvent<ApplicationActionInvocation>(APP_PAGE_ACTION_EVENT, { detail: invocation }));
}

export function subscribeApplicationPageAction(actionId: string): ApplicationActionInvocation[] {
  activePageActionListeners.set(actionId, (activePageActionListeners.get(actionId) || 0) + 1);
  const pending = pendingPageActions.get(actionId);
  pendingPageActions.delete(actionId);
  return pending || [];
}

export function unsubscribeApplicationPageAction(actionId: string): void {
  const next = (activePageActionListeners.get(actionId) || 1) - 1;
  if (next > 0) activePageActionListeners.set(actionId, next);
  else activePageActionListeners.delete(actionId);
}
