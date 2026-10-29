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
  scrcpy: { label: "启动实时投屏", destination: "Scrcpy 实时投屏", description: "选择 Android 设备后启动 Scrcpy 会话。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }] },
  logcat: { label: "启动 Logcat 采集", destination: "Logcat 实时日志", description: "选择设备、应用日志过滤条件并启动 Logcat 采集。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "query", description: "日志关键字或过滤表达式，可省略" }] },
  shell: { label: "在终端中执行命令", destination: "终端", description: "选择设备并把待执行命令预填到终端，命令仍由终端确认发送。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "command", description: "待确认的 ADB Shell 命令，可省略" }] },
  files: { label: "浏览设备文件", destination: "设备文件与互传", description: "选择 Android 设备并打开指定目录或文件。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "path", description: "设备文件或目录路径，可省略" }] },
  apps: { label: "查看应用包", destination: "应用包管理器", description: "加载设备应用列表并定位指定应用包。", parameters: [{ name: "packageName", description: "应用包名，可省略" }, { name: "serial", description: "目标设备序列号，可省略" }] },
  perf: { label: "准备 Perfetto 采集", destination: "性能监视", description: "选择设备、采集时长和已有的本地输出目录后启动 Perfetto。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "durationSeconds", description: "采集秒数，可省略" }, { name: "destination", description: "已存在的本地输出目录；省略时页面要求选择目录" }] },
  adb: { label: "刷新 ADB 设备", destination: "ADB 设备管理", description: "刷新 ADB 设备连接和诊断状态。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }] },
  archive: { label: "解析 APK / JAR", destination: "APK / JAR 包查看器", description: "使用已知本地路径解析 APK 或 JAR 包结构与清单。", parameters: [{ name: "path", description: "已知的本地 APK 或 JAR 路径；省略时页面要求选择文件" }] },
  aosp: { label: "执行 AOSP 任务", destination: "AOSP 构建与模块编译", description: "验证 AOSP 源码树，并按计划执行指定 lunch 目标和模块编译。", parameters: [{ name: "operation", description: "inspect 或 build；build 会在校验通过后直接启动构建" }, { name: "targetKind", description: "ssh 或 wsl，可省略" }, { name: "targetId", description: "已保存的 SSH 或 WSL 连接 ID，可省略" }, { name: "workspaceRoot", description: "AOSP 源码根目录，可省略" }, { name: "lunchTarget", description: "明确的 lunch 目标，可省略" }, { name: "moduleName", description: "要编译的模块名，可省略" }] },
  capture: { label: "执行屏幕捕获", destination: "设备截图与录屏", description: "选择设备并启动截图或录屏；保存位置由页面确认。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "operation", description: "screenshot 或 record，可省略" }] },
  control: { label: "打开设备控制", destination: "设备快捷控制", description: "选择设备后预填受控的系统调试动作；重启和清除等动作仍需页面确认。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "operation", description: "只读或调试动作名称，可省略" }] },
  simpleperf: { label: "启动 Native CPU 采样", destination: "Native CPU 热点", description: "配置设备、包名、时长和已有输出目录后启动 Simpleperf。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "packageName", description: "应用包名，可省略" }, { name: "durationSeconds", description: "采样秒数，可省略" }, { name: "destination", description: "已存在的本地输出目录；省略时页面要求选择目录" }] },
  "method-profiler": { label: "启动 Java 方法采样", destination: "Java 方法采样", description: "配置目标应用、设备、时长和已有输出目录后采集方法 Trace。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "packageName", description: "应用包名，可省略" }, { name: "durationSeconds", description: "采样秒数，可省略" }, { name: "destination", description: "已存在的本地输出目录；省略时页面要求选择目录" }] },
  "heap-dump": { label: "启动 Java 堆转储", destination: "Java 堆转储", description: "配置目标应用、设备和已有输出目录后导出 HPROF。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "packageName", description: "应用包名，可省略" }, { name: "destination", description: "已存在的本地输出目录；省略时页面要求选择目录" }] },
  benchmark: { label: "执行基准与帧卡顿", destination: "基准与帧卡顿", description: "导入已知 Benchmark 结果或在设备上启动 AndroidX Benchmark。", parameters: [{ name: "serial", description: "目标设备序列号，可省略" }, { name: "operation", description: "import 或 run，可省略" }, { name: "path", description: "已知的 Benchmark 结果路径；import 时必填" }] },
  "native-symbolizer": { label: "执行 Native 符号化", destination: "Native 符号化", description: "使用已知 tombstone 文件和符号目录进行栈回溯。", parameters: [{ name: "tombstonePath", description: "已知的本地 tombstone 文件路径" }, { name: "symbolsPath", description: "已知的本地未剥离符号目录" }] },
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
