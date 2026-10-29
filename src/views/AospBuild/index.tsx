import { readToolWindowState, useToolWindowState } from "@/platform/toolWindowState";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import React, { useState, useEffect, useRef, useSyncExternalStore } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { SettingsEditDialog } from "@/components/SettingsModal/SettingsEditDialog";
import { createPortal } from "react-dom";
import {
  Hammer,
  Terminal,
  Server,
  CheckCircle2,
  AlertTriangle,
  Search,
  Play,
  Square,
  PackageSearch,
  ListFilter,
  FileText,
  Copy,
  Smartphone,
  FolderCheck,
  FolderOpen,
  Download,
  Layers,
  Cpu,
  Boxes,
  Pencil,
  Trash2,
  LoaderCircle,
  Plug,
  Unplug,
  RefreshCw,
  Settings,
} from "lucide-react";
import { Button, Input, Badge, Select } from "@/components/ui";
import { formatInvokeError, invoke, isTauri, listAdbDevices, listAndroidTasks, waitForTask } from "@/platform/runtime";
import { listWslProfiles, type WslProfile } from "@/platform/api";
import { getGlobalEnvironmentTarget, openGlobalSshMenu, useGlobalEnvironment } from "@/platform/globalEnvironment";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { ChatTerminalPanel, type TerminalTarget } from "@/views/Chat/ChatTerminalPanel";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity } from "@/platform/notifications";
import { logLineClass } from "@/platform/logHighlight";
import { setRemoteAutoConnectPaused } from "@/platform/remoteConnectionPolicy";
import { useAospConnection } from "./useAospConnection";
import "./aosp-build.css";
import { appendAospOutput, getAospRun, lunchWasPackaged, parseLunchMenu, patchAospRun, subscribeAospRun, type AospBusyAction, type AospLunchTarget } from "./session";
import { AOSP_PREFERENCES_KEY, readAospPreferences, rememberAospWorkspace, restoreAospWorkspace, workspaceKey, type SavedAospCommand } from "./preferences";

export interface RemoteTarget {
  id: string;
  name: string;
  defaultRoot?: string;
  distribution?: string;
}

export interface AospProbe {
  linux: boolean;
  bashAvailable: boolean;
  envsetupPath?: string;
  soongPath?: string;
  manifestPath?: string;
  androidBpCount?: number | null;
  gitBranch?: string | null;
  ccacheStatus?: string | null;
  output: string;
}

export interface AospTask {
  id: string;
  status: "idle" | "running" | "done" | "error" | "cancelled";
  progress: number;
  message: string;
  elapsedSeconds: number;
  error?: string;
}

export interface AospArtifact {
  name: string;
  path: string;
  size: number;
  kind: "apk" | "img" | "jar" | "bin";
  installable: boolean;
  installNote?: string;
}

interface AospWindowSnapshot {
  kind: "wsl" | "ssh"; selectedTargetId: string; root: string;
  tab: "overview" | "lunch" | "terminal" | "console" | "artifacts";
  probe: AospProbe | null; lunch: string; moduleName: string; commandDraft: string;
  commandName: string; buildMode: "lunch" | "custom"; artifacts: AospArtifact[];
}

const AOSP_WORKSPACE_SELECTION_STORAGE_KEY = "android-tool.aosp-workspace-selection.v1";

function toRemoteTarget(item: { id: string; name: string; defaultRoot?: string | null; distribution?: string }): RemoteTarget {
  return { id: item.id, name: item.name, defaultRoot: item.defaultRoot || undefined, distribution: item.distribution };
}

