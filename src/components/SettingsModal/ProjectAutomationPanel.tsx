import React, { useEffect, useMemo, useRef, useState } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import {
  ArrowLeft,
  ArrowUpRight,
  Boxes,
  ChevronDown,
  ChevronRight,
  FileText,
  FolderKanban,
  FolderOpen,
  GitBranch,
  Plus,
  Save,
  Server,
  Sparkles,
  TerminalSquare,
  Trash2,
  Upload,
  Workflow,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Badge, Button, Input, Select } from "@/components/ui";
import { isTauri, openDirectory, openFile } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import { writeProjectPreference } from "@/platform/projectPreferences";
import { publishAiTaskActivity } from "@/platform/notifications";
import { analyzeProjectKnowledge } from "./projectKnowledgeAnalysis";

type ProjectTab = "overview" | "connections" | "commands" | "workflows" | "knowledge";

type ProjectCommand = {
  id: string;
  name: string;
  target: string;
  command: string;
};

type ProjectWorkflow = {
  id: string;
  name: string;
  steps: string;
};

/** 「已加入项目」是旧版本写入的状态：文档只登记了路径、没有摘要，原样保留不冒充已解析。 */
type ProjectKnowledgeStatus = "待分析" | "解析中" | "已解析" | "已加入项目";

type ProjectKnowledge = {
  id: string;
  path: string;
  status: ProjectKnowledgeStatus;
  /** AI 解析出的知识摘要，会随项目配置一起进入对话上下文。 */
  analysis?: string;
  analyzedAt?: number;
  analysisModel?: string;
  analysisTruncated?: boolean;
};

type AutomationProject = {
  id: string;
  name: string;
  description: string;
  instructions: string;
  builtin?: boolean;
  commands: ProjectCommand[];
  workflows: ProjectWorkflow[];
  knowledge: ProjectKnowledge[];
};

const STORAGE_KEY = "android-tool.project-automation.v1";
const ACTIVE_PROJECT_KEY = "android-tool.project-automation.active-project.v1";
const DEFAULT_PROJECT: AutomationProject = {
  id: "general",
  name: "通用",
  description: "适用于所有项目的公共命令、设备操作和知识。",
  instructions: "",
  builtin: true,
  commands: [],
  workflows: [],
  knowledge: [],
};

const tabs: Array<{ id: ProjectTab; label: string; icon: LucideIcon }> = [
  { id: "overview", label: "项目概览", icon: FolderKanban },
  { id: "connections", label: "连接环境", icon: Server },
  { id: "commands", label: "命令模板", icon: TerminalSquare },
  { id: "workflows", label: "自动化流程", icon: Workflow },
  { id: "knowledge", label: "知识文档", icon: FileText },
];

/**
 * 读旧数据时把知识条目补全。
 * 旧版本只存 `{ id, path, status: "待分析" | "已加入项目" }`：
 * 「已加入项目」原样保留（它没有摘要，不能当成已解析），「解析中」是运行时状态，重启后回落成待分析，
 * 其它未识别状态统一回落成待分析，避免列表里出现脏字符串。
 */
function normalizeKnowledge(raw: unknown): ProjectKnowledge {
  const item = (raw ?? {}) as Partial<ProjectKnowledge> & { id?: string; path?: string };
  const status: ProjectKnowledgeStatus = item.status === "已加入项目" || item.status === "已解析"
    ? item.status
    : "待分析";
  return {
    id: typeof item.id === "string" ? item.id : id("knowledge"),
    path: typeof item.path === "string" ? item.path : "",
    status,
    analysis: typeof item.analysis === "string" ? item.analysis : undefined,
    analyzedAt: typeof item.analyzedAt === "number" ? item.analyzedAt : undefined,
    analysisModel: typeof item.analysisModel === "string" ? item.analysisModel : undefined,
    analysisTruncated: typeof item.analysisTruncated === "boolean" ? item.analysisTruncated : undefined,
  };
}

