import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bot, BrainCircuit, Check, ChevronRight, Circle, CircleDot, CornerDownLeft, GripHorizontal, History, MapPin, Minus, Play, Route, Search, Send, ShieldCheck, Sparkles, Trash2, X } from "lucide-react";
import { Badge, Button, Input } from "@/components/ui";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { AgentModelPicker } from "@/components/AgentModelPicker/AgentModelPicker";
import { ToolIcon } from "@/components/ToolIcon/ToolIcon";
import {
  applicationActionCatalogForPrompt,
  normalizeActionParameters,
  resolveApplicationAction,
  type ApplicationActionInvocation,
} from "@/application/pageActions";
import { listAgentProviders, listSshConnections, listWslProfiles, type AgentProvider } from "@/platform/api";
import { formatInvokeError, invoke, isTauri, listAdbDevices } from "@/platform/runtime";
import { homeToolStatus, homeToolStatusLabel, homeToolStatusVariant, type HomeTool } from "@/views/Home";
import type { SupportedToolKey } from "@/views/ToolWindows";
import { AI_TASK_ACTIVITY_EVENT, notify, publishAiTaskActivity, type AiTaskActivity } from "@/platform/notifications";
import "./quick-tool-palette.css";

interface QuickToolPaletteProps {
  open: boolean;
  tools: HomeTool[];
  onClose: () => void;
  onOpenTool: (tool: SupportedToolKey) => void;
  onExecuteAction: (action: ApplicationActionInvocation) => void | Promise<void>;
  onMinimize: () => void;
  variant?: "tools" | "ai";
  initialQuery?: string;
  onOpenAi?: () => void;
  externalActivity?: AiTaskActivity;
  allowAi?: boolean;
}

interface ToolUsage {
  count: number;
  lastUsedAt: number;
}

interface ToolUsageStore {
  version: 1;
  tools: Record<string, ToolUsage>;
}

type PaletteItem =
  | { kind: "ask-ai" }
  | { kind: "tool"; tool: HomeTool };

type PaletteMode = "tools" | "ai";

interface AgentResponse {
  text?: string;
}

type PlanStep = {
  title: string;
  detail: string;
};

type TaskPlan = {
  summary: string;
  steps: PlanStep[];
  execution: ApplicationActionInvocation | null;
  validation: PlanValidation;
};

type TaskRunStep = {
  id: string;
  title: string;
  detail: string;
  kind: "analysis" | "validation" | "execution";
  status: "pending" | "running" | "done" | "error";
  durationMs?: number;
};

type TaskRuntimeActivity = {
  title: string;
  detail: string;
  status: "idle" | "running" | "done" | "error";
  progress: number | null;
};

type PlanValidation = {
  status: "ready" | "needs-input" | "not-applicable";
  message: string;
};

type PlanValidationResult = PlanValidation & {
  resolvedParameters?: Record<string, string>;
};

type AospWorkspaceProbe = {
  linux: boolean;
  bashAvailable: boolean;
  envsetupPath?: string;
  soongPath?: string;
  manifestPath?: string;
};

type TaskHistoryEntry = {
  id: string;
  task: string;
  summary: string;
  destination: string;
  actionId?: string;
  status: "planned" | "executed";
  createdAt: number;
  plan?: TaskPlan;
  runSteps?: TaskRunStep[];
  runtimeActivity?: TaskRuntimeActivity;
  activePlanningStage?: number;
};

const PIPELINE_STAGES = [
  { label: "理解目标", detail: "提取任务范围与已有条件", icon: BrainCircuit },
  { label: "识别能力", detail: "匹配模型与应用公开动作", icon: ShieldCheck },
  { label: "校验工具", detail: "确认设备、连接和配置", icon: CircleDot },
  { label: "准备执行", detail: "整理结果，等待你确认进入页面", icon: Route },
] as const;

const USAGE_STORAGE_KEY = "android-tool.quick-tool-usage.v1";
const CHAT_SELECTION_STORAGE_KEY = "android-tool.chat.selection.v1";
const TASK_HISTORY_STORAGE_KEY = "android-tool.ai-task-history.v1";
const AOSP_WORKSPACE_SELECTION_STORAGE_KEY = "android-tool.aosp-workspace-selection.v1";

type SavedAospWorkspace = {
  targetKind: "ssh" | "wsl";
  targetId: string;
  workspaceRoot: string;
};

type AospTargetCandidate = SavedAospWorkspace & {
  name: string;
  defaultRoot?: string | null;
};

function matchesText(value: string, query: string) {
  return value.toLocaleLowerCase("zh-CN").includes(query.toLocaleLowerCase("zh-CN").trim());
}

function matchesTool(tool: HomeTool, query: string) {
  return matchesText(`${tool.title} ${tool.subtitle} ${tool.desc} ${tool.tag}`, query);
}

function loadToolUsage(): ToolUsageStore {
  try {
    const value = JSON.parse(window.localStorage.getItem(USAGE_STORAGE_KEY) || "{}") as Partial<ToolUsageStore>;
    if (value.version !== 1 || !value.tools || typeof value.tools !== "object") return { version: 1, tools: {} };
    const tools = Object.fromEntries(Object.entries(value.tools).flatMap(([id, entry]) => {
      if (!entry || typeof entry !== "object") return [];
      const count = Number((entry as Partial<ToolUsage>).count);
      const lastUsedAt = Number((entry as Partial<ToolUsage>).lastUsedAt);
      return Number.isFinite(count) && count > 0 && Number.isFinite(lastUsedAt)
        ? [[id, { count, lastUsedAt }]]
        : [];
    }));
    return { version: 1, tools };
  } catch {
    return { version: 1, tools: {} };
  }
}

function saveToolUsage(value: ToolUsageStore) {
  try {
    window.localStorage.setItem(USAGE_STORAGE_KEY, JSON.stringify(value));
  } catch {
    // Usage ordering is a convenience and must not block opening a tool.
  }
}

function loadTaskHistory(): TaskHistoryEntry[] {
  try {
    const value = JSON.parse(window.localStorage.getItem(TASK_HISTORY_STORAGE_KEY) || "[]") as unknown;
    if (!Array.isArray(value)) return [];
    return value.filter((entry): entry is TaskHistoryEntry => Boolean(entry && typeof entry === "object" && typeof (entry as TaskHistoryEntry).task === "string" && typeof (entry as TaskHistoryEntry).createdAt === "number")).slice(0, 20);
  } catch {
    return [];
  }
}

function saveTaskHistory(value: TaskHistoryEntry[]) {
  try {
    window.localStorage.setItem(TASK_HISTORY_STORAGE_KEY, JSON.stringify(value.slice(0, 20)));
  } catch {
    // History is best effort and must not block task routing.
  }
}

function selectedAgent() {
  try {
    const value = JSON.parse(window.localStorage.getItem(CHAT_SELECTION_STORAGE_KEY) || "{}") as {
      providerId?: unknown;
      modelId?: unknown;
    };
    return {
      providerId: typeof value.providerId === "string" ? value.providerId : "",
      modelId: typeof value.modelId === "string" ? value.modelId : "",
    };
  } catch {
    return { providerId: "", modelId: "" };
  }
}