export const AospBuildView: React.FC<{ active?: boolean; onBack?: () => void }> = ({ active = true, onBack }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const desktop = isTauri();
  const restored = useRef(readToolWindowState<AospWindowSnapshot>("aosp")).current;
  const environment = useGlobalEnvironment();
  const globalSshId = environment.target.kind === "ssh" ? environment.target.id || "" : "";
  // 1. 环境与目标选择
  const [preferences, setPreferences] = useState(readAospPreferences);
  const preferencesRef = useRef(preferences);
  preferencesRef.current = preferences;
  const initialWorkspace = restoreAospWorkspace(preferences, preferences.kind, []);
  const [kind, setKind] = useState<"wsl" | "ssh">(restored?.kind || preferences.kind);
  const [selectedTargetId, setTargetId] = useState<string>(restored?.selectedTargetId || preferences.lastTargets[preferences.kind] || initialWorkspace.id);
  const targetId = kind === "ssh" ? globalSshId : selectedTargetId;
  const [root, setRoot] = useState<string>(restored?.root ?? preferences.roots[workspaceKey(preferences.kind, selectedTargetId)] ?? initialWorkspace.root);
  const [sshRootTargetId, setSshRootTargetId] = useState<string | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;
  const [targetsLoaded, setTargetsLoaded] = useState(!desktop);
  const [configurationError, setConfigurationError] = useState("");
  const [configurationRevision, setConfigurationRevision] = useState(0);
  const run = useSyncExternalStore(subscribeAospRun, getAospRun);
  const busyAction = run.busyAction;
  const busy = busyAction !== null;
  const setBusy = (on: boolean, action: AospBusyAction = "probe") => {
    patchAospRun({ busyAction: on ? action : null });
  };

  // 2. 预设目标配置
  const [wslProfiles, setWslProfiles] = useState<WslProfile[]>([]);
  const sshConnections = environment.ssh.connections;
  const [wslTargets, setWslTargets] = useState<RemoteTarget[]>([]);
  const sshTargets = sshConnections.map(toRemoteTarget);

  const currentTargets = kind === "wsl" ? wslTargets : sshTargets;
  const connection = useAospConnection(kind, targetId, desktop && active && targetsLoaded && currentTargets.some((target) => target.id === targetId));
  const connectionReady = connection.phase === "connected" && (kind !== "ssh" || sshRootTargetId === targetId);
  const [dataError, setDataError] = useState("");
  const autoInspectRef = useRef("");
  const workspaceRef = useRef({ targetKind: kind, targetId, root: root.trim() });
  workspaceRef.current = { targetKind: kind, targetId, root: root.trim() };

  // 3. Tab 状态
  const [tab, setTab] = useState<"overview" | "lunch" | "terminal" | "console" | "artifacts">(restored?.tab || "overview");

  // 4. 源码树探测信息
  const [probe, setProbe] = useState<AospProbe | null>(restored?.probe || null);

  // 5. 编译目标参数
  const [lunch, setLunch] = useState<string>(restored?.lunch ?? preferences.lunch);
  const [moduleName, setModuleName] = useState<string>(restored?.moduleName ?? preferences.module);
  const [commandDraft, setCommandDraft] = useState(restored?.commandDraft ?? preferences.commandDraft);
  const [commandName, setCommandName] = useState(restored?.commandName ?? preferences.commandName);
  const [selectedCommandId, setSelectedCommandId] = useState("");
  const [editingCommand, setEditingCommand] = useState<SavedAospCommand | null>(null);
  const [buildMode, setBuildMode] = useState<"lunch" | "custom">(restored?.buildMode || "lunch");
  const consoleRef = useRef<HTMLPreElement>(null);
  const followLogsRef = useRef(true);

  // 6. 任务状态与日志
  const task = run.task;
  const setTask = (value: AospTask | ((prev: AospTask) => AospTask)) => {
    const prev = getAospRun().task;
    patchAospRun({ task: typeof value === "function" ? value(prev) : value });
  };
  const lunchTargets = run.lunchTargets;
  const setLunchTargets = (targets: AospLunchTarget[]) => patchAospRun({ lunchTargets: targets });

  const logs = run.logs;
  const setLogs = (value: string[] | ((prev: string[]) => string[])) => {
    const prev = getAospRun().logs;
    patchAospRun({ logs: (typeof value === "function" ? value(prev) : value).slice(-3000) });
  };

  // 7. 产物列表
  const [artifacts, setArtifacts] = useState<AospArtifact[]>(restored?.artifacts || []);

  useToolWindowState("aosp", () => {
    const current = getAospRun();
    if (current.busyAction && (current.busyAction !== "build" || current.task.status !== "running")) throw new Error("当前操作正在执行，请完成后再拖出。");
    return { kind, selectedTargetId, root, tab, probe, lunch, moduleName, commandDraft, commandName, buildMode, artifacts };
  });
  useToolWindowState("aosp-run", () => ({ ...getAospRun(), logs: getAospRun().logs.slice(-1000).map((line) => line.slice(-2000)) }));

  // 8. 设备连接
  const [pickerPath, setPickerPath] = useState<string | null>(null);
  const [pickerError, setPickerError] = useState("");
  const [directories, setDirectories] = useState<string[]>([]);
  const [listingDirs, setListingDirs] = useState(false);
  const [selectedDevice, setSelectedDevice] = useState<string>("");
  const [deviceOptions, setDeviceOptions] = useState<Array<{ serial: string; label: string }>>(
    []
  );

  const showToast = (msg: string) => notify(msg, "info");

  const lastDirEnterAt = useRef(0);
  const agentTargetRequestedRef = useRef(false);
  const pendingAgentBuildRef = useRef<{ lunch: string; module: string; actionId: string } | null>(null);

  useApplicationPageAction("open.tool.aosp", (action) => {
    const parameters = action.parameters;
    agentTargetRequestedRef.current = true;
    if (parameters.targetKind === "ssh" || parameters.targetKind === "wsl") setKind(parameters.targetKind);
    if (parameters.targetId !== undefined) setTargetId(parameters.targetId);
    if (parameters.workspaceRoot !== undefined) setRoot(parameters.workspaceRoot);
    if (parameters.lunchTarget !== undefined) setLunch(parameters.lunchTarget);
    if (parameters.moduleName !== undefined) setModuleName(parameters.moduleName);
    setTab("overview");
    if (parameters.operation === "build") {
      pendingAgentBuildRef.current = {
        lunch: parameters.lunchTarget || lunch,
        module: parameters.moduleName || moduleName,
        actionId: action.actionId,
      };
      publishAiTaskActivity({ running: true, status: "running", title: "正在校验 AOSP 源码树", detail: "AI 已交接模块编译参数", actionId: action.actionId });
      window.setTimeout(() => { void handleInspectWorkspace({
        targetKind: parameters.targetKind === "ssh" || parameters.targetKind === "wsl" ? parameters.targetKind : kind,
        targetId: parameters.targetId || targetId,
        root: parameters.workspaceRoot || root,
      }); }, 0);
    } else if (parameters.operation === "inspect") {
      window.setTimeout(() => { void handleInspectWorkspace({
        targetKind: parameters.targetKind === "ssh" || parameters.targetKind === "wsl" ? parameters.targetKind : kind,
        targetId: parameters.targetId || targetId,
        root: parameters.workspaceRoot || root,
      }); }, 0);
    }
  });

  useEffect(() => {
    if (!desktop || !active) return;
    let disposed = false;
    setConfigurationError("");
    void (async () => {
      try {
        const [profiles, devices] = await Promise.all([
          listWslProfiles(),
          listAdbDevices(),
        ]);
        if (disposed) return;
        setWslProfiles(profiles);
        setWslTargets(profiles.map(toRemoteTarget));
        const online = devices.filter((item) => item.state === "device");
        setDeviceOptions(online.map((item) => ({ serial: item.serial, label: item.model || item.product || item.serial })));
        if (!agentTargetRequestedRef.current && !restored) {
          const saved = preferencesRef.current;
          const globalTarget = getGlobalEnvironmentTarget().target;
          if (globalTarget.kind !== "ssh" && saved.kind === "wsl") {
            const restored = restoreAospWorkspace(saved, "wsl", profiles.map(toRemoteTarget));
            setKind("wsl");
            setTargetId(restored.id);
            setRoot(restored.root);
          }
        }
        setTargetsLoaded(true);
        if (online[0]) setSelectedDevice(online[0].serial);
      } catch (reason) {
        if (!disposed) setConfigurationError(formatInvokeError(reason));
      } finally {
        if (!disposed) setTargetsLoaded(true);
      }
    })();
    return () => { disposed = true; };
  }, [desktop, active, configurationRevision]);

  useEffect(() => {
    if (active && globalSshId) setKind("ssh");
  }, [active, globalSshId]);

  useEffect(() => {
    if (kind !== "ssh") return;
    setRoot(restored?.kind === "ssh" && targetId === restored.selectedTargetId ? restored.root : targetId ? preferencesRef.current.roots[workspaceKey("ssh", targetId)] ?? environment.ssh.connection?.defaultRoot ?? "" : "");
    setSshRootTargetId(targetId);
  }, [kind, targetId]);

  useEffect(() => {
    if (!targetsLoaded || !targetId || (kind === "ssh" && sshRootTargetId !== targetId)) return;
    setPreferences((previous) => rememberAospWorkspace(previous, kind, targetId, root));
  }, [kind, targetId, root, targetsLoaded, sshRootTargetId]);

  useEffect(() => {
    try {
      window.localStorage.setItem(AOSP_PREFERENCES_KEY, JSON.stringify({ ...preferences, lunch, module: moduleName, commandDraft, commandName }));
    } catch { /* Continue using in-memory preferences when storage is unavailable. */ }
  }, [preferences, lunch, moduleName, commandDraft, commandName]);

  useEffect(() => {
    if (restored && kind === restored.kind && targetId === restored.selectedTargetId && root === restored.root) return;
    setProbe(null);
    setArtifacts([]);
    setLunchTargets([]);
    setDataError("");
  }, [desktop, kind, targetId, root]);

  useEffect(() => {
    if (active && tab === "console" && followLogsRef.current && consoleRef.current) {
      consoleRef.current.scrollTop = consoleRef.current.scrollHeight;
    }
  }, [active, tab, logs]);

  useEffect(() => {
    if (!desktop || task.id === "task-init") return;
    const id = task.id;
    let disposed = false;
    let timer: number | undefined;
    let reportedError = false;
    const readOutput = async () => {
      try {
        const current = getAospRun();
        if (current.task.id !== id) return;
        const output = await invoke<{ text: string; nextOffset: number }>("read_aosp_live_output", { taskId: id, offset: current.logOffset });
        if (disposed || getAospRun().task.id !== id) return;
        if (output.text) patchAospRun({ logs: appendAospOutput(getAospRun().logs, output.text), logOffset: output.nextOffset });
        if (!output.text && getAospRun().task.status !== "running") return;
      } catch (reason) {
        if (disposed) return;
        if (!reportedError) { showToast(`实时日志读取失败：${formatInvokeError(reason)}`); reportedError = true; }
        if (getAospRun().task.status !== "running") return;
      }
      if (!disposed) timer = window.setTimeout(() => void readOutput(), 250);
    };
    void readOutput();
    return () => { disposed = true; if (timer !== undefined) window.clearTimeout(timer); };
  }, [desktop, task.id, task.status]);


  // A detached viewer follows the existing backend task without starting a new build.
  useEffect(() => {
    if (!desktop || !restored || task.status !== "running") return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = (await listAndroidTasks()).find((item) => item.id === task.id);
        if (disposed || !next) return;
        const running = next.status === "running" || next.status === "queued";
        patchAospRun({ busyAction: running ? "build" : null, task: {
          id: next.id, status: running ? "running" : next.status === "done" ? "done" : next.status === "cancelled" ? "cancelled" : "error",
          progress: next.progress, message: next.message,
          elapsedSeconds: Math.max(0, Math.floor(((next.finishedAt || Date.now()) - next.startedAt) / 1000)), error: next.error || undefined,
        } });
        if (!running) return;
      } catch (error) { if (!disposed) setDataError(formatInvokeError(error)); }
      if (!disposed) timer = setTimeout(() => void poll(), 800);
    };
    void poll();
    return () => { disposed = true; clearTimeout(timer); };
  }, [desktop, task.id, task.status]);

  // 切换目标
  const handleSelectTarget = (id: string) => {
    setRemoteAutoConnectPaused(kind, id, false);
    setTargetId(id);
    const found = currentTargets.find((t) => t.id === id);
    setRoot(preferences.roots[workspaceKey(kind, id)] ?? found?.defaultRoot ?? "");
    showToast(`已切换目标环境至：${found?.name || id}`);
  };

  const handleSwitchTargetKind = (nextKind: "wsl" | "ssh") => {
    if (nextKind === kind || busy || task.status === "running") return;
    setKind(nextKind);
    if (nextKind === "ssh") {
      if (!globalSshId) openGlobalSshMenu();
      return;
    }
    const next = restoreAospWorkspace(preferences, nextKind, nextKind === "wsl" ? wslTargets : sshTargets);
    setTargetId(next.id);
    setRoot(next.root);
  };

  const interactiveTerminalTarget: TerminalTarget = {
    kind,
    id: targetId || undefined,
    label: `${kind === "wsl" ? "WSL" : "SSH"} · ${currentTargets.find((item) => item.id === targetId)?.name || "未选择目标"}`,
    root: root.trim() || undefined,
  };

  const handleSaveCommand = () => {
    const command = commandDraft.trim();
    if (!command) { showToast("请先输入自定义命令。"); return; }
    const id = selectedCommandId || preferences.commands.find((item) => item.command === command)?.id || crypto.randomUUID();
    const entry = { id, name: commandName.trim() || command.split("\n")[0].slice(0, 64), command };
    setPreferences((previous) => ({ ...previous, commands: [...previous.commands.filter((item) => item.id !== id), entry] }));
    setSelectedCommandId(id);
    showToast("自定义命令已保存。");
  };

  const handleSaveCommandEdit = () => {
    if (!editingCommand) return;
    const command = editingCommand.command.trim();
    if (!command) { showToast("请先输入自定义命令。"); return; }
    const entry = { ...editingCommand, command, name: editingCommand.name.trim() || command.split("\n")[0].slice(0, 64) };
    setPreferences((previous) => ({ ...previous, commands: previous.commands.map((item) => item.id === entry.id ? entry : item) }));
    if (selectedCommandId === entry.id) { setCommandName(entry.name); setCommandDraft(entry.command); }
    setEditingCommand(null);
    showToast("自定义命令已更新。");
  };

  // 验证源码树
  const linuxRequest = () => ({ targetKind: kind, targetId, root: root.trim() });

  const handleInspectWorkspace = async (requestOverride?: { targetKind: "wsl" | "ssh"; targetId: string; root: string }, quiet = false) => {
    const request = requestOverride || linuxRequest();
    if (!desktop) {
      showToast("请在桌面应用中连接 Linux 目标并验证源码树。");
      return;
    }
    if (!request.targetId || !request.root.trim()) {
      showToast("请选择 Linux 目标和工作目录，再验证源码树。自定义命令可直接在当前目录执行。");
      return;
    }
    if (!await connection.ensure({ kind: request.targetKind, id: request.targetId })) {
      if (!quiet) showToast("Linux 目标未连接，请先连接或重试。");
      const pending = pendingAgentBuildRef.current;
      if (pending) { pendingAgentBuildRef.current = null; publishAiTaskActivity({ running: false, status: "error", title: "AOSP 连接失败", detail: "请连接指定的 Linux 目标后再执行编译。", actionId: pending.actionId }); }
      return;
    }
    setBusy(true, "probe");
    try {
      const next = await invoke<AospProbe>("probe_aosp_linux_workspace", { request });
      const current = workspaceRef.current;
      if (current.targetKind !== request.targetKind || current.targetId !== request.targetId || current.root !== request.root.trim()) return;
      setProbe(next);
      setDataError("");
      if (next.linux && next.bashAvailable && next.envsetupPath && next.soongPath && next.manifestPath && request.root.trim()) {
        try {
          window.localStorage.setItem(AOSP_WORKSPACE_SELECTION_STORAGE_KEY, JSON.stringify({
            targetKind: request.targetKind,
            targetId: request.targetId,
            workspaceRoot: request.root.trim(),
          }));
        } catch {
          // The AOSP page remains usable when renderer storage is unavailable.
        }
      }
      if (!quiet) showToast(next.envsetupPath && next.soongPath ? "已确认 AOSP 源码树。" : "该目录未检测到完整 AOSP 构建入口。");
      if (pendingAgentBuildRef.current?.actionId) {
        publishAiTaskActivity({ running: true, status: "running", title: "正在启动 AOSP 模块编译", detail: `已确认源码树，准备执行 lunch ${pendingAgentBuildRef.current.lunch}`, actionId: pendingAgentBuildRef.current.actionId, progress: 12 });
      }
      return next;
    } catch (reason) {
      const message = formatInvokeError(reason);
      const current = workspaceRef.current;
      if (current.targetKind !== request.targetKind || current.targetId !== request.targetId || current.root !== request.root.trim()) return;
      setDataError(message);
      if (!quiet) showToast(message);
      const pending = pendingAgentBuildRef.current;
      if (pending) {
        pendingAgentBuildRef.current = null;
        publishAiTaskActivity({ running: false, status: "error", title: "AOSP 源码树校验失败", detail: message, actionId: pending.actionId });
      }
    } finally {
      setBusy(false);
    }
  };

  // 提交后台构建任务
  const handleStartBuild = async (agentBuild?: { lunch: string; module: string; actionId: string }, customCommand = "") => {
    const nextLunch = agentBuild?.lunch || lunch;
    const nextModule = agentBuild?.module || moduleName;
    if (getAospRun().task.status === "running" || getAospRun().busyAction === "build") return;
    const custom = customCommand.trim();
    if (!targetId || !root.trim()) { showToast("请选择 Linux 目标并填写工作目录。"); return; }
    if (desktop) {
      if (!await connection.ensure()) { showToast("Linux 目标未连接，请先连接或重试。"); return; }
      if (!custom && !probe?.envsetupPath) {
        showToast("请先验证 AOSP 源码树。");
        if (agentBuild) publishAiTaskActivity({ running: false, status: "error", title: "无法启动 AOSP 构建", detail: "源码树校验未通过。", actionId: agentBuild.actionId });
        return;
      }
      if (!custom && !nextLunch.trim()) {
        showToast("请输入明确的 lunch 目标。");
        if (agentBuild) publishAiTaskActivity({ running: false, status: "error", title: "无法启动 AOSP 构建", detail: "缺少 lunch 目标。", actionId: agentBuild.actionId });
        return;
      }
      setBusy(true, "build");
      try {
        const id = await invoke<string>("start_aosp_linux_build", {
          request: { ...linuxRequest(), lunchTarget: nextLunch, module: nextModule, customCommand: custom },
        });
        setTab("console");
        setTask({ id, status: "running", progress: 0, message: "正在准备 Linux AOSP 构建...", elapsedSeconds: 0 });
        patchAospRun({ logs: [], logOffset: 0 });
        followLogsRef.current = true;
        const done = await waitForTask(id, (next) => {
          setTask({
            id,
            status: next.status === "running" || next.status === "queued" ? "running" : next.status === "done" ? "done" : next.status === "cancelled" ? "cancelled" : "error",
            progress: next.progress,
            message: next.message,
            elapsedSeconds: Math.max(0, Math.floor(((next.finishedAt || Date.now()) - next.startedAt) / 1000)),
            error: next.error || undefined,
          });
          if (agentBuild) publishAiTaskActivity({
            running: next.status === "running",
            status: next.status === "done" ? "done" : next.status === "running" ? "running" : "error",
            title: next.status === "running" ? "正在编译 AOSP 模块" : next.status === "done" ? "AOSP 模块编译完成" : "AOSP 模块编译失败",
            detail: next.message || next.error || `lunch ${nextLunch} · ${nextModule || "全量 m"}`,
            actionId: agentBuild.actionId,
            progress: next.progress,
          });
        }, 4 * 60 * 60 * 1000 + 60_000);
        if (done.status === "done") {
          showToast("AOSP 构建结束。");
          if (agentBuild) publishAiTaskActivity({ running: false, status: "done", title: "AOSP 模块编译完成", detail: done.message || "构建任务已完成", actionId: agentBuild.actionId, progress: 100 });
        } else if (agentBuild) {
          publishAiTaskActivity({ running: false, status: "error", title: "AOSP 模块编译失败", detail: done.error || done.message || "构建任务未完成", actionId: agentBuild.actionId, progress: done.progress });
        }
      } catch (reason) {
        showToast(formatInvokeError(reason));
        setTask((previous) => ({ ...previous, status: "error", message: formatInvokeError(reason), error: formatInvokeError(reason) }));
        if (agentBuild) publishAiTaskActivity({ running: false, status: "error", title: "AOSP 模块编译失败", detail: formatInvokeError(reason), actionId: agentBuild.actionId });
      } finally {
        setBusy(false);
      }
      return;
    }
    showToast("请在桌面应用中执行 Linux 构建命令。");

  };

  useEffect(() => {
    const pending = pendingAgentBuildRef.current;
    if (!pending || busy || !probe?.envsetupPath) return;
    pendingAgentBuildRef.current = null;
    void handleStartBuild(pending);
  }, [busy, probe]);

  // 取消构建
  const handleCancelBuild = async () => {
    if (desktop && task.id && task.status === "running") {
      try {
        await invoke("cancel_android_task", { id: task.id });
      } catch (reason) {
        showToast(formatInvokeError(reason));
        return;
      }
    }
    setTask((prev) => ({
      ...prev,
      status: "cancelled",
      message: "构建任务已被用户取消",
    }));
    setLogs((prev) => [
      ...prev,
      `[终止] 用户手动终止了当前编译任务。保留已生成的中间缓存。`,
    ]);
    showToast("已取消构建任务");
  };

  const handleLoadArtifacts = async () => {
    if (!desktop) {
      showToast("请在桌面应用中扫描构建产物。");
      return;
    }
    if (!probe?.envsetupPath) {
      showToast("请先验证 AOSP 源码树。");
      return;
    }
    if (!await connection.ensure()) { showToast("Linux 目标未连接，请先连接或重试。"); return; }
    setBusy(true, "artifacts");
    try {
      const rows = await invoke<Array<{ path: string; size: number; kind: string; installable: boolean; installNote?: string }>>("list_aosp_build_artifacts", { request: linuxRequest() });
      setArtifacts(rows.map((row) => ({
        name: row.path.split("/").filter(Boolean).pop() || row.path,
        path: row.path,
        size: row.size,
        kind: (row.kind === "apk" || row.kind === "img" || row.kind === "jar" ? row.kind : "bin"),
        installable: row.installable,
        installNote: row.installNote,
      })));
      showToast(rows.length ? `已发现 ${rows.length} 个构建产物。` : "当前源码树还没有可展示的构建产物。");
    } catch (reason) {
      showToast(formatInvokeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleInstallArtifact = async (artifact: AospArtifact) => {
    if (!desktop) {
      showToast("请在桌面应用中安装构建产物。");
      return;
    }
    if (!selectedDevice) {
      showToast("请选择用于安装的设备。");
      return;
    }
    if (!await connection.ensure()) { showToast("Linux 目标未连接，请先连接或重试。"); return; }
    setBusy(true, "install");
    try {
      const id = await invoke<string>("start_aosp_artifact_install", {
        request: { ...linuxRequest(), serial: selectedDevice, artifactPath: artifact.path, replace: true },
      });
      showToast(`正在安装 ${artifact.name}...`);
      const done = await waitForTask(id);
      showToast(done.status === "done" ? `${artifact.name} 已安装。` : done.error || done.message);
    } catch (reason) {
      showToast(formatInvokeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const joinLinuxPath = (base: string, name: string) => {
    const current = base.trim().replace(/\/+$/, "") || "/";
    if (name === "..") {
      const parts = current.split("/").filter(Boolean);
      parts.pop();
      return parts.length ? `/${parts.join("/")}` : "/";
    }
    return current === "/" ? `/${name}` : `${current}/${name}`;
  };

  const browseDirectories = async (nextRoot: string) => {
    const path = nextRoot.trim() || "/";
    setPickerPath(path);
    setPickerError("");
    if (!isTauri()) {
      setDirectories([]);
      setPickerError("目录浏览需要在桌面应用里使用。");
      return;
    }
    if (!targetId) {
      setDirectories([]);
      setPickerError("请先在左侧选择一个已保存的 WSL 或 SSH 目标。");
      return;
    }
    if (!await connection.ensure()) { setPickerError("Linux 目标未连接，请先连接或重试。"); return; }
    setListingDirs(true);
    try {
      const names = await invoke<string[]>("list_aosp_directories", {
        request: { targetKind: kind, targetId, root: path },
      });
      setDirectories(names);
    } catch (reason) {
      setDirectories([]);
      setPickerError(formatInvokeError(reason));
    } finally {
      setListingDirs(false);
    }
  };

  const handleQueryLunch = async (requestOverride?: { targetKind: "wsl" | "ssh"; targetId: string; root: string }, quiet = false) => {
    const request = requestOverride || linuxRequest();
    if (!desktop) {
      showToast("请在桌面应用中查询 Linux 源码树的 lunch 目标。");
      return;
    }
    if (!request.targetId || !request.root.trim()) {
      showToast("请选择 Linux 目标和工作目录，再验证源码树。自定义命令可直接在当前目录执行。");
      return;
    }
    if (!await connection.ensure({ kind: request.targetKind, id: request.targetId })) {
      if (!quiet) showToast("Linux 目标未连接，请先连接或重试。");
      return;
    }
    setBusy(true, "lunch");
    try {
      const catalog = await invoke<{ menu: string; built?: Array<{ output: string; product: string }> }>("list_aosp_lunch_targets", { request });
      const current = workspaceRef.current;
      if (current.targetKind !== request.targetKind || current.targetId !== request.targetId || current.root !== request.root.trim()) return;
      const built = catalog.built ?? [];
      const names = parseLunchMenu(catalog.menu);
      const targets = names.map((name) => ({
        name,
        outputs: [...new Set(built.filter((item) => lunchWasPackaged(name, item.product)).map((item) => item.output))],
      }));
      setLunchTargets(targets);
      setDataError("");
      if (targets[0] && !lunch.trim()) setLunch(targets[0].name);
      const packaged = targets.filter((item) => item.outputs.length > 0).length;
      if (!quiet) showToast(targets.length ? `已读取 ${targets.length} 个 lunch 目标，其中 ${packaged} 个已打包。` : "源码树没有返回可选 lunch 目标。");
    } catch (reason) {
      const current = workspaceRef.current;
      if (current.targetKind !== request.targetKind || current.targetId !== request.targetId || current.root !== request.root.trim()) return;
      setDataError(formatInvokeError(reason));
      if (!quiet) showToast(formatInvokeError(reason));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!connectionReady) { autoInspectRef.current = ""; return; }
    if (!active || !desktop || busy || task.status === "running" || !root.trim()) return;
    const request = linuxRequest();
    const key = JSON.stringify(request);
    if (autoInspectRef.current === key) return;
    const timer = window.setTimeout(() => {
      autoInspectRef.current = key;
      void (async () => {
        const next = await handleInspectWorkspace(request, true);
        const current = workspaceRef.current;
        if (!pendingAgentBuildRef.current && activeRef.current && getAospRun().task.status !== "running" && next?.envsetupPath && next.soongPath && current.targetKind === request.targetKind && current.targetId === request.targetId && current.root === request.root) await handleQueryLunch(request, true);
      })();
    }, 600);
    return () => window.clearTimeout(timer);
  }, [active, desktop, connectionReady, kind, targetId, root, busy, task.status]);

  // 格式化文件大小
  const formatBytes = (bytes: number) => {
    if (bytes >= 1024 * 1024 * 1024) return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
    if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
    return Math.max(1, Math.ceil(bytes / 1024)) + " KB";
  };

  // 遥测属性项目
  const probeProperties: PropertyItem[] = [
    {
      icon: <Terminal size={14} />,
      label: "执行宿主环境",
      value: targetId ? (kind === "wsl" ? "WSL Linux" : "SSH 远程 Linux") : "-",
      valueType: "accent",
    },
    {
      icon: <Server size={14} />,
      label: "Linux 目标实例",
      value: currentTargets.find((t) => t.id === targetId)?.name || "-",
    },
    {
      icon: <FolderCheck size={14} />,
      label: "AOSP 根目录",
      value: root || "-",
      valueType: "default",
    },
    {
      icon: <CheckCircle2 size={14} />,
      label: "环境引导脚本",
      value: probe?.envsetupPath || "未检测",
      valueType: probe?.envsetupPath ? "green" : "orange",
    },
    {
      icon: <Cpu size={14} />,
      label: "Soong 构建引擎",
      value: probe?.soongPath || "未检测",
      valueType: probe?.soongPath ? "green" : "orange",
    },
    {
      icon: <Boxes size={14} />,
      label: "Android.bp 文件数量",
      value: probe?.androidBpCount != null ? `${probe.androidBpCount.toLocaleString()} 个` : "未检测",
      valueType: probe?.androidBpCount != null ? "default" : "orange",
    },
    {
      icon: <Layers size={14} />,
      label: "Git 仓库分支",
      value: probe?.gitBranch || "未检测",
      valueType: probe?.gitBranch ? "default" : "orange",
    },
    {
      icon: <CheckCircle2 size={14} />,
      label: "编译缓存状态",
      value: probe?.ccacheStatus ? <span className="aosp-cache-stats">{probe.ccacheStatus}</span> : "未检测",
      valueType: probe?.ccacheStatus ? "default" : "orange",
    },
  ];

  return (
    <div className="aosp-container">
      {confirmationDialog}
      {editingCommand && <SettingsEditDialog title="编辑自定义命令" onClose={() => setEditingCommand(null)} className="aosp-command-dialog">
        <form className="aosp-custom-commands" onSubmit={(event) => { event.preventDefault(); handleSaveCommandEdit(); }}>
          <div className="aosp-build-field">
            <label htmlFor="aosp-edit-command-name">命令名称（可选）</label>
            <Input id="aosp-edit-command-name" size="sm" value={editingCommand.name} onChange={(event) => setEditingCommand({ ...editingCommand, name: event.target.value })} />
          </div>
          <div className="aosp-build-field">
            <label htmlFor="aosp-edit-command">构建 / 搜索命令</label>
            <textarea id="aosp-edit-command" className="aosp-command-editor" rows={7} value={editingCommand.command} onChange={(event) => setEditingCommand({ ...editingCommand, command: event.target.value })} spellCheck={false} />
          </div>
          <div className="aosp-command-dialog-actions">
            <Button type="button" size="sm" variant="secondary" onClick={() => setEditingCommand(null)}>取消</Button>
            <Button type="submit" size="sm" variant="primary" disabled={!editingCommand.command.trim()}>保存修改</Button>
          </div>
        </form>
      </SettingsEditDialog>}
      {/* 悬浮 Toast 胶囊通知 */}
      {pickerPath !== null && createPortal(
        <div className="aosp-dir-overlay" onClick={() => setPickerPath(null)}>
          <div className="aosp-dir-dialog" role="dialog" aria-modal="true" aria-label="选择源码目录" onClick={(event) => event.stopPropagation()}>
            <header className="aosp-dir-header">
              <div>
                <div className="aosp-dir-title">选择源码目录</div>
                <code className="aosp-dir-path">{pickerPath}</code>
              </div>
              <button type="button" className="aosp-dir-close" onClick={() => setPickerPath(null)} aria-label="关闭">×</button>
            </header>
            <button
              type="button"
              className="aosp-dir-parent"
              onClick={() => void browseDirectories(joinLinuxPath(pickerPath, ".."))}
            >
              上级目录
            </button>
            <div className="aosp-dir-list">
              {pickerError && <p className="aosp-dir-hint">{pickerError}</p>}
              {listingDirs && <p className="aosp-dir-hint">正在读取目录…</p>}
              {!listingDirs && !pickerError && directories.filter((name) => name !== "..").length === 0 && <p className="aosp-dir-hint">这个目录下没有子目录。</p>}
              {!listingDirs && directories.filter((name) => name !== "..").map((name) => (
                <div key={name} className="aosp-dir-row">
                  <button
                    type="button"
                    className="aosp-dir-name"
                    onClick={() => {
                      const now = Date.now();
                      if (now - lastDirEnterAt.current < 400) return;
                      lastDirEnterAt.current = now;
                      void browseDirectories(joinLinuxPath(pickerPath, name));
                    }}
                  >
                    {name}
                  </button>
                  <div className="aosp-dir-row-actions">
                    <Button size="xs" variant="primary" onClick={() => { setRoot(joinLinuxPath(pickerPath, name)); setPickerPath(null); }}>选择</Button>
                  </div>
                </div>
              ))}
            </div>
            <footer className="aosp-dir-footer">
              <Button size="sm" variant="secondary" onClick={() => setPickerPath(null)}>取消</Button>
              <Button size="sm" variant="primary" onClick={() => { setRoot(pickerPath); setPickerPath(null); }}>使用当前目录</Button>
            </footer>
          </div>
        </div>,
        document.body,
      )}

      {/* 1. 顶部 Header */}
      <header className="aosp-top-header">
        <div className="aosp-header-left">
          <ToolBackButton onBack={onBack} />
          <div className="aosp-title-wrap">
            <h1 className="aosp-title">AOSP 构建与模块编译</h1>
            <span className="aosp-subtitle">
              连接 Linux 宿主执行 envsetup 探测、lunch 目标配置与多模块增量构建
            </span>
          </div>
        </div>

        <div className="aosp-header-right">
          <Button
            variant="secondary"
            size="sm"
            loading={busyAction === "probe"}
            onClick={() => { setConfigurationRevision((value) => value + 1); autoInspectRef.current = ""; if (targetId) void connection.retry(); }}
            disabled={busy || connection.phase === "checking"}
          >
            刷新环境
          </Button>
        </div>
      </header>


      {/* 2. 顶层副工具条 (环境、目标、路径、验证) */}
      <div className="aosp-actions-bar">
        <div className="aosp-env-selector-group">
          {/* WSL / SSH 分段微切换器 */}
          <SegmentedControl value={kind} onChange={handleSwitchTargetKind} label="AOSP 环境类型"
            className="aosp-env-kind-segmented" buttonClassName="aosp-env-kind-btn" options={[
              { value: "wsl", label: "WSL Linux", disabled: busy || task.status === "running" }, { value: "ssh", label: "SSH 远程", disabled: busy || task.status === "running" },
            ]} />

          {/* 目标列表下拉 */}
          <div className="aosp-target-select">
            {kind === "ssh" ? <Button size="sm" variant="subtle" icon={<Server size={14} />} title="选择全局 SSH 连接" aria-label="选择全局 SSH 连接" onClick={openGlobalSshMenu}>
              <span className="aosp-global-ssh-label">{environment.target.kind === "ssh" ? environment.ssh.connection?.name || "SSH" : "选择全局 SSH"}</span>
            </Button> :
            <Select
              size="sm"
              value={targetId}
              placeholder={targetsLoaded ? currentTargets.length ? "请选择 Linux 目标" : "暂无已保存的目标" : "正在读取目标…"}
              disabled={busy || task.status === "running"}
              options={currentTargets.map((t) => ({ label: t.name, value: t.id }))}
              onChange={handleSelectTarget}
            />}
          </div>

          {(targetId || kind === "ssh") && <div className="aosp-connection-controls">
            <span className="aosp-connection-status" data-phase={connection.phase} role="status">
              {connection.phase === "checking" ? <LoaderCircle size={13} className="inline-spin" /> : connectionReady ? <CheckCircle2 size={13} /> : connection.phase === "failed" ? <AlertTriangle size={13} /> : <Unplug size={13} />}
              {kind === "ssh" ? "SSH" : "WSL"} {connectionReady ? "已连接" : connection.phase === "checking" ? "正在连接" : connection.phase === "failed" ? "连接失败" : connection.phase === "paused" ? "已断开" : "未连接"}
            </span>
            {kind === "wsl" && (connectionReady ? <Button size="sm" variant="subtle" icon={<Unplug size={14} />} title="断开当前目标" aria-label="断开当前目标" disabled={busy || task.status === "running"} onClick={() => { void connection.disconnect().catch((error) => showToast(formatInvokeError(error))); }} /> : <Button size="sm" variant="subtle" icon={connection.phase === "failed" ? <RefreshCw size={14} /> : <Plug size={14} />} title={connection.phase === "failed" ? "重试连接" : "连接当前目标"} aria-label={connection.phase === "failed" ? "重试连接" : "连接当前目标"} disabled={!desktop || busy || connection.phase === "checking"} onClick={() => { void connection.retry(); }} />)}
          </div>}

          {/* 根路径输入 */}
          <div className="aosp-root-input-wrap">
            <Input
              size="sm"
              placeholder="AOSP 根目录，例如 /home/android/aosp"
              disabled={busy || task.status === "running"}
              value={root}
              onChange={(e) => setRoot(e.target.value)}
              wrapperStyle={{ flex: 1, minWidth: 0, width: "auto" }}
              inputStyle={{ width: "auto", flex: 1, minWidth: 0 }}
              suffixAction={(
                <button
                  type="button"
                  className="aosp-dir-open-btn"
                  title="在 Linux 目标上选择目录"
                  disabled={!connectionReady || busy}
                  onClick={() => void browseDirectories(root.trim() || "/")}
                >
                  <FolderOpen size={14} />
                </button>
              )}
            />
          </div>
        </div>

        <div className="aosp-bar-actions">
          <Button
            variant="primary"
            size="sm"
            icon={<Search size={13} />}
            onClick={() => { void handleInspectWorkspace(); }}
            loading={busyAction === "probe"}
            disabled={busy || !connectionReady || !root.trim()}
          >
            验证源码树
          </Button>
        </div>
      </div>

      {(configurationError || connection.error || dataError || ((probe || lunchTargets.length > 0) && !connectionReady)) && <div className="aosp-connection-notice" role="status">
        {(configurationError || connection.error || dataError) && <span>{configurationError || connection.error || dataError}</span>}
        {(probe || lunchTargets.length > 0) && <span className="aosp-cached-result">上次结果</span>}
        {(configurationError || connection.error || dataError) && <Button size="xs" variant="subtle" icon={<RefreshCw size={13} />} disabled={busy || connection.phase === "checking"} onClick={() => { autoInspectRef.current = ""; if (configurationError) setConfigurationRevision((value) => value + 1); else void connection.retry(); }}>重试</Button>}
      </div>}

      {/* 3. 主体工作区 */}
      <main className="aosp-main-content">
        {/* 胶囊分段导航栏 (与 ADB、包查看器 100% 对齐) */}
        <div className="aosp-tabs-container">
          <SegmentedControl value={tab} onChange={setTab} label="AOSP 视图" className="aosp-seg-group" options={[
            { value: "overview", label: "源码环境与遥测" }, { value: "lunch", label: "目标与模块编译" },
            { value: "terminal", label: "交互式终端" }, { value: "console", label: "实时日志与诊断" },
            { value: "artifacts", label: `产物与设备部署 (${artifacts.length})` },
          ]} />

          <div className="aosp-build-badge-group">
            {task.status === "running" && (
              <Badge variant="primary">
                <span className="inline-block w-2 h-2 rounded-full bg-sky-400 animate-ping mr-1" />
                正在构建中 ({task.progress}%)
              </Badge>
            )}
            {task.status === "done" && <Badge variant="success">最近构建就绪</Badge>}
            {task.status === "cancelled" && <Badge variant="warning">已取消</Badge>}
          </div>
        </div>

        {!targetId ? <div className="aosp-workspace-empty"><span>{targetsLoaded ? configurationError ? "Linux 目标加载失败" : currentTargets.length ? "请选择 Linux 目标" : "暂无已保存的 Linux 目标" : "正在读取 Linux 目标…"}</span>{targetsLoaded && !currentTargets.length && !configurationError && <Button variant="subtle" size="sm" icon={<Settings size={14} />} onClick={() => window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: kind } }))}>配置目标</Button>}</div> : <>
        {/* ─── TAB 1: 源码环境与遥测 ─── */}
        {tab === "overview" && (
          <div className="aosp-tab-pane aosp-overview-panel custom-scrollbar">
            <PropertyGrid items={probeProperties} columns={2} />

            <div style={{ marginTop: 4 }}>
              <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 8 }}>
                探测宿主系统内核与工具链版本签名
              </div>
              <code className={`aosp-code-box${probe?.output ? "" : " aosp-not-detected"}`}>{probe?.output || "未检测"}</code>
            </div>
          </div>
        )}

        {/* ─── TAB 2: 目标与模块编译 ─── */}
        {tab === "lunch" && (
          <div className="aosp-tab-pane custom-scrollbar">
            {/* 编译控制面板 */}
            <div className="aosp-build-controls-card">
              <div className="segmented-glass aosp-build-mode-tabs" role="tablist" aria-label="构建方式">
                <button type="button" role="tab" aria-selected={buildMode === "lunch"} className={`seg-btn ${buildMode === "lunch" ? "active" : ""}`} onClick={() => setBuildMode("lunch")}>Lunch / 模块构建</button>
                <button type="button" role="tab" aria-selected={buildMode === "custom"} className={`seg-btn ${buildMode === "custom" ? "active" : ""}`} onClick={() => setBuildMode("custom")}>自定义命令</button>
              </div>
              {buildMode === "lunch" && <div className="aosp-build-fields-row" role="tabpanel" aria-label="Lunch / 模块构建">
                <div className="aosp-build-field">
                  <label>Lunch 构建目标</label>
                  <div style={{ display: "flex", gap: 8 }}>
                    <Input
                      size="sm"
                      placeholder="例如 aosp_cf_x86_64_phone-userdebug"
                      value={lunch}
                      onChange={(e) => setLunch(e.target.value)}
                    />
                    <Button size="sm" variant="secondary" loading={busyAction === "lunch"} disabled={busy || !connectionReady} onClick={() => void handleQueryLunch()}>
                      查询目标
                    </Button>
                  </div>
                </div>

                <div className="aosp-build-field">
                  <label>编译模块（留空则执行全量 m）</label>
                  <Input
                    size="sm"
                    placeholder="例如 framework, services, Settings"
                    value={moduleName}
                    onChange={(e) => setModuleName(e.target.value)}
                  />
                </div>

                <div className="aosp-build-actions">
                  <Button
                    variant="primary"
                    size="sm"
                    icon={<Play size={13} />}
                    onClick={() => { void handleStartBuild(); }}
                    loading={busyAction === "build" || task.status === "running"}
                    disabled={busy || task.status === "running" || !connectionReady || !!dataError}
                  >
                    开始增量构建
                  </Button>
                </div>
              </div>}

            {buildMode === "custom" && <div className="aosp-custom-commands" role="tabpanel" aria-label="自定义命令">
              <div className="aosp-build-field">
                <label htmlFor="aosp-command-name">自定义命令名称（可选）</label>
                <Input id="aosp-command-name" size="sm" value={commandName} onChange={(event) => setCommandName(event.target.value)} placeholder="为命令取一个便于识别的名称" />
              </div>
              <div className="aosp-build-field">
                <label htmlFor="aosp-custom-command">自定义构建 / 搜索命令</label>
                <textarea id="aosp-custom-command" className="aosp-command-editor" rows={4} value={commandDraft} onChange={(event) => setCommandDraft(event.target.value)} placeholder="输入要在当前 Linux 工作目录执行的 Shell 命令" spellCheck={false} />
                <span className="aosp-command-help">支持多行、管道和重定向。按原文执行，可保存复用；不加入 lunch 目标列表。</span>
              </div>
              <div className="aosp-command-actions">
                <Button size="sm" variant="secondary" onClick={handleSaveCommand} disabled={!commandDraft.trim()}>{selectedCommandId ? "保存修改" : "保存命令"}</Button>
                <Button size="sm" variant="primary" icon={<Play size={13} />} disabled={busy || task.status === "running" || !connectionReady || !commandDraft.trim()} onClick={() => void handleStartBuild(undefined, commandDraft)}>执行命令</Button>
                <Button size="sm" variant="subtle" onClick={() => { setSelectedCommandId(""); setCommandName(""); setCommandDraft(""); }}>新建命令</Button>
              </div>
              {preferences.commands.length > 0 && <div className="aosp-saved-commands">
                <div className="aosp-saved-commands-heading">
                  <span>已保存的命令</span><span className="aosp-saved-commands-count">{preferences.commands.length}</span>
                </div>
                {preferences.commands.map((item) => <div key={item.id} className="aosp-saved-command">
                  <div className="aosp-saved-command-header">
                    <div className="aosp-saved-command-title">
                      <span className="aosp-saved-command-icon" aria-hidden="true"><Terminal size={16} /></span>
                      <strong>{item.name}</strong>
                    </div>
                    <div className="aosp-command-actions">
                      <Button size="xs" variant="secondary" icon={<Pencil size={12} />} onClick={() => setEditingCommand({ ...item })}>编辑</Button>
                      <Button size="xs" variant="primary" icon={<Play size={12} />} disabled={busy || task.status === "running" || !connectionReady} onClick={() => void handleStartBuild(undefined, item.command)}>执行</Button>
                      <Button size="xs" variant="danger" icon={<Trash2 size={12} />} onClick={() => confirmAction({ title: "删除自定义命令", message: `确认删除「${item.name}」？`, confirmLabel: "确认删除", onConfirm: () => { setPreferences((previous) => ({ ...previous, commands: previous.commands.filter((command) => command.id !== item.id) })); if (selectedCommandId === item.id) setSelectedCommandId(""); showToast("自定义命令已删除。"); } })}>删除</Button>
                    </div>
                  </div>
                  <div className="aosp-saved-command-code"><span className="aosp-saved-command-prompt" aria-hidden="true">$</span><code>{item.command}</code></div>
                </div>)}
              </div>}
            </div>}
            </div>

            {buildMode === "lunch" && <div>
              <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 10 }}>
                源码树中的 lunch 目标
              </div>
              {lunchTargets.length > 0 ? (
                <div className="aosp-preset-targets-grid">
                  {lunchTargets.map((target) => (
                    <button
                      key={target.name}
                      type="button"
                      className={`aosp-target-pill-card ${lunch === target.name ? "selected" : ""}`}
                      onClick={() => {
                        setLunch(target.name);
                        showToast(`已选定 Lunch 目标：${target.name}`);
                      }}
                    >
                      <div className="aosp-target-pill-info">
                        <span className="aosp-target-pill-title">{target.name}</span>
                      </div>
                      {target.outputs.length > 0 && (
                        <span className="aosp-target-tags">
                          <Badge variant="success">已打包</Badge>
                          {target.outputs.map((output) => (
                            <Badge key={output} variant="neutral">{output}</Badge>
                          ))}
                        </span>
                      )}
                    </button>
                  ))}
                </div>
              ) : (
                <p style={{ margin: 0, fontSize: 12.5, color: "var(--text-tertiary)" }}>
                  还没有读取 lunch 目标。验证源码树后，点「查询目标」，这里会列出该树实际提供的目标。
                </p>
              )}
            </div>}
          </div>
        )}

          <div className="aosp-tab-pane aosp-terminal-pane" style={tab === "terminal" ? undefined : { display: "none" }} aria-hidden={tab !== "terminal"}>
            <div className="aosp-terminal-context">
              <div className="aosp-terminal-context-title">
                <Terminal size={15} />
                <div>
                  <strong>交互式 AOSP 终端</strong>
                  <span>{interactiveTerminalTarget.label} · {root.trim() || "未设置工作目录"} · 切换页面后会话继续运行</span>
                </div>
              </div>
              <Badge variant={kind === "ssh" ? "primary" : "neutral"}>{kind === "ssh" ? "SSH 远程" : "WSL Linux"}</Badge>
            </div>
            {targetId ? (
              <ChatTerminalPanel
                visible={active && tab === "terminal" && connectionReady}
                windowStateKey="aosp-terminals"
                panelClassName="aosp-interactive-terminal"
                showHideControl={false}
                showTargetControl
                target={interactiveTerminalTarget}
                workspacePath={root.trim() || undefined}
                sshConnections={sshConnections}
                wslProfiles={wslProfiles}
                onHide={() => undefined}
              />
            ) : (
              <div className="aosp-status-banner warning">
                <AlertTriangle size={16} />
                请先在顶部选择已保存的 WSL 或 SSH Linux 目标，再打开交互式终端。
              </div>
            )}
            {targetId && !connectionReady && <div className="aosp-workspace-empty">{connection.phase === "checking" ? "正在连接 Linux 目标…" : "Linux 目标未连接"}</div>}
          </div>

        {/* ─── TAB 3: 实时任务监控与日志控制台 ─── */}
        {tab === "console" && (
          <div className="aosp-tab-pane custom-scrollbar">
            {/* 任务状态卡片 */}
            <div className="aosp-task-status-card">
              <div className="aosp-task-header-row">
                <div className="aosp-task-message">
                  {task.status === "running" ? (
                    <span className="inline-block w-2.5 h-2.5 rounded-full bg-sky-400 animate-pulse mr-1" />
                  ) : task.status === "done" ? (
                    <CheckCircle2 size={16} className="text-emerald-500" />
                  ) : task.status === "cancelled" ? (
                    <AlertTriangle size={16} className="text-amber-500" />
                  ) : (
                    <Hammer size={16} className="text-slate-400" />
                  )}
                  <span>{task.message}</span>
                  {task.status !== "idle" && <span className="aosp-command-help">已运行 {task.elapsedSeconds} 秒</span>}
                </div>

                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  {task.status === "running" && (
                    <Button variant="danger" size="xs" icon={<Square size={12} />} onClick={handleCancelBuild}>
                      取消任务
                    </Button>
                  )}
                  <Button
                    variant="subtle"
                    size="xs"
                    icon={<Copy size={12} />}
                    onClick={() => {
                      navigator.clipboard.writeText(logs.join("\n"));
                      showToast("构建日志已复制到剪贴板");
                    }}
                  >
                    复制日志
                  </Button>
                </div>
              </div>

              {/* 进度光轨 */}
              <div className="aosp-progress-wrapper">
                <div className="aosp-progress-bar-track">
                  <div className="aosp-progress-bar-fill" style={{ width: `${task.progress}%` }} />
                </div>
                <span className="aosp-progress-percentage">{task.progress}%</span>
              </div>
            </div>

            {/* 控制台日志视口 */}
            <div className="aosp-console-viewport">
              <div className="aosp-console-titlebar">
                <div>
                  <span>实时构建管道输出 (受管会话: {task.id})</span>
                </div>
                <div style={{ display: "flex", gap: 6 }}>
                  <Button variant="subtle" size="xs" onClick={() => confirmAction({ title: "清空构建输出", message: "确认清空当前构建输出？不会停止编译或删除构建产物。", confirmLabel: "确认清空", onConfirm: () => setLogs([]) })}>
                    清空输出
                  </Button>
                </div>
              </div>

              <pre ref={consoleRef} className="aosp-console-logs custom-scrollbar" onScroll={(event) => { const element = event.currentTarget; followLogsRef.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48; }}>
                {logs.map((line, idx) => (
                    <div key={idx} className={logLineClass(line)}>
                      {line}
                    </div>
                ))}
              </pre>
            </div>
          </div>
        )}

        {/* ─── TAB 4: 编译产物与设备部署 ─── */}
        {tab === "artifacts" && (
          <div className="aosp-tab-pane custom-scrollbar">
            <div className="aosp-artifacts-header">
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)" }}>
                  部署目标设备:
                </span>
                <div style={{ width: 260 }}>
                  <Select
                    size="sm"
                    value={selectedDevice}
                    options={deviceOptions.map((item) => ({ label: item.label, value: item.serial }))}
                    onChange={(val) => setSelectedDevice(val)}
                  />
                </div>
              </div>

              <Button
                variant="secondary"
                size="sm"
                loading={busyAction === "artifacts"}
                disabled={busy || !connectionReady}
                onClick={() => void handleLoadArtifacts()}
              >
                重新扫描产物
              </Button>
            </div>

            <div className="aosp-artifacts-list">
              {artifacts.map((art) => (
                <div key={art.path} className="aosp-artifact-item-row">
                  <div className="aosp-artifact-item-info">
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <span className="aosp-artifact-item-path">{art.name}</span>
                      <Badge variant={art.kind === "apk" ? "primary" : art.kind === "img" ? "warning" : "neutral"}>
                        {art.kind.toUpperCase()}
                      </Badge>
                      <span style={{ fontSize: 11.5, color: "var(--text-tertiary)" }}>
                        {formatBytes(art.size)}
                      </span>
                    </div>
                    <div className="aosp-artifact-item-meta">
                      <span>{art.path}</span>
                      <span>·</span>
                      <span>{art.installNote}</span>
                    </div>
                  </div>

                  <div className="aosp-artifact-item-actions">
                    {art.installable ? (
                      <Button
                        variant="primary"
                        size="sm"
                        icon={<Smartphone size={13} />}
                        loading={busyAction === "install"}
                        disabled={busy || !connectionReady}
                        onClick={() => void handleInstallArtifact(art)}
                      >
                        直装到设备
                      </Button>
                    ) : (
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<Download size={13} />}
                        onClick={() => showToast(desktop ? `产物仍在 Linux 源码树：${art.path}` : `已启动提取 ${art.name} 到本地`)}
                      >
                        提取到本地
                      </Button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
        </>}
      </main>
    </div>
  );
};

export default AospBuildView;