function readProjects(): AutomationProject[] {
  if (typeof window === "undefined") return [DEFAULT_PROJECT];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(STORAGE_KEY) || "null") as AutomationProject[] | null;
    if (!Array.isArray(parsed)) return [DEFAULT_PROJECT];
    const projects = parsed
      .filter((item) => item && typeof item.id === "string" && typeof item.name === "string")
      .map((item) => ({
        ...item,
        instructions: typeof item.instructions === "string" ? item.instructions : "",
        commands: Array.isArray(item.commands) ? item.commands : [],
        workflows: Array.isArray(item.workflows) ? item.workflows : [],
        knowledge: Array.isArray(item.knowledge) ? item.knowledge.map(normalizeKnowledge) : [],
      }));
    return projects.some((item) => item.id === DEFAULT_PROJECT.id) ? projects : [DEFAULT_PROJECT, ...projects];
  } catch {
    return [DEFAULT_PROJECT];
  }
}

const id = (prefix: string) => `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

export const ProjectAutomationPanel: React.FC = () => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [projects, setProjects] = useState<AutomationProject[]>(readProjects);
  const [selectedProjectId, setSelectedProjectId] = useState(() => {
    if (typeof window === "undefined") return "general";
    return window.localStorage.getItem(ACTIVE_PROJECT_KEY) || "general";
  });
  const [activeTab, setActiveTab] = useState<ProjectTab>("overview");
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [newProjectName, setNewProjectName] = useState("");
  const [newProjectDescription, setNewProjectDescription] = useState("");
  const [commandName, setCommandName] = useState("");
  const [commandTarget, setCommandTarget] = useState("");
  const [commandText, setCommandText] = useState("");
  const [workflowName, setWorkflowName] = useState("");
  const [workflowSteps, setWorkflowSteps] = useState("");
  const [projectInstructions, setProjectInstructions] = useState("");
  /** 正在解析的文档 id（只存在内存里，"解析中" 不进 localStorage，重启后仍是待分析）。 */
  const [analyzingKnowledge, setAnalyzingKnowledge] = useState<Record<string, true>>({});
  /** 展开查看解析结果的文档 id。 */
  const [expandedKnowledge, setExpandedKnowledge] = useState<Record<string, true>>({});
  const textareaRefs = useRef<Record<string, HTMLTextAreaElement | null>>({});

  const resizeProjectTextarea = (key: string, element?: HTMLTextAreaElement | null) => {
    const target = element || textareaRefs.current[key];
    if (!target) return;
    target.style.height = "auto";
    const minHeight = 88;
    const maxHeight = 240;
    const nextHeight = Math.min(maxHeight, Math.max(minHeight, target.scrollHeight));
    target.style.height = `${nextHeight}px`;
    target.style.overflowY = target.scrollHeight > maxHeight ? "auto" : "hidden";
  };

  useEffect(() => {
    resizeProjectTextarea("instructions");
    resizeProjectTextarea("command");
    resizeProjectTextarea("workflow");
  }, [projectInstructions, commandText, workflowSteps, activeTab]);

  useEffect(() => {
    const widths = new WeakMap<Element, number>();
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (widths.get(entry.target) === entry.contentRect.width) continue;
        widths.set(entry.target, entry.contentRect.width);
        resizeProjectTextarea("", entry.target as HTMLTextAreaElement);
      }
    });
    Object.values(textareaRefs.current).forEach((element) => { if (element) observer.observe(element); });
    return () => observer.disconnect();
  }, [activeTab]);

  const selectedProject = projects.find((project) => project.id === selectedProjectId) || projects[0] || DEFAULT_PROJECT;

  useEffect(() => {
    writeProjectPreference(STORAGE_KEY, JSON.stringify(projects));
  }, [projects]);

  useEffect(() => {
    writeProjectPreference(ACTIVE_PROJECT_KEY, selectedProjectId);
  }, [selectedProjectId]);

  useEffect(() => {
    setProjectInstructions(selectedProject.instructions || "");
  }, [selectedProject.id]);

  const updateSelectedProject = (update: (project: AutomationProject) => AutomationProject) => {
    setProjects((current) => current.map((project) => project.id === selectedProject.id ? update(project) : project));
  };

  const projectOptions = useMemo(() => projects.map((project) => ({ label: project.name, value: project.id })), [projects]);

  const createProject = () => {
    const name = newProjectName.trim();
    if (!name) {
      notify("请先填写项目名称。", "error");
      return;
    }
    if (projects.some((project) => project.name === name)) {
      notify("项目名称已存在。", "error");
      return;
    }
    const project: AutomationProject = {
      id: id("project"),
      name,
      description: newProjectDescription.trim() || "未填写项目说明。",
      instructions: "",
      commands: [],
      workflows: [],
      knowledge: [],
    };
    setProjects((current) => [...current, project]);
    setSelectedProjectId(project.id);
    setNewProjectName("");
    setNewProjectDescription("");
    setNewProjectOpen(false);
    setActiveTab("overview");
    notify(`项目“${name}”已创建。`, "success");
  };

  const deleteSelectedProject = () => {
    if (selectedProject.id === DEFAULT_PROJECT.id) {
      notify("“通用”项目不可删除。", "error");
      return;
    }
    confirmAction({ title: "删除项目", message: `确认删除「${selectedProject.name}」及其中的命令、流程和知识文档关联？不会删除项目的本地文件。`, onConfirm: () => {
      setProjects((current) => current.filter((project) => project.id !== selectedProject.id));
      setSelectedProjectId(DEFAULT_PROJECT.id);
      setActiveTab("overview");
      notify("项目已删除。", "success");
    } });
  };

  const addCommand = () => {
    const name = commandName.trim();
    const command = commandText.trim();
    if (!name || !command) {
      notify("请填写命令名称和执行内容。", "error");
      return;
    }
    updateSelectedProject((project) => ({
      ...project,
      commands: [...project.commands, { id: id("command"), name, target: commandTarget.trim() || "自定义", command }],
    }));
    setCommandName("");
    setCommandText("");
    notify("命令模板已保存到当前项目。", "success");
  };

  const saveProjectInstructions = () => {
    updateSelectedProject((project) => ({ ...project, instructions: projectInstructions.trim() }));
    notify("项目工作说明已保存，勾选该项目后会提供给后续对话。", "success");
  };

  const addWorkflow = () => {
    const name = workflowName.trim();
    const steps = workflowSteps.trim();
    if (!name || !steps) {
      notify("请填写流程名称和步骤。", "error");
      return;
    }
    updateSelectedProject((project) => ({
      ...project,
      workflows: [...project.workflows, { id: id("workflow"), name, steps }],
    }));
    setWorkflowName("");
    setWorkflowSteps("");
    notify("自动化流程已保存到当前项目。", "success");
  };

  const importKnowledge = async (directory: boolean) => {
    if (!isTauri()) {
      notify("请在桌面应用中选择知识文档。", "error");
      return;
    }
    const path = directory
      ? await openDirectory("选择项目知识目录")
      : await openFile("选择项目知识文档", [{ name: "文档", extensions: ["md", "txt", "json", "yaml", "yml", "pdf", "docx"] }]);
    if (!path) return;
    updateSelectedProject((project) => ({
      ...project,
      knowledge: [...project.knowledge, { id: id("knowledge"), path, status: "待分析" }],
    }));
    notify("文档已加入当前项目，等待分析。", "success");
  };

  /**
   * 用 AI 解析一份知识文档：
   * 没配模型 → 提示去 Agent 接入里配置；配了就解析，成功后状态变「已解析」并把摘要存回项目库。
   */
  const analyzeKnowledge = async (item: ProjectKnowledge) => {
    if (analyzingKnowledge[item.id]) return;
    const title = item.path.split(/[\\/]/).filter(Boolean).pop() || item.path;
    setAnalyzingKnowledge((current) => ({ ...current, [item.id]: true }));
    publishAiTaskActivity({ running: true, status: "running", actionId: `project-knowledge-${item.id}`, title: "正在解析项目文档", detail: title, progress: null });
    try {
      const result = await analyzeProjectKnowledge(item.path);
      if (!result.ok) {
        // 未配置 AI 是前置条件：模块里的提示已经写明去「Agent 接入」配置，这里不再重复。
        notify(result.message, "error");
        publishAiTaskActivity({ running: false, status: "error", actionId: `project-knowledge-${item.id}`, title: "文档解析未完成", detail: result.message });
        return;
      }
      updateSelectedProject((project) => ({
        ...project,
        knowledge: project.knowledge.map((entry) => entry.id === item.id ? {
          ...entry,
          status: "已解析",
          analysis: result.summary,
          analyzedAt: Date.now(),
          analysisModel: result.modelId,
          analysisTruncated: result.truncated,
        } : entry),
      }));
      setExpandedKnowledge((current) => ({ ...current, [item.id]: true }));
      notify(result.truncated ? "文档已解析（内容较长，只取了前一部分）。" : "文档已解析，摘要已加入项目知识。", "success");
      publishAiTaskActivity({ running: false, status: "done", actionId: `project-knowledge-${item.id}`, title: "文档解析完成", detail: `${result.documents} 个文件 · ${result.modelId}`, progress: 100 });
    } catch (error) {
      // 兜底：解析流程自身已处理可预期失败，这里只防止意外异常静默丢弃。
      const detail = error instanceof Error ? error.message : String(error ?? "");
      notify(detail.trim() || "文档解析失败，请重试。", "error");
      publishAiTaskActivity({ running: false, status: "error", actionId: `project-knowledge-${item.id}`, title: "文档解析未完成", detail: detail.trim() || undefined });
    } finally {
      setAnalyzingKnowledge((current) => {
        const next = { ...current };
        delete next[item.id];
        return next;
      });
    }
  };

  const removeCommand = (commandId: string) => confirmAction({ title: "删除命令模板", message: `确认删除「${selectedProject.commands.find((item) => item.id === commandId)?.name || commandId}」？`, onConfirm: () => updateSelectedProject((project) => ({ ...project, commands: project.commands.filter((item) => item.id !== commandId) })) });
  const removeWorkflow = (workflowId: string) => confirmAction({ title: "删除自动化流程", message: `确认删除「${selectedProject.workflows.find((item) => item.id === workflowId)?.name || workflowId}」？`, onConfirm: () => updateSelectedProject((project) => ({ ...project, workflows: project.workflows.filter((item) => item.id !== workflowId) })) });
  const removeKnowledge = (knowledgeId: string) => confirmAction({ title: "移除知识文档", message: `确认移除「${selectedProject.knowledge.find((item) => item.id === knowledgeId)?.path || knowledgeId}」的关联？不会删除原始文档。`, confirmLabel: "确认移除", onConfirm: () => updateSelectedProject((project) => ({ ...project, knowledge: project.knowledge.filter((item) => item.id !== knowledgeId) })) });

  const renderOverview = () => <>
    <div className="project-automation-overview-grid">
      <div className="project-automation-summary-card is-primary">
        <span>当前项目</span>
        <strong>{selectedProject.name}</strong>
        <small>{selectedProject.description}</small>
        <div className="project-automation-summary-stats" aria-label="项目配置统计">
          <span><b>{selectedProject.commands.length}</b> 条命令</span>
          <span><b>{selectedProject.workflows.length}</b> 条流程</span>
          <span><b>{selectedProject.knowledge.length}</b> 份文档</span>
        </div>
      </div>
    </div>
    <div className="setting-group-title">项目工作说明</div>
    <div className="setting-card-group project-automation-form-card project-automation-context-card">
      <p>直接写自然语言即可，例如项目用途、Gradle/Yocto/AOSP 路径、编译约定和需要优先遵守的内部规范。不限制类型，也不要求填写 JSON。</p>
      <textarea ref={(element) => { textareaRefs.current.instructions = element; }} rows={3} className="desk-input project-automation-textarea" value={projectInstructions} aria-label="项目工作说明" placeholder="例如：这是一个 Android App 项目，使用 Gradle Wrapper 构建；需要打包 APK 时优先执行 ./gradlew assembleDebug。也可以写入 Yocto、AOSP 或公司的自定义流程。" onChange={(event) => { setProjectInstructions(event.target.value); resizeProjectTextarea("instructions", event.currentTarget); }} />
      <div className="project-automation-form-actions"><span>保存后会作为当前项目的上下文来源，供对话识别和后续调度使用。</span><Button size="sm" variant="secondary" icon={<Save size={14} />} onClick={saveProjectInstructions}>保存说明</Button></div>
    </div>
    <div className="setting-group-title">项目能力</div>
    <div className="project-automation-card-grid">
      {tabs.slice(1).map((tab) => {
        const Icon = tab.icon;
        const count = tab.id === "commands" ? selectedProject.commands.length : tab.id === "workflows" ? selectedProject.workflows.length : tab.id === "knowledge" ? selectedProject.knowledge.length : 0;
        return <button type="button" className="project-automation-entry" key={tab.id} onClick={() => setActiveTab(tab.id)}><span className="project-automation-entry-icon"><Icon size={16} /></span><span><b>{tab.label}</b><small>{tab.id === "connections" ? "关联 SSH、WSL 和设备环境" : tab.id === "commands" ? "保存 ADB、Fastboot 和 Shell 指令" : tab.id === "workflows" ? "编排编译、打包和诊断流程" : "导入项目 Markdown 和规范文档"}</small></span><strong>{count}</strong><ArrowUpRight size={14} /></button>;
      })}
    </div>
  </>;

  const renderConnections = () => <>
    <div className="setting-group-title">连接环境</div>
    <div className="setting-card-group project-automation-empty-card"><span className="project-automation-entry-icon"><Server size={17} /></span><div><b>管理执行环境</b><p>SSH 和 WSL 连接由各自的设置页面维护。执行任务前，请在对话中选择环境；当前项目配置不会自动切换连接。</p></div><Button size="sm" variant="secondary" onClick={() => window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: "ssh" } }))}>管理 SSH</Button></div>
    <div className="setting-card-group project-automation-empty-card"><span className="project-automation-entry-icon"><GitBranch size={17} /></span><div><b>当前关联范围</b><p>{selectedProject.id === "general" ? "通用项目会继承所有已启用的连接能力。" : `项目“${selectedProject.name}”的连接绑定将在验证后用于对话调度。`}</p></div><Badge variant="neutral">待配置</Badge></div>
  </>;

  const renderCommands = () => <>
    <div className="setting-group-title">新建命令模板</div>
    <div className="setting-card-group project-automation-form-card">
      <div className="project-automation-form-grid"><Input size="sm" value={commandName} placeholder="例如：打包 Debug APK" aria-label="命令名称" onChange={(event) => setCommandName(event.target.value)} /><Input size="sm" value={commandTarget} placeholder="能力或运行目标，例如 Gradle、Yocto、ADB" aria-label="能力或运行目标" onChange={(event) => setCommandTarget(event.target.value)} /></div>
      <textarea ref={(element) => { textareaRefs.current.command = element; }} rows={3} className="desk-input project-automation-textarea" value={commandText} aria-label="执行内容" placeholder="填写一条受控命令或动作说明，例如 adb shell cat /etc/os-release" onChange={(event) => { setCommandText(event.target.value); resizeProjectTextarea("command", event.currentTarget); }} />
      <div className="project-automation-form-actions"><span>保存后可被当前项目的对话和自动化流程复用。</span><Button size="sm" variant="primary" icon={<Plus size={14} />} onClick={addCommand}>保存命令</Button></div>
    </div>
    <div className="setting-group-title">已保存命令 ({selectedProject.commands.length})</div>
    <div className="project-automation-list">{selectedProject.commands.length ? selectedProject.commands.map((item) => <div className="project-automation-list-item" key={item.id}><span className="project-automation-entry-icon"><TerminalSquare size={15} /></span><div><b>{item.name}</b><small>{item.target} · {item.command}</small></div><button type="button" className="icon-action-btn danger" title="删除命令" aria-label="删除命令" onClick={() => removeCommand(item.id)}><Trash2 size={13} /></button></div>) : <div className="project-automation-placeholder">当前项目还没有命令模板。</div>}</div>
  </>;

  const renderWorkflows = () => <>
    <div className="setting-group-title">新建自动化流程</div>
    <div className="setting-card-group project-automation-form-card"><Input size="sm" value={workflowName} placeholder="例如：编译后收集启动日志" aria-label="流程名称" onChange={(event) => setWorkflowName(event.target.value)} /><textarea ref={(element) => { textareaRefs.current.workflow = element; }} rows={3} className="desk-input project-automation-textarea" value={workflowSteps} aria-label="流程步骤" placeholder="每行一个步骤，例如：\n1. 检查设备连接\n2. 执行安装\n3. 抓取 Logcat" onChange={(event) => { setWorkflowSteps(event.target.value); resizeProjectTextarea("workflow", event.currentTarget); }} /><div className="project-automation-form-actions"><span>保存后，在对话中提出任务，由 AI 按步骤调用工具。</span><Button size="sm" variant="primary" icon={<Plus size={14} />} onClick={addWorkflow}>保存流程</Button></div></div>
    <div className="setting-group-title">已保存流程 ({selectedProject.workflows.length})</div>
    <div className="project-automation-list">{selectedProject.workflows.length ? selectedProject.workflows.map((item) => <div className="project-automation-list-item" key={item.id}><span className="project-automation-entry-icon"><Workflow size={15} /></span><div><b>{item.name}</b><small>{item.steps.split("\n")[0]}</small></div><button type="button" className="icon-action-btn danger" title="删除流程" aria-label="删除流程" onClick={() => removeWorkflow(item.id)}><Trash2 size={13} /></button></div>) : <div className="project-automation-placeholder">当前项目还没有自动化流程。</div>}</div>
  </>;

  const renderKnowledge = () => <>
    <div className="setting-group-title">导入项目知识</div>
    <div className="setting-card-group project-automation-import-card"><div><b>关联项目文档</b><p>导入后点「分析」让 AI 读取文档并生成项目知识摘要；摘要会随项目配置一起进入后续对话。没有配置模型供应商时会提示先到「Agent 接入」里配置。</p></div><div className="project-automation-form-actions"><Button size="sm" variant="secondary" icon={<FolderOpen size={14} />} onClick={() => void importKnowledge(true)}>选择目录</Button><Button size="sm" variant="primary" icon={<Upload size={14} />} onClick={() => void importKnowledge(false)}>选择文档</Button></div></div>
    <div className="setting-group-title">项目文档 ({selectedProject.knowledge.length})</div>
    <div className="project-automation-list">{selectedProject.knowledge.length ? selectedProject.knowledge.map((item) => {
      const busy = Boolean(analyzingKnowledge[item.id]);
      const analyzed = Boolean(item.analysis) && item.status !== "待分析";
      const expanded = Boolean(expandedKnowledge[item.id]);
      const status: { label: string; variant: "warning" | "primary" | "success" | "neutral"; pulse?: boolean } = busy
        ? { label: "解析中", variant: "primary", pulse: true }
        : item.status === "待分析"
          ? { label: "待分析", variant: "warning" }
          : analyzed || item.status === "已解析"
            ? { label: "已解析", variant: "success" }
            : { label: "已加入项目", variant: "neutral" };
      // 旧数据（已加入项目）没有摘要，按钮仍叫「分析」，只有真的解析过才叫「重新分析」。
      const actionLabel = analyzed ? "重新分析" : "分析";
      return <div className="project-automation-knowledge-item" key={item.id}>
        <div className="project-automation-list-item">
          <span className="project-automation-entry-icon"><FileText size={15} /></span>
          <div><b>{item.path.split(/[\\/]/).filter(Boolean).pop() || item.path}</b><small>{item.path}</small></div>
          <Badge variant={status.variant} pulse={status.pulse}>{status.label}</Badge>
          <Button size="xs" variant={analyzed ? "subtle" : "primary"} icon={<Sparkles size={12} />} loading={busy} title={analyzed ? "重新解析" : "让 AI 解析这份文档"} onClick={() => void analyzeKnowledge(item)}>{actionLabel}</Button>
          {analyzed && <button type="button" className="icon-action-btn" title={expanded ? "收起解析结果" : "查看解析结果"} aria-label={expanded ? "收起解析结果" : "查看解析结果"} onClick={() => setExpandedKnowledge((current) => { const next = { ...current }; if (next[item.id]) delete next[item.id]; else next[item.id] = true; return next; })}>{expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button>}
          <button type="button" className="icon-action-btn danger" title="移除文档" aria-label="移除文档" onClick={() => removeKnowledge(item.id)}><Trash2 size={13} /></button>
        </div>
        {analyzed && expanded && <div className="project-automation-knowledge-analysis">
          {item.analysis}
          {item.analysisTruncated && <span className="project-automation-knowledge-note">内容较长，解析时只提供了前一部分。</span>}
          {item.analysisModel && <span className="project-automation-knowledge-note">{item.analysisModel}{item.analyzedAt ? ` · ${new Date(item.analyzedAt).toLocaleString("zh-CN")}` : ""}</span>}
        </div>}
      </div>;
    }) : <div className="project-automation-placeholder">还没有导入项目文档。</div>}</div>
  </>;

  return <section className="settings-pane project-automation-pane">
    {confirmationDialog}
    <header className="pane-header project-automation-header"><div><h2 className="pane-title">项目与自动化</h2><p className="project-automation-intro">在对话输入框的加号菜单中勾选项目，说明、命令和流程会与通用配置一起加入后续对话；通用配置默认启用。尚未勾选时使用此处选中的项目。发送“按某流程执行”后由 AI 调用工具，并按需加载 Skill。保存不会立即执行，也不会定时运行。</p></div><Boxes size={22} /></header>
    <div className="project-automation-project-bar"><div className="project-automation-project-select"><span className="project-automation-label">当前项目</span><Select size="sm" value={selectedProject.id} onChange={(value) => { setSelectedProjectId(value); setActiveTab("overview"); }} options={projectOptions} /></div><div className="project-automation-project-actions"><Badge variant={selectedProject.id === "general" ? "neutral" : "primary"}>{selectedProject.id === "general" ? "公共能力" : "项目范围"}</Badge>{selectedProject.id !== DEFAULT_PROJECT.id && <button type="button" className="icon-action-btn danger" title="删除当前项目" aria-label="删除当前项目" onClick={deleteSelectedProject}><Trash2 size={14} /></button>}<Button size="sm" variant="secondary" icon={<Plus size={14} />} onClick={() => setNewProjectOpen((value) => !value)}>新建项目</Button></div></div>
    {newProjectOpen && <div className="setting-card-group project-automation-new-project"><Input size="sm" value={newProjectName} placeholder="项目名称，例如 MTK8676 3OS" aria-label="项目名称" onChange={(event) => setNewProjectName(event.target.value)} /><Input size="sm" value={newProjectDescription} placeholder="项目说明，可选" aria-label="项目说明" onChange={(event) => setNewProjectDescription(event.target.value)} /><Button size="sm" variant="primary" onClick={createProject}>创建项目</Button></div>}
    {activeTab !== "overview" && <div className="project-automation-subpage-header"><button type="button" className="project-automation-back" onClick={() => setActiveTab("overview")}><ArrowLeft size={14} />返回项目概览</button><span>{tabs.find((tab) => tab.id === activeTab)?.label}</span></div>}
    <div className="project-automation-content">{activeTab === "overview" && renderOverview()}{activeTab === "connections" && renderConnections()}{activeTab === "commands" && renderCommands()}{activeTab === "workflows" && renderWorkflows()}{activeTab === "knowledge" && renderKnowledge()}</div>
  </section>;
};

export default ProjectAutomationPanel;