function saveSelectedAgent(providerId: string, modelId: string) {
  try {
    let previous: Record<string, unknown> = {};
    try {
      const stored = JSON.parse(window.localStorage.getItem(CHAT_SELECTION_STORAGE_KEY) || "{}");
      if (stored && typeof stored === "object" && !Array.isArray(stored)) {
        previous = stored as Record<string, unknown>;
      }
    } catch {
      // A malformed previous selection should not block saving the new model.
    }
    window.localStorage.setItem(CHAT_SELECTION_STORAGE_KEY, JSON.stringify({
      ...previous,
      providerId,
      modelId,
    }));
  } catch {
    // The planner can still use the current selection when storage is unavailable.
  }
}

function loadSavedAospWorkspace(): SavedAospWorkspace | null {
  try {
    const value = JSON.parse(window.localStorage.getItem(AOSP_WORKSPACE_SELECTION_STORAGE_KEY) || "null") as Partial<SavedAospWorkspace> | null;
    if (!value || (value.targetKind !== "ssh" && value.targetKind !== "wsl") || typeof value.targetId !== "string" || typeof value.workspaceRoot !== "string") {
      return null;
    }
    const targetId = value.targetId.trim();
    const workspaceRoot = value.workspaceRoot.trim();
    return targetId && workspaceRoot.startsWith("/") ? { targetKind: value.targetKind, targetId, workspaceRoot } : null;
  } catch {
    return null;
  }
}

function saveAospWorkspace(workspace: SavedAospWorkspace) {
  try {
    window.localStorage.setItem(AOSP_WORKSPACE_SELECTION_STORAGE_KEY, JSON.stringify(workspace));
  } catch {
    // Workspace recall is a convenience; a storage failure must not block a build.
  }
}

function requireText(value: unknown, field: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`AI 返回的任务计划缺少${field}。`);
  return value.trim().slice(0, 2000);
}

function parseTaskPlan(text: string): TaskPlan {
  let payload: unknown;
  try {
    const trimmed = text.trim().replace(/^```(?:json)?\s*/i, "").replace(/\s*```$/i, "");
    try {
      payload = JSON.parse(trimmed);
    } catch {
      // Some compatible providers add a short sentence around the JSON even
      // when asked not to. Recover only a balanced top-level object; schema
      // validation below still rejects anything outside our action catalog.
      const start = trimmed.indexOf("{");
      const end = trimmed.lastIndexOf("}");
      if (start < 0 || end <= start) throw new Error("no JSON object");
      payload = JSON.parse(trimmed.slice(start, end + 1));
    }
  } catch {
    throw new Error("AI 没有按任务调度协议返回 JSON，请重新分析。");
  }
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error("AI 返回的任务计划格式无效，请重新分析。");
  }
  const value = payload as Record<string, unknown>;
  const rawSteps = Array.isArray(value.steps) ? value.steps.slice(0, 5) : [];
  if (!rawSteps.length) throw new Error("AI 返回的任务计划没有步骤，请重新分析。");
  const steps = rawSteps.map((item, index) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) throw new Error(`AI 返回的第 ${index + 1} 个步骤无效。`);
    const step = item as Record<string, unknown>;
    return { title: requireText(step.title, "步骤标题"), detail: requireText(step.detail, "步骤说明") };
  });

  let execution: ApplicationActionInvocation | null = null;
  if (value.execution !== null && value.execution !== undefined) {
    if (!value.execution || typeof value.execution !== "object" || Array.isArray(value.execution)) {
      throw new Error("AI 返回的执行位置无效，请重新分析。");
    }
    const rawExecution = value.execution as Record<string, unknown>;
    const actionId = requireText(rawExecution.actionId, "执行动作");
    const action = resolveApplicationAction(actionId);
    if (!action) throw new Error("AI 选择了当前应用未公开的执行动作，请重新分析。");
    execution = {
      actionId: action.id,
      summary: typeof rawExecution.summary === "string" && rawExecution.summary.trim()
        ? rawExecution.summary.trim().slice(0, 2000)
        : action.description,
      parameters: normalizeActionParameters(action, rawExecution.parameters),
    };
  }
  return {
    summary: requireText(value.summary, "任务理解"),
    steps,
    execution,
    validation: execution
      ? { status: "not-applicable", message: "等待校验执行前置条件。" }
      : { status: "not-applicable", message: "当前计划没有可执行的页面动作。" },
  };
}

async function validateTaskPlan(plan: TaskPlan): Promise<PlanValidationResult> {
  if (!plan.execution) return plan.validation;
  if (!isTauri()) return { status: "not-applicable", message: "浏览器预览不读取桌面端设备和连接配置。" };

  const action = resolveApplicationAction(plan.execution.actionId);
  if (!action) return { status: "needs-input", message: "执行动作已失效，请重新分析。" };
  const parameters = plan.execution.parameters;
  const deviceActions = new Set([
    "open.tool.scrcpy", "open.tool.logcat", "open.tool.shell", "open.tool.files", "open.tool.apps",
    "open.tool.perf", "open.tool.adb", "open.tool.capture", "open.tool.control", "open.tool.simpleperf",
    "open.tool.method-profiler", "open.tool.heap-dump", "open.tool.benchmark",
  ]);

  if (deviceActions.has(action.id)) {
    const devices = await listAdbDevices();
    const online = devices.filter((device) => device.state === "device");
    if (!online.length) return { status: "needs-input", message: "未检测到在线 ADB 设备；已保留任务参数，进入页面后可刷新或连接设备。" };
    if (parameters.serial && !online.some((device) => device.serial === parameters.serial)) {
      return { status: "needs-input", message: `未找到指定设备 ${parameters.serial}；进入页面后请选择在线设备。` };
    }
    if (action.id === "open.tool.benchmark" && parameters.operation === "import" && !parameters.path) {
      return { status: "needs-input", message: "导入 Benchmark 结果需要已知的本地 JSON 或 TXT 路径。" };
    }
    if (new Set(["open.tool.perf", "open.tool.simpleperf", "open.tool.method-profiler", "open.tool.heap-dump"]).has(action.id) && !parameters.destination) {
      return { status: "needs-input", message: "缺少本地输出目录；执行时会要求选择目录，然后才能启动采集任务。" };
    }
    return { status: "ready", message: `已检测到 ${online.length} 台在线 ADB 设备，页面可继续准备任务。` };
  }

  if (action.id === "open.tool.aosp") {
    const [wslProfiles, sshConnections] = await Promise.all([listWslProfiles(), listSshConnections()]);
    const requestedKind = parameters.targetKind === "ssh" || parameters.targetKind === "wsl" ? parameters.targetKind : null;
    const requestedId = parameters.targetId?.trim() || "";
    const configuredTargets: AospTargetCandidate[] = [
      ...wslProfiles.map((target) => ({ targetKind: "wsl" as const, targetId: target.id, name: target.name, defaultRoot: target.defaultRoot, workspaceRoot: "" })),
      ...sshConnections.map((target) => ({ targetKind: "ssh" as const, targetId: target.id, name: target.name, defaultRoot: target.defaultRoot, workspaceRoot: "" })),
    ];
    if (!configuredTargets.length) {
      return { status: "needs-input", message: "尚未保存 WSL 或 SSH Linux 连接；请先在设置中添加一个可用目标。" };
    }

    let candidates = configuredTargets.filter((target) => (!requestedKind || target.targetKind === requestedKind) && (!requestedId || target.targetId === requestedId));
    if (!candidates.length) {
      return { status: "needs-input", message: requestedId ? "未找到计划中指定的 Linux 连接，请重新分析或在设置中检查连接。" : "未找到所选类型的 Linux 连接，请在设置中检查 WSL 或 SSH 配置。" };
    }

    const savedWorkspace = loadSavedAospWorkspace();
    candidates = candidates.sort((left, right) => {
      const leftSaved = savedWorkspace?.targetKind === left.targetKind && savedWorkspace.targetId === left.targetId ? 0 : 1;
      const rightSaved = savedWorkspace?.targetKind === right.targetKind && savedWorkspace.targetId === right.targetId ? 0 : 1;
      return leftSaved - rightSaved;
    });

    const failures: string[] = [];
    const ambiguous: string[] = [];
    for (const target of candidates) {
      const roots = new Set<string>();
      const explicitRoot = parameters.workspaceRoot?.trim();
      if (explicitRoot) {
        roots.add(explicitRoot);
      } else {
        if (savedWorkspace?.targetKind === target.targetKind && savedWorkspace.targetId === target.targetId) roots.add(savedWorkspace.workspaceRoot);
        if (target.defaultRoot?.trim()) roots.add(target.defaultRoot.trim());
      }

      const attemptedRoots = new Set<string>();
      const validateRoot = async (root: string, discovered: boolean): Promise<PlanValidationResult | null> => {
        if (attemptedRoots.has(root)) return null;
        attemptedRoots.add(root);
        try {
          const probe = await invoke<AospWorkspaceProbe>("probe_aosp_linux_workspace", {
            request: { targetKind: target.targetKind, targetId: target.targetId, root },
          });
          if (!probe.linux || !probe.bashAvailable || !probe.envsetupPath || !probe.soongPath || !probe.manifestPath) {
            failures.push(`${target.name} 的 ${root} 不是完整 AOSP 源码树`);
            return null;
          }
          const resolved = { targetKind: target.targetKind, targetId: target.targetId, workspaceRoot: root };
          saveAospWorkspace(resolved);
          return {
            status: "ready",
            message: `${discovered ? "已自动检索并" : "已"}确认 ${target.targetKind.toUpperCase()} 目标“${target.name}”中的 AOSP 源码树：${root}。`,
            resolvedParameters: resolved,
          };
        } catch (reason) {
          failures.push(`${target.name} 的 ${root} 无法验证：${formatInvokeError(reason)}`);
          return null;
        }
      };

      for (const root of roots) {
        const resolved = await validateRoot(root, false);
        if (resolved) return resolved;
      }

      if (explicitRoot) continue;
      try {
        const discovered = await invoke<string[]>("discover_aosp_linux_workspaces", {
          request: { targetKind: target.targetKind, targetId: target.targetId },
        });
        const remaining = discovered.filter((root) => !attemptedRoots.has(root));
        if (remaining.length === 1) {
          const resolved = await validateRoot(remaining[0], true);
          if (resolved) return resolved;
        }
        if (remaining.length > 1) {
          ambiguous.push(`${target.name}：${remaining.join("、")}`);
          continue;
        }
      } catch (reason) {
        failures.push(`${target.name} 的目录检索失败：${formatInvokeError(reason)}`);
        continue;
      }

    }

    if (ambiguous.length) {
      return { status: "needs-input", message: `检测到多个有效 AOSP 源码树，无法安全地自动选择：${ambiguous.join("；")}。请在 AOSP 页面选择一次，之后会记住该目录。` };
    }
    return { status: "needs-input", message: failures.length ? `没有找到可用 AOSP 源码树：${failures[0]}。` : "未在已保存的 WSL 或 SSH 目标中找到 AOSP 源码树。" };
  }

  if (action.id === "open.tool.archive" && !parameters.path) {
    return { status: "needs-input", message: "缺少 APK 或 JAR 的本地路径；执行后会在包查看器中等待选择文件。" };
  }

  if (action.id === "open.tool.native-symbolizer" && (!parameters.tombstonePath || !parameters.symbolsPath)) {
    return { status: "needs-input", message: "需要 tombstone 文件和未剥离符号目录；执行后会在 Native 符号化页面等待选择。" };
  }

  if (action.id === "open.settings.agent") {
    const providers = await listAgentProviders();
    return providers.length
      ? { status: "ready", message: `已读取 ${providers.length} 个模型供应商配置。` }
      : { status: "needs-input", message: "尚未配置模型供应商，进入设置后添加即可。" };
  }
  return { status: "not-applicable", message: "该页面不需要在跳转前进行运行时校验。" };
}

interface TaskPipelineProps {
  activeStage: number;
  complete: boolean;
  analyzing: boolean;
}

const TaskPipeline: React.FC<TaskPipelineProps> = ({ activeStage, complete, analyzing }) => {
  const currentStage = PIPELINE_STAGES[Math.min(activeStage, PIPELINE_STAGES.length - 1)];
  return (
  <section className="quick-tool-task-pipeline" aria-label="任务调度进度">
    <div className="quick-tool-task-pipeline-heading">
      <div><b>任务调度</b><Badge variant={complete ? "success" : "primary"}>{complete ? "准备完成" : "进行中"}</Badge></div>
      <span>{complete ? "已完成执行前校验" : currentStage.label}</span>
    </div>
    <div className="quick-tool-task-stages">
      {PIPELINE_STAGES.map((stage, index) => {
        const done = complete || index < activeStage;
        const current = !complete && analyzing && index === activeStage;
        const connected = complete || index <= activeStage;
        const Icon = stage.icon;
        return (
          <div key={stage.label} className={`quick-tool-task-stage${done ? " done" : ""}${current ? " current" : ""}${connected ? " connected" : ""}`}>
            <span className="quick-tool-task-stage-icon">{done ? <Check size={15} /> : current ? <span className="btn-spinner quick-tool-task-stage-spinner" aria-label="处理中" /> : <Icon size={15} />}</span>
            <b>{stage.label}</b>
            <small>{stage.detail}</small>
          </div>
        );
      })}
    </div>
  </section>
  );
};

function formatTaskDuration(durationMs?: number) {
  if (!durationMs) return "--";
  return `${(durationMs / 1000).toFixed(durationMs < 1000 ? 1 : 0)}s`;
}

const TaskRunList: React.FC<{ steps: TaskRunStep[] }> = ({ steps }) => {
  const done = steps.filter((step) => step.status === "done").length;
  const pending = steps.filter((step) => step.status === "pending").length;
  return <section className="quick-tool-task-run-list" aria-label="具体执行流程">
    <div className="quick-tool-task-run-heading">
      <span>具体执行流程</span>
      <small>{pending ? `已完成 ${done} · 待执行 ${pending}` : `已完成 ${done} / ${steps.length}`}</small>
    </div>
    <div className="quick-tool-task-run-rows">
      {steps.map((step, index) => (
        <div className={`quick-tool-task-run-row ${step.status}`} key={step.id}>
          <span className="quick-tool-task-run-indicator" aria-hidden="true">
            {step.status === "done" ? <Check size={14} /> : step.status === "running" ? <span className="btn-spinner quick-tool-task-run-spinner" /> : step.status === "error" ? <X size={14} /> : <Circle size={14} />}
          </span>
          <div className="quick-tool-task-run-copy">
            <b>{step.title}</b>
            <p>{step.detail}</p>
          </div>
          <time>{step.status === "pending" ? "待执行" : step.status === "running" ? "进行中" : step.status === "error" ? "失败" : formatTaskDuration(step.durationMs)}</time>
          <span className="quick-tool-task-run-more" aria-hidden="true">{index + 1}</span>
        </div>
      ))}
    </div>
  </section>;
};

export const QuickToolPalette: React.FC<QuickToolPaletteProps> = ({ open, tools, onClose, onOpenTool, onExecuteAction, onMinimize, variant = "tools", initialQuery = "", onOpenAi, externalActivity, allowAi = true }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const inputRef = useRef<HTMLInputElement>(null);
  const aiInputRef = useRef<HTMLTextAreaElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const usageRef = useRef<ToolUsageStore>(loadToolUsage());
  const [toolUsage, setToolUsage] = useState<ToolUsageStore>(() => usageRef.current);
  const [query, setQuery] = useState(initialQuery);
  const [activeIndex, setActiveIndex] = useState(0);
  useEffect(() => {
    setQuery(initialQuery);
    setActiveIndex(0);
  }, [initialQuery]);
  const [mode, setMode] = useState<PaletteMode>(variant === "ai" ? "ai" : "tools");
  const [aiTask, setAiTask] = useState("");
  const [taskPlan, setTaskPlan] = useState<TaskPlan | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [activePlanningStage, setActivePlanningStage] = useState(0);
  const [taskRunSteps, setTaskRunSteps] = useState<TaskRunStep[]>([]);
  const [runtimeActivity, setRuntimeActivity] = useState<TaskRuntimeActivity>({
    title: "等待执行",
    detail: "完成分析后，执行状态会持续显示在这里。",
    status: "idle",
    progress: null,
  });
  const [taskHistory, setTaskHistory] = useState<TaskHistoryEntry[]>(() => loadTaskHistory());
  const [historyOpen, setHistoryOpen] = useState(false);
  const [plannerProviders, setPlannerProviders] = useState<AgentProvider[]>([]);
  const [plannerSelection, setPlannerSelection] = useState(() => selectedAgent());
  const [followUpTask, setFollowUpTask] = useState("");
  const dragRef = useRef<{
    pointerId: number;
    x: number;
    y: number;
    offsetX: number;
    offsetY: number;
    minX: number;
    maxX: number;
    minY: number;
    maxY: number;
  } | null>(null);
  const dragPointRef = useRef<{ x: number; y: number } | null>(null);
  const dragFrameRef = useRef<number | null>(null);
  const dialogOffsetRef = useRef({ x: 0, y: 0 });
  const dialogRef = useRef<HTMLElement>(null);
  const planningTimerRef = useRef<number | null>(null);
  const planningStartedAtRef = useRef(0);
  const activeTaskHistoryIdRef = useRef<string | null>(null);
  const displayedRuntimeActivity: TaskRuntimeActivity = externalActivity?.title
    ? {
        title: externalActivity.title,
        detail: externalActivity.detail || "目标页面正在处理 AI 交接的任务。",
        status: externalActivity.status || (externalActivity.running ? "running" : "idle"),
        progress: externalActivity.progress ?? null,
      }
    : runtimeActivity;

  const { unusedTools, usedTools, visibleTools } = useMemo(() => {
    const matching = tools.filter((tool) => matchesTool(tool, query));
    const unused = matching.filter((tool) => !toolUsage.tools[tool.id]);
    const used = matching
      .filter((tool) => Boolean(toolUsage.tools[tool.id]))
      .sort((left, right) => {
        const leftUsage = toolUsage.tools[left.id];
        const rightUsage = toolUsage.tools[right.id];
        return rightUsage.count - leftUsage.count
          || rightUsage.lastUsedAt - leftUsage.lastUsedAt
          || tools.indexOf(left) - tools.indexOf(right);
      });
    return { unusedTools: unused, usedTools: used, visibleTools: [...unused, ...used] };
  }, [query, toolUsage, tools]);

  const aiActionVisible = allowAi && matchesText("AI 提问 分析任务 调度建议", query);
  const paletteItems = useMemo<PaletteItem[]>(() => [
    ...(aiActionVisible ? [{ kind: "ask-ai" } as const] : []),
    ...visibleTools.map((tool) => ({ kind: "tool" as const, tool })),
  ], [aiActionVisible, visibleTools]);

  const recordToolUse = useCallback((toolId: string) => {
    const previous = usageRef.current;
    const current = previous.tools[toolId];
    const next: ToolUsageStore = {
      version: 1,
      tools: {
        ...previous.tools,
        [toolId]: { count: (current?.count || 0) + 1, lastUsedAt: Date.now() },
      },
    };
    usageRef.current = next;
    setToolUsage(next);
    saveToolUsage(next);
  }, []);

  const openTool = useCallback((tool: HomeTool) => {
    if (homeToolStatus(tool) !== "ready" || !tool.toolKey) return;
    recordToolUse(tool.id);
    onOpenTool(tool.toolKey);
    onClose();
  }, [onClose, onOpenTool, recordToolUse]);

  const openAiQuestion = useCallback(() => {
    if (onOpenAi) {
      onOpenAi();
      return;
    }
    setMode("ai");
  }, [onOpenAi]);

  const activateItem = useCallback((item: PaletteItem) => {
    if (item.kind === "ask-ai") {
      openAiQuestion();
      return;
    }
    openTool(item.tool);
  }, [openAiQuestion, openTool]);

  useEffect(() => {
    if (!open) return;
    setActiveIndex(0);
    if (variant === "tools") {
      setMode("tools");
      setActivePlanningStage(0);
    } else {
      setMode("ai");
    }
  }, [open, variant]);

  useEffect(() => {
    if (mode !== "ai" || !isTauri()) return;
    let cancelled = false;
    void listAgentProviders().then((next) => {
      if (cancelled) return;
      setPlannerProviders(next);
      setPlannerSelection((current) => {
        const provider = next.find((item) => item.id === current.providerId) || next[0];
        const models = provider?.models.length ? provider.models : provider?.model ? [{ id: provider.model, name: provider.model }] : [];
        const model = models.find((item) => item.id === current.modelId) || models[0];
        const selection = { providerId: provider?.id || "", modelId: model?.id || "" };
        if (selection.providerId && selection.modelId) saveSelectedAgent(selection.providerId, selection.modelId);
        return selection;
      });
    }).catch(() => {
      if (!cancelled) setPlannerProviders([]);
    });
    return () => { cancelled = true; };
  }, [mode]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    dialogOffsetRef.current = { x: 0, y: 0 };
    dialog.style.transform = mode === "ai" ? "translate3d(-50%, -50%, 0)" : "";
  }, [mode, open]);

  useEffect(() => () => {
    if (planningTimerRef.current) window.clearInterval(planningTimerRef.current);
  }, []);

  useEffect(() => {
    if (mode !== "ai") return;
    const onTaskActivity = (event: Event) => {
      const activity = (event as CustomEvent<{ running?: boolean; title?: string; detail?: string; progress?: number | null; actionId?: string; status?: string }>).detail;
      if (!activity?.actionId || !taskPlan?.execution || activity.actionId !== taskPlan.execution.actionId) return;
      const status = activity.status === "error" ? "error" : activity.status === "done" || !activity.running ? "done" : "running";
      setRuntimeActivity((current) => ({
        title: activity.title || current.title,
        detail: activity.detail || current.detail,
        status,
        progress: typeof activity.progress === "number" ? Math.max(0, Math.min(100, activity.progress)) : current.progress,
      }));
      setTaskRunSteps((current) => {
        const executionIndexes = current.flatMap((step, index) => step.kind === "execution" ? [index] : []);
        if (!executionIndexes.length) return current;
        const next = current.map((step) => ({ ...step }));
        if (activity.status === "error") {
          const runningIndex = executionIndexes.find((index) => next[index].status === "running") ?? executionIndexes.find((index) => next[index].status === "pending");
          if (runningIndex !== undefined) next[runningIndex] = { ...next[runningIndex], status: "error", detail: activity.detail || next[runningIndex].detail };
        } else if (activity.status === "done" || !activity.running) {
          for (const index of executionIndexes) {
            next[index] = { ...next[index], status: "done", detail: activity.detail || next[index].detail };
          }
        } else {
          const runningIndex = executionIndexes.find((index) => next[index].status === "running") ?? executionIndexes.find((index) => next[index].status === "pending");
          if (runningIndex !== undefined) next[runningIndex] = { ...next[runningIndex], status: "running", detail: activity.detail || next[runningIndex].detail };
        }
        return next;
      });
    };
    window.addEventListener(AI_TASK_ACTIVITY_EVENT, onTaskActivity);
    return () => window.removeEventListener(AI_TASK_ACTIVITY_EVENT, onTaskActivity);
  }, [mode, taskPlan]);

  const recordTaskHistory = useCallback((entry: TaskHistoryEntry) => {
    setTaskHistory((current) => {
      const next = [entry, ...current.filter((item) => item.id !== entry.id)];
      saveTaskHistory(next);
      return next;
    });
  }, []);

  // Keep the persisted record in sync while the destination page reports
  // progress. This is what lets reopening a minimized or historical task
  // show the live step state instead of the initial pending snapshot.
  useEffect(() => {
    const historyId = activeTaskHistoryIdRef.current;
    if (!historyId || !taskPlan) return;
    setTaskHistory((current) => {
      const entry = current.find((item) => item.id === historyId);
      if (!entry) return current;
      const next = current.map((item) => item.id === historyId
        ? {
            ...item,
            plan: taskPlan,
            runSteps: taskRunSteps,
            runtimeActivity,
            activePlanningStage,
          }
        : item);
      saveTaskHistory(next);
      return next;
    });
  }, [activePlanningStage, runtimeActivity, taskPlan, taskRunSteps]);

  const handleDialogPointerDown = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button, input, textarea, select, a")) return;
    const dialog = dialogRef.current;
    if (!dialog) return;
    const offset = dialogOffsetRef.current;
    const margin = 8;
    const halfViewportWidth = window.innerWidth / 2;
    const halfViewportHeight = window.innerHeight / 2;
    const halfDialogWidth = dialog.offsetWidth / 2;
    const halfDialogHeight = dialog.offsetHeight / 2;
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      offsetX: offset.x,
      offsetY: offset.y,
      minX: margin + halfDialogWidth - halfViewportWidth,
      maxX: halfViewportWidth - margin - halfDialogWidth,
      minY: margin + halfDialogHeight - halfViewportHeight,
      maxY: halfViewportHeight - margin - halfDialogHeight,
    };
    event.preventDefault();
    dialog.style.willChange = "transform";
    dialog.setPointerCapture(event.pointerId);
  };

  const handleDialogPointerMove = (event: React.PointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    const dialog = dialogRef.current;
    if (!drag || !dialog || drag.pointerId !== event.pointerId) return;
    dragPointRef.current = { x: event.clientX, y: event.clientY };
    if (dragFrameRef.current !== null) return;
    dragFrameRef.current = window.requestAnimationFrame(() => {
      dragFrameRef.current = null;
      const point = dragPointRef.current;
      if (!point || !dialogRef.current || !dragRef.current) return;
      const currentDialog = dialogRef.current;
      const currentDrag = dragRef.current;
      const offsetX = Math.min(currentDrag.maxX, Math.max(currentDrag.minX, currentDrag.offsetX + point.x - currentDrag.x));
      const offsetY = Math.min(currentDrag.maxY, Math.max(currentDrag.minY, currentDrag.offsetY + point.y - currentDrag.y));
      dialogOffsetRef.current = { x: offsetX, y: offsetY };
      currentDialog.style.transform = `translate3d(calc(-50% + ${offsetX}px), calc(-50% + ${offsetY}px), 0)`;
    });
  };

  const handleDialogPointerUp = () => {
    const dialog = dialogRef.current;
    if (dialog) {
      dialog.style.willChange = "auto";
      if (dragRef.current && dialog.hasPointerCapture(dragRef.current.pointerId)) {
        dialog.releasePointerCapture(dragRef.current.pointerId);
      }
    }
    dragRef.current = null;
    dragPointRef.current = null;
    if (dragFrameRef.current !== null) {
      window.cancelAnimationFrame(dragFrameRef.current);
      dragFrameRef.current = null;
    }
  };

  const handleDialogPointerCancel = (event: React.PointerEvent<HTMLElement>) => {
    if (!dragRef.current || dragRef.current.pointerId !== event.pointerId) return;
    handleDialogPointerUp();
  };

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      if (mode === "tools") inputRef.current?.focus();
      else aiInputRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [mode, open]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        if (analyzing) return;
        if (mode === "ai" && variant !== "ai") {
          setMode("tools");
          return;
        }
        onClose();
        return;
      }

      if (mode !== "tools" || event.isComposing || !paletteItems.length) return;

      if (event.key === "ArrowDown") {
        event.preventDefault();
        setActiveIndex((value) => (value + 1) % paletteItems.length);
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        setActiveIndex((value) => (value - 1 + paletteItems.length) % paletteItems.length);
      }

      if (event.key === "Enter" && paletteItems[activeIndex]) {
        event.preventDefault();
        activateItem(paletteItems[activeIndex]);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeIndex, activateItem, analyzing, mode, onClose, open, paletteItems, variant]);

  useEffect(() => {
    setActiveIndex((value) => Math.min(value, Math.max(0, paletteItems.length - 1)));
  }, [paletteItems.length]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLButtonElement>(".quick-tool-palette-row.active")?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, paletteItems]);

  const analyzeTask = async (taskOverride?: string) => {
    const task = (taskOverride ?? aiTask).trim();
    if (!task) {
      notify("请先描述希望完成的工作。", "warning");
      aiInputRef.current?.focus();
      return;
    }
    if (!isTauri()) {
      notify("浏览器预览无法调用已配置的模型，请在桌面应用中分析任务。", "warning");
      return;
    }

    if (task !== aiTask) setAiTask(task);
    const historyId = taskOverride !== undefined && activeTaskHistoryIdRef.current
      ? activeTaskHistoryIdRef.current
      : `task-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    activeTaskHistoryIdRef.current = historyId;
    setAnalyzing(true);
    setTaskPlan(null);
    setRuntimeActivity({
      title: "正在分析任务",
      detail: "AI 正在理解目标并匹配应用能力。",
      status: "running",
      progress: null,
    });
    setActivePlanningStage(0);
    planningStartedAtRef.current = Date.now();
    setTaskRunSteps([
      { id: "understand", title: "理解任务目标", detail: "提取目标、约束与已有条件", kind: "analysis", status: "running" },
      { id: "capability", title: "匹配应用能力", detail: "读取已公开页面动作与可用模型", kind: "analysis", status: "pending" },
      { id: "plan", title: "生成执行计划", detail: "由 AI 选择页面与后续执行顺序", kind: "analysis", status: "pending" },
      { id: "validation", title: "校验执行条件", detail: "确认设备、连接或配置是否满足前置条件", kind: "validation", status: "pending" },
    ]);
    publishAiTaskActivity({ running: true, title: "正在分析任务" });
    if (planningTimerRef.current) window.clearInterval(planningTimerRef.current);
    try {
      const providers = plannerProviders.length ? plannerProviders : await listAgentProviders();
      const saved = plannerSelection;
      const provider = providers.find((item) => item.id === saved.providerId) || providers[0];
      const models = provider?.models.length ? provider.models : provider?.model ? [{ id: provider.model, name: provider.model }] : [];
      const model = models.find((item) => item.id === saved.modelId) || models[0];
      if (!provider || !model?.id) throw new Error("请先在设置中配置模型供应商，再使用 AI 提问。");

      const capabilityStartedAt = Date.now();
      setActivePlanningStage(1);
      setTaskRunSteps((current) => current.map((step) => {
        if (step.id === "understand") return { ...step, status: "done", durationMs: capabilityStartedAt - planningStartedAtRef.current };
        if (step.id === "capability") return { ...step, status: "done", durationMs: capabilityStartedAt - planningStartedAtRef.current };
        if (step.id === "plan") return { ...step, status: "running" };
        return step;
      }));

      const response = await invoke<AgentResponse>("agent_one_shot", {
        request: {
          modelId: `${provider.id}:${model.id}`,
          message: `你是 Android Toolkit 的任务调度规划器。先理解用户全部自然语言目标，再从应用实际公开的页面动作中选择执行位置。只分析和规划：不调用任何工具、不读取文件、不修改系统、不执行命令，也不要按关键词猜测文件名、页面或命令。\n\n用户目标：\n${task}\n\n应用当前公开的动作目录（每一行是一个 JSON 对象；只能选择其中 actionId，parameters 只能使用该动作列出的名称）：\n${applicationActionCatalogForPrompt()}\n\nAOSP 规则：用户表达“编译、构建、build、lunch、模块编译”等目标时，必须选择 open.tool.aosp，并在 parameters 中填写 operation="build"。在用户给出了 lunch 目标或模块名时，分别填入 lunchTarget、moduleName；没有明确值时省略，绝不编造。仅用户要求检查源码树或构建环境时才填写 operation="inspect"。\n\n请只返回一个合法 JSON 对象，禁止 Markdown、代码围栏、标题或对象外的任何文字。结构必须完全符合：\n{"summary":"一句话说明任务理解","steps":[{"title":"步骤名称","detail":"该步骤的说明或需要确认的信息"}],"execution":{"actionId":"目录中的动作 ID","summary":"为什么应在此处执行","parameters":{"仅填写该动作允许的参数名":"值"}}}\n\nsteps 写 2 至 5 步。需要用户先补充信息时也要选择最合适的页面动作；确实没有可用页面动作时 execution 写 null。不要虚构连接 ID、设备序列号、文件路径或未公开的动作。`,
          jiraContext: null,
          artifacts: [],
          attachments: [],
          workspacePath: null,
          sessionId: `quick-plan-${Date.now()}`,
          turnId: `quick-plan-turn-${Date.now()}`,
          permissionMode: "ask",
          analysisMode: false,
          context: [],
          toolPolicy: "disabled",
          skillIds: [],
          requestedTool: null,
          reasoningEffort: model.supportsReasoning ? "medium" : null,
          taskPlanMode: "off",
          taskPlanMinSteps: 2,
          executionTarget: { kind: "local", connectionId: null, profileId: null, root: null },
        },
      });
      const parsedPlan = parseTaskPlan(response.text || "");
      const validationStartedAt = Date.now();
      setActivePlanningStage(2);
      setTaskRunSteps((current): TaskRunStep[] => {
        const updated = current.map((step): TaskRunStep => {
          if (step.id === "plan") return { ...step, status: "done", durationMs: validationStartedAt - capabilityStartedAt };
          return step;
        });
        const validationStep = updated.find((step) => step.id === "validation");
        return [
          ...updated.filter((step) => step.id !== "validation"),
          ...parsedPlan.steps.map((step, index) => ({
          id: `execution-${index}`,
          title: step.title,
          detail: step.detail,
          kind: "execution" as const,
          status: "pending" as const,
          })),
          ...(validationStep ? [{ ...validationStep, status: "running" as const }] : []),
        ];
      });
       const { resolvedParameters, ...validation } = await validateTaskPlan(parsedPlan);
       const resolvedPlan = resolvedParameters && parsedPlan.execution
         ? {
             ...parsedPlan,
             execution: {
               ...parsedPlan.execution,
               parameters: { ...parsedPlan.execution.parameters, ...resolvedParameters },
             },
           }
         : parsedPlan;
       setTaskPlan({ ...resolvedPlan, validation });
       setRuntimeActivity({
         title: resolvedPlan.execution ? "已完成执行前校验" : "任务仅完成分析",
         detail: validation.message,
         status: "idle",
         progress: null,
       });
       setActivePlanningStage(PIPELINE_STAGES.length - 1);
      setTaskRunSteps((current) => current.map((step) => step.id === "validation"
        ? { ...step, status: "done", durationMs: Date.now() - validationStartedAt }
        : step));
      const destination = resolvedPlan.execution ? resolveApplicationAction(resolvedPlan.execution.actionId)?.destination || "待确认" : "无直接执行页面";
      const persistedSteps: TaskRunStep[] = [
        { id: "understand", title: "理解任务目标", detail: "提取目标、约束与已有条件", kind: "analysis", status: "done" },
        { id: "capability", title: "匹配应用能力", detail: "读取已公开页面动作与可用模型", kind: "analysis", status: "done" },
        { id: "plan", title: "生成执行计划", detail: "由 AI 选择页面与后续执行顺序", kind: "analysis", status: "done" },
        ...resolvedPlan.steps.map((step, index) => ({ id: `execution-${index}`, title: step.title, detail: step.detail, kind: "execution" as const, status: "pending" as const })),
        { id: "validation", title: "校验执行条件", detail: validation.message, kind: "validation", status: "done" },
      ];
      const persistedRuntime: TaskRuntimeActivity = {
        title: resolvedPlan.execution ? "已完成执行前校验" : "任务仅完成分析",
        detail: validation.message,
        status: "idle",
        progress: null,
      };
       recordTaskHistory({ id: historyId, task, summary: resolvedPlan.summary, destination, actionId: resolvedPlan.execution?.actionId, status: "planned", createdAt: Date.now(), plan: resolvedPlan, runSteps: persistedSteps, runtimeActivity: persistedRuntime, activePlanningStage: PIPELINE_STAGES.length - 1 });
    } catch (error) {
      setTaskRunSteps((current) => current.map((step) => step.status === "running" ? { ...step, status: "error", durationMs: Date.now() - planningStartedAtRef.current } : step));
      notify(formatInvokeError(error), "error");
    } finally {
      if (planningTimerRef.current) {
        window.clearInterval(planningTimerRef.current);
        planningTimerRef.current = null;
      }
      setAnalyzing(false);
      publishAiTaskActivity({ running: false });
    }
  };

  const executeTaskPlan = async () => {
    if (!taskPlan?.execution) return;
    setTaskRunSteps((current) => current.map((step, index) => step.kind === "execution"
      ? { ...step, status: index === current.findIndex((candidate) => candidate.kind === "execution") ? "running" : "pending" }
      : step));
    setRuntimeActivity({
      title: "正在启动执行流程",
      detail: "正在打开目标页面并交接任务参数。",
      status: "running",
      progress: 0,
    });
    publishAiTaskActivity({ running: true, status: "running", title: "正在启动执行流程", detail: "正在打开目标页面并交接任务参数", actionId: taskPlan.execution.actionId });
    const destination = resolveApplicationAction(taskPlan.execution.actionId)?.destination || "目标页面";
    try {
      await onExecuteAction(taskPlan.execution);
       recordTaskHistory({ id: activeTaskHistoryIdRef.current || `task-${Date.now()}`, task: aiTask.trim(), summary: taskPlan.summary, destination, actionId: taskPlan.execution.actionId, status: "executed", createdAt: Date.now(), plan: taskPlan, runSteps: taskRunSteps, runtimeActivity: { title: `已进入${destination}`, detail: "等待目标页面回报执行进度", status: "running", progress: null }, activePlanningStage: PIPELINE_STAGES.length - 1 });
      notify(`已启动${destination}，AI 提问窗口将持续显示执行状态。`, "success");
    } finally {
      // The destination page owns the actual operation and will publish the
      // terminal status. Keep this window open so the user can follow it.
      publishAiTaskActivity({ running: true, status: "running", title: `已进入${destination}`, detail: "等待目标页面回报执行进度", actionId: taskPlan.execution.actionId });
    }
  };

  const continueTaskConversation = () => {
    const followUp = followUpTask.trim();
    if (!followUp) {
      notify("请先输入要补充或追问的内容。", "warning");
      return;
    }
    setFollowUpTask("");
    void analyzeTask(`${aiTask.trim()}\n\n补充要求：${followUp}`);
  };

  const restoreTaskHistory = (entry: TaskHistoryEntry) => {
    activeTaskHistoryIdRef.current = entry.id;
    setAiTask(entry.task);
    setTaskPlan(entry.plan || null);
    setTaskRunSteps(entry.runSteps || []);
    setRuntimeActivity(entry.runtimeActivity || {
      title: entry.status === "executed" ? "已进入执行页面" : "已恢复任务计划",
      detail: entry.summary,
      status: entry.status === "executed" ? "running" : "idle",
      progress: null,
    });
    setActivePlanningStage(entry.activePlanningStage ?? (entry.plan ? PIPELINE_STAGES.length - 1 : 0));
    setHistoryOpen(false);
  };

  const deleteTaskHistory = (id: string) => {
    confirmAction({ title: "删除任务记录", message: `确认删除「${taskHistory.find((entry) => entry.id === id)?.task || "此任务"}」的历史记录？不会撤销已执行的操作。`, onConfirm: () => {
      const next = taskHistory.filter((entry) => entry.id !== id);
      saveTaskHistory(next);
      setTaskHistory(next);
    } });
  };

  const dismissAiDialog = () => {
    if (analyzing || runtimeActivity.status === "running") {
      onMinimize();
      return;
    }
    onClose();
  };

  if (!open) return null;

  return (
    <div
      className={`quick-tool-palette-overlay${variant === "ai" ? " ai-planner-overlay" : ""}`}
      onPointerDown={(event) => {
        if (mode === "tools" && event.target === event.currentTarget) onClose();
      }}
    >
      {confirmationDialog}
      {mode === "tools" ? (
        <section ref={dialogRef} className="quick-tool-palette quick-tool-search-dialog" role="dialog" aria-modal="true" aria-label="快捷工具">
          <header className="quick-tool-palette-header">
            <Input ref={inputRef} size="sm" aria-label="筛选工具" prefixIcon={<Search size={16} />} value={query} placeholder="搜索主页工具或快捷操作..." wrapperClassName="quick-tool-palette-search" suffixAction={<kbd>Esc</kbd>} onChange={(event) => { setQuery(event.target.value); setActiveIndex(0); }} />
          </header>

          <div ref={listRef} className="quick-tool-palette-list" role="listbox" aria-label="快捷工具">
            {aiActionVisible && (
              <button type="button" className={`quick-tool-palette-row quick-tool-palette-ai-action${activeIndex === 0 ? " active" : ""}`} role="option" aria-selected={activeIndex === 0} onMouseMove={() => setActiveIndex(0)} onClick={openAiQuestion}>
                <span className="quick-tool-palette-ai-icon"><Sparkles size={18} /></span>
                <span className="quick-tool-palette-copy"><b>AI 提问</b><small>先分析目标和调度方式，再决定是否继续执行</small></span>
                <span className="quick-tool-palette-action"><Badge variant="neutral">规划</Badge><CornerDownLeft size={14} aria-hidden="true" /></span>
              </button>
            )}

            {unusedTools.length > 0 && <div className="quick-tool-palette-section-label">工具</div>}
            {unusedTools.map((tool) => {
              const index = (aiActionVisible ? 1 : 0) + visibleTools.indexOf(tool);
              return <ToolRow key={tool.id} tool={tool} index={index} active={index === activeIndex} onHighlight={setActiveIndex} onActivate={openTool} />;
            })}

            {usedTools.length > 0 && <div className="quick-tool-palette-section-label used">已使用 · 按次数排序</div>}
            {usedTools.map((tool) => {
              const index = (aiActionVisible ? 1 : 0) + visibleTools.indexOf(tool);
              return <ToolRow key={tool.id} tool={tool} index={index} active={index === activeIndex} useCount={toolUsage.tools[tool.id].count} onHighlight={setActiveIndex} onActivate={openTool} />;
            })}

            {!paletteItems.length && <div className="quick-tool-palette-empty">没有匹配的快捷操作或主页工具</div>}
          </div>
        </section>
      ) : (
        <section ref={dialogRef} className="quick-tool-palette quick-tool-ai-dialog" role="dialog" aria-modal="true" aria-label="AI 提问" onPointerMove={handleDialogPointerMove} onPointerUp={handleDialogPointerUp} onPointerCancel={handleDialogPointerCancel}>
          <header className="quick-tool-ai-header quick-tool-draggable-header" onPointerDown={handleDialogPointerDown}>
              <span className="quick-tool-drag-handle" aria-hidden="true"><GripHorizontal size={16} /></span>
              <span className="quick-tool-ai-title"><Bot size={18} />AI 提问</span>
              <div className="quick-tool-window-actions">
                <button type="button" className={`quick-tool-history-button${historyOpen ? " active" : ""}`} title="任务历史" aria-label="任务历史" onClick={() => setHistoryOpen((value) => !value)}><History size={15} /></button>
                <button type="button" className="quick-tool-window-minimize" title="收起到侧栏" aria-label="收起到侧栏" onClick={onMinimize}><Minus size={16} /></button>
                <button type="button" className="quick-tool-ai-close" title={analyzing || runtimeActivity.status === "running" ? "收起并持续跟踪任务" : "关闭 AI 提问"} aria-label={analyzing || runtimeActivity.status === "running" ? "收起并持续跟踪任务" : "关闭 AI 提问"} onClick={dismissAiDialog}><X size={16} /></button>
              </div>
          </header>
          <div className="quick-tool-ai-body">
            <div className="quick-tool-ai-content">
            <label className="quick-tool-ai-field"><span>想完成什么？</span><textarea ref={aiInputRef} value={aiTask} placeholder="描述目标、已有条件和希望得到的结果" rows={5} onChange={(event) => setAiTask(event.target.value)} /></label>
            {(analyzing || taskPlan) && <TaskPipeline activeStage={activePlanningStage} complete={Boolean(taskPlan)} analyzing={analyzing} />}
            {taskRunSteps.length > 0 && <TaskRunList steps={taskRunSteps} />}
            {taskPlan && <section className="quick-tool-ai-result" aria-label="AI 任务调度结果">
              <div className="quick-tool-ai-result-heading"><span>任务调度结果</span><Badge variant="success">已就绪</Badge></div>
              <p className="quick-tool-ai-summary">{taskPlan.summary}</p>
              <div className={`quick-tool-ai-validation ${taskPlan.validation.status}`}><span>{taskPlan.validation.status === "ready" ? <Check size={14} /> : <CircleDot size={14} />}</span><div><b>执行条件校验</b><p>{taskPlan.validation.message}</p></div></div>
              {taskPlan.execution ? (() => {
                const action = resolveApplicationAction(taskPlan.execution!.actionId)!;
                return <div className="quick-tool-ai-destination"><span><MapPin size={15} />执行位置</span><div><b>{action.destination}</b><p>{taskPlan.execution!.summary}</p></div><ChevronRight size={16} aria-hidden="true" /></div>;
              })() : <div className="quick-tool-ai-no-destination"><CircleDot size={15} />当前应用没有可直接执行的页面，需要先补充任务条件。</div>}
            </section>}
            {(taskPlan || externalActivity?.title) && <section className={`quick-tool-ai-runtime ${displayedRuntimeActivity.status}`} aria-live="polite">
              <span className="quick-tool-ai-runtime-icon">{displayedRuntimeActivity.status === "done" ? <Check size={15} /> : displayedRuntimeActivity.status === "error" ? <X size={15} /> : displayedRuntimeActivity.status === "running" ? <span className="btn-spinner quick-tool-runtime-spinner" /> : <CircleDot size={15} />}</span>
              <div>
                <div className="quick-tool-ai-runtime-heading"><b>{displayedRuntimeActivity.title}</b><small>{displayedRuntimeActivity.status === "running" ? "执行中" : displayedRuntimeActivity.status === "done" ? "已完成" : displayedRuntimeActivity.status === "error" ? "失败" : "待执行"}</small></div>
                <p>{displayedRuntimeActivity.detail}</p>
                {displayedRuntimeActivity.progress !== null && <span className="quick-tool-ai-runtime-progress"><i style={{ width: `${displayedRuntimeActivity.progress}%` }} /><small>{displayedRuntimeActivity.progress}%</small></span>}
              </div>
            </section>}
            {taskPlan && <section className="quick-tool-ai-follow-up" aria-label="继续与 AI 对话">
              <div className="quick-tool-ai-follow-up-field">
                <span>继续与 AI 对话</span>
                <textarea rows={2} value={followUpTask} placeholder="补充条件、追问分析结果，或要求调整执行计划" disabled={analyzing} onChange={(event) => setFollowUpTask(event.target.value)} />
              </div>
              <Button type="button" variant="secondary" size="sm" icon={<Send size={14} />} disabled={analyzing || !followUpTask.trim()} onClick={continueTaskConversation}>继续分析</Button>
            </section>}
            {historyOpen && <section className="quick-tool-ai-history" aria-label="任务历史">
              <div className="quick-tool-ai-history-heading"><span>任务历史</span><small>最近 {taskHistory.length} 条</small></div>
              {taskHistory.length ? taskHistory.slice(0, 8).map((entry) => <div className="quick-tool-ai-history-row" key={entry.id}>
                <button type="button" className="quick-tool-ai-history-main" onClick={() => restoreTaskHistory(entry)}><span><b>{entry.task}</b><small>{entry.destination} · {entry.status === "executed" ? "已进入" : "已分析"}{entry.plan ? " · 含完整进度" : ""}</small></span><time>{new Date(entry.createdAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time></button>
                <button type="button" className="quick-tool-ai-history-delete" title="删除任务记录" aria-label={`删除任务记录：${entry.task}`} onClick={() => deleteTaskHistory(entry.id)}><Trash2 size={13} /></button>
              </div>) : <p className="quick-tool-ai-history-empty">还没有任务记录</p>}
            </section>}
            </div>
          </div>
          <footer className="quick-tool-ai-footer">
            <Button type="button" variant="subtle" size="sm" onClick={dismissAiDialog}>{analyzing || runtimeActivity.status === "running" ? "收起" : "关闭"}</Button>
            <div className="quick-tool-ai-footer-actions">
              <AgentModelPicker
                className="quick-tool-ai-model-picker"
                providers={plannerProviders}
                selection={plannerSelection}
                disabled={analyzing || !plannerProviders.length}
                onChange={(next) => {
                  setPlannerSelection(next);
                  saveSelectedAgent(next.providerId, next.modelId);
                }}
                onOpenSettings={() => window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: "agent" } }))}
              />
              {taskPlan ? <Button type="button" variant="primary" size="sm" icon={<Play size={15} />} disabled={!taskPlan.execution || analyzing} onClick={() => { void executeTaskPlan(); }}>执行</Button> : <Button type="button" variant="primary" size="sm" icon={<Sparkles size={15} />} loading={analyzing} onClick={() => { void analyzeTask(); }}>分析任务</Button>}
            </div>
          </footer>
        </section>
      )}
    </div>
  );
};

interface ToolRowProps {
  tool: HomeTool;
  index: number;
  active: boolean;
  useCount?: number;
  onHighlight: (index: number) => void;
  onActivate: (tool: HomeTool) => void;
}

const ToolRow: React.FC<ToolRowProps> = React.memo(({ tool, index, active, useCount, onHighlight, onActivate }) => {
  const status = homeToolStatus(tool);
  return (
    <button type="button" className={`quick-tool-palette-row${active ? " active" : ""}${status === "ready" ? "" : " unavailable"}`} role="option" aria-selected={active} aria-disabled={status !== "ready"} onFocus={() => onHighlight(index)} onMouseMove={() => onHighlight(index)} onClick={() => onActivate(tool)}>
      <ToolIcon className="home-tool-icon-box">{tool.icon}</ToolIcon>
      <span className="quick-tool-palette-copy"><b>{tool.title}</b><small>{tool.subtitle}</small></span>
      <span className="quick-tool-palette-action"><Badge variant={homeToolStatusVariant(status)}>{homeToolStatusLabel(status)}</Badge>{useCount ? <small className="quick-tool-usage-count">{useCount} 次</small> : null}{status === "ready" && <CornerDownLeft size={14} aria-hidden="true" />}</span>
    </button>
  );
});

export default QuickToolPalette;
