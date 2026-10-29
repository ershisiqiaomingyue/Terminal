import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import React, { useEffect, useRef, useState } from "react";
import { Terminal, ScrollText, Tv, FolderSync, Package, Activity } from "lucide-react";
import { Button, Badge, Select } from "@/components/ui";
import {
  cancelAndroidTask,
  deviceLabel,
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openDirectory,
  waitForTask,
  type AdbDevice,
} from "@/platform/runtime";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { perfettoDurationSeconds } from "@/application/androidActionParameters";
import { publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import "./tool-window-action.css";

const LogcatView = React.lazy(() => import("@/views/Logcat").then((module) => ({ default: module.LogcatView })));
const CodeViewerView = React.lazy(() => import("@/views/CodeViewer").then((module) => ({ default: module.CodeViewerView })));
const TerminalView = React.lazy(() => import("@/views/Terminal").then((module) => ({ default: module.TerminalView })));
const PackageManagerView = React.lazy(() => import("@/views/PackageManager").then((module) => ({ default: module.PackageManagerView })));
const AdbDevicesView = React.lazy(() => import("@/views/AdbDevices").then((module) => ({ default: module.AdbDevicesView })));
const ArchiveInspectorView = React.lazy(() => import("@/views/ArchiveInspector").then((module) => ({ default: module.ArchiveInspectorView })));
const AospBuildView = React.lazy(() => import("@/views/AospBuild").then((module) => ({ default: module.AospBuildView })));
const DeviceFilesView = React.lazy(() => import("@/views/DeviceFiles").then((module) => ({ default: module.DeviceFilesView })));
const DeviceCaptureView = React.lazy(() => import("@/views/DeviceCapture").then((module) => ({ default: module.DeviceCaptureView })));
const DeviceControlView = React.lazy(() => import("@/views/DeviceControl").then((module) => ({ default: module.DeviceControlView })));
const SimpleperfView = React.lazy(() => import("@/views/Simpleperf").then((module) => ({ default: module.SimpleperfView })));
const MethodProfilerView = React.lazy(() => import("@/views/MethodProfiler").then((module) => ({ default: module.MethodProfilerView })));
const HeapDumpView = React.lazy(() => import("@/views/HeapDump").then((module) => ({ default: module.HeapDumpView })));
const BenchmarkView = React.lazy(() => import("@/views/Benchmark").then((module) => ({ default: module.BenchmarkView })));
const NativeSymbolizerView = React.lazy(() => import("@/views/NativeSymbolizer").then((module) => ({ default: module.NativeSymbolizerView })));

export type SupportedToolKey =
  | "code-viewer"
  | "scrcpy"
  | "logcat"
  | "shell"
  | "files"
  | "apps"
  | "perf"
  | "adb"
  | "archive"
  | "aosp"
  | "capture"
  | "control"
  | "simpleperf"
  | "method-profiler"
  | "heap-dump"
  | "benchmark"
  | "native-symbolizer";

interface WindowProps {
  type: SupportedToolKey;
  active?: boolean;
  onOpenTool?: (toolKey: SupportedToolKey) => void;
  onBack?: () => void;
}

function fileInDirectory(directory: string, name: string): string {
  const trimmed = directory.replace(/[\\/]+$/, "");
  const separator = trimmed.includes("\\") ? "\\" : "/";
  return `${trimmed}${separator}${name}`;
}

const LiveSessionPanel: React.FC<{ type: "scrcpy" | "perf"; onBack?: () => void }> = ({ type, onBack }) => {
  const desktop = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue(type, "serial", "");
  const [statusText, setStatusText] = useToolWindowValue(type, "statusText", type === "scrcpy" ? "尚未启动投屏" : "尚未采集 Perfetto");
  const [running, setRunning] = useState(false);
  const taskIdRef = useRef<string | null>(null);
  const startingRef = useRef(false);
  useToolDetachGuard(type, running);

  useEffect(() => {
    if (!desktop) return;
    let cancelled = false;
    listAdbDevices()
      .then((list) => {
        if (cancelled) return;
        const online = list.filter((device) => device.state === "device");
        setDevices(online);
        setSerial((current) => (current && online.some((device) => device.serial === current) ? current : online[0]?.serial ?? ""));
      })
      .catch((reason) => {
        if (!cancelled) setStatusText(formatInvokeError(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [desktop]);

  const selected = devices.find((device) => device.serial === serial);
  const title = type === "scrcpy" ? "Scrcpy 实时投屏" : "Perfetto 性能采集";
  const icon = type === "scrcpy" ? <Tv size={20} /> : <Activity size={20} />;

  const requireSerial = () => {
    if (serial) return true;
    setStatusText("请先选择在线设备");
    return false;
  };

  const startScrcpy = async (serialOverride?: string, actionId?: string) => {
    if (!desktop) { notifyDesktopRequired(actionId); return; }
    const targetSerial = serialOverride || serial;
    if (!targetSerial) {
      const message = "请先选择在线设备";
      setStatusText(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Scrcpy 未启动", detail: message, actionId });
      return;
    }
    setRunning(true);
    try {
      const taskId = await invoke<string>("start_scrcpy", { request: { serial: targetSerial, noAudio: true } });
      setStatusText(`投屏已启动，任务 ${taskId}。命令未返回帧率。`);
      if (actionId) publishAiTaskActivity({ running: true, status: "running", title: "Scrcpy 实时投屏中", detail: `已连接 ${targetSerial}。`, actionId, progress: null });
    } catch (reason) {
      const message = formatInvokeError(reason);
      setStatusText(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Scrcpy 启动失败", detail: message, actionId });
    } finally {
      setRunning(false);
    }
  };

  const stopScrcpy = async () => {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!requireSerial()) return;
    try {
      await invoke("stop_scrcpy", { serial });
      setStatusText(`已关闭 ${serial} 的投屏`);
    } catch (reason) {
      setStatusText(formatInvokeError(reason));
    }
  };

  const startPerfetto = async (serialOverride?: string, destinationOverride?: string, actionId?: string, durationOverride?: string) => {
    if (!desktop) { notifyDesktopRequired(actionId); return; }
    const targetSerial = serialOverride || serial;
    if (!targetSerial || startingRef.current || running) {
      const message = targetSerial ? "Perfetto 任务已在运行。" : "请先选择在线设备";
      setStatusText(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Perfetto 未启动", detail: message, actionId });
      return;
    }
    startingRef.current = true;
    setRunning(true);
    try {
      const durationSeconds = perfettoDurationSeconds(durationOverride);
      const directory = destinationOverride || await openDirectory("Perfetto 产物目录");
      if (!directory) {
        if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Perfetto 等待输出目录", detail: "未选择本地输出目录，任务尚未启动。", actionId, progress: 0 });
        return;
      }
      // Pull target must be a file; the dialog only selects the parent directory.
      const destination = fileInDirectory(directory, "perfetto-ui.pf");
      const taskId = await invoke<string>("start_perfetto_capture", {
        request: { serial: targetSerial, destination, durationSeconds, preset: "ui" },
      });
      taskIdRef.current = taskId;
      const task = await waitForTask(taskId, (update) => {
        if (update.message) setStatusText(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在采集 Perfetto", detail: update.message || "正在采样。", actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "Perfetto 未完成");
      }
      setStatusText(task.artifactPath ? `产物：${task.artifactPath}` : "Perfetto 完成，但任务没有返回产物路径");
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Perfetto 采集完成", detail: task.artifactPath || task.message || "已生成性能采样。", actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      setStatusText(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Perfetto 采集失败", detail: message, actionId });
    } finally {
      taskIdRef.current = null;
      startingRef.current = false;
      setRunning(false);
    }
  };

  const cancelPerfetto = async () => {
    const id = taskIdRef.current;
    if (!id) {
      setStatusText("没有正在进行的 Perfetto 任务");
      return;
    }
    try {
      await cancelAndroidTask(id);
      setStatusText("已请求取消 Perfetto 采集");
    } catch (reason) {
      setStatusText(formatInvokeError(reason));
    }
  };

  useApplicationPageAction(`open.tool.${type}`, (action) => {
    const targetSerial = action.parameters.serial || serial;
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (type === "scrcpy") {
      publishAiTaskActivity({ running: true, status: "running", title: "正在启动 Scrcpy", detail: "正在连接 Android 设备。", actionId: action.actionId, progress: 10 });
      void startScrcpy(targetSerial, action.actionId);
      return;
    }
    publishAiTaskActivity({ running: true, status: "running", title: "正在启动 Perfetto", detail: "正在准备性能采样任务。", actionId: action.actionId, progress: 10 });
    void startPerfetto(targetSerial, action.parameters.destination, action.actionId, action.parameters.durationSeconds);
  });

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 16,
        height: "100%",
        minHeight: 0,
        overflowY: "auto",
        background: "var(--bg-canvas)",
        padding: "24px",
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          paddingBottom: 16,
          borderBottom: "1px solid var(--divider-subtle)",
          gap: 12,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <ToolBackButton onBack={onBack} />
          <div
            style={{
              width: 40,
              height: 40,
              borderRadius: "var(--radius-base)",
              background: "var(--accent-subtle)",
              color: "var(--accent-primary)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            {icon}
          </div>
          <div>
            <h3 style={{ fontSize: 16, fontWeight: 600, color: "var(--text-primary)" }}>
              {title}
            </h3>
            <p style={{ fontSize: 12, color: "var(--text-tertiary)", marginTop: 2 }}>
              {selected ? deviceLabel(selected) : "未选择设备"}
            </p>
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>
          <Select
            size="sm"
            placeholder="选择设备"
            options={devices.map((device) => ({ value: device.serial, label: deviceOptionLabel(device) }))}
            value={serial}
            onChange={setSerial}
            style={{ minWidth: 220 }}
          />
          <Badge variant={running ? "success" : "neutral"}>{running ? "进行中" : "空闲"}</Badge>
          {type === "scrcpy" ? (
            <>
              <Button variant="primary" size="sm" onClick={() => void startScrcpy()} disabled={running || !desktop || !serial}>
                启动投屏
              </Button>
              <Button variant="danger" size="sm" onClick={() => void stopScrcpy()} disabled={!desktop || !serial}>
                关闭投屏
              </Button>
            </>
          ) : (
            <>
              <Button variant="primary" size="sm" onClick={() => void startPerfetto()} disabled={running || !desktop || !serial}>
                开始 Perfetto (10s UI preset)
              </Button>
              <Button variant="secondary" size="sm" onClick={() => void cancelPerfetto()} disabled={!running}>
                取消
              </Button>
            </>
          )}
        </div>
      </header>

      <div
        style={{
          flex: 1,
          minHeight: 380,
          background: "var(--bg-input)",
          borderRadius: "var(--radius-base)",
          padding: 20,
          fontFamily: "var(--font-mono)",
          fontSize: 12.5,
          color: "var(--text-secondary)",
          lineHeight: 1.6,
          overflowY: "auto",
        }}
      >
        {type === "scrcpy" ? (
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", gap: 12 }}>
            <div style={{ width: 220, height: 360, borderRadius: 28, background: "var(--tool-state-bg)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}>
              <span style={{ fontSize: 13, color: "var(--text-tertiary)", textAlign: "center", wordBreak: "break-all" }}>{statusText}</span>
            </div>
          </div>
        ) : (
          <div style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{statusText}</div>
        )}
      </div>
    </div>
  );
};
export const ToolWindowView: React.FC<WindowProps> = ({ type, active = true, onOpenTool, onBack }) => {
  let page: React.ReactNode;
  if (type === "code-viewer") {
    page = <CodeViewerView active={active} />;
  } else if (type === "logcat") {
    page = <LogcatView active={active} onBack={onBack} />;
  } else if (type === "shell") {
    page = <TerminalView onBack={onBack} />;
  } else if (type === "apps") {
    page = <PackageManagerView onBack={onBack} onOpenTerminal={() => onOpenTool?.("shell")} />;
  } else if (type === "adb") {
    page = <AdbDevicesView onBack={onBack} onOpenTerminal={() => onOpenTool?.("shell")} onOpenScrcpy={() => onOpenTool?.("scrcpy")} />;
  } else if (type === "archive") {
    page = <ArchiveInspectorView onBack={onBack} />;
  } else if (type === "aosp") {
    page = <AospBuildView active={active} onBack={onBack} />;
  } else if (type === "files") {
    page = <DeviceFilesView onBack={onBack} />;
  } else if (type === "capture") {
    page = <DeviceCaptureView onBack={onBack} />;
  } else if (type === "control") {
    page = <DeviceControlView onBack={onBack} />;
  } else if (type === "simpleperf") {
    page = <SimpleperfView onBack={onBack} />;
  } else if (type === "method-profiler") {
    page = <MethodProfilerView onBack={onBack} />;
  } else if (type === "heap-dump") {
    page = <HeapDumpView onBack={onBack} />;
  } else if (type === "benchmark") {
    page = <BenchmarkView onBack={onBack} />;
  } else if (type === "native-symbolizer") {
    page = <NativeSymbolizerView onBack={onBack} />;
  } else {
    page = <LiveSessionPanel type={type} onBack={onBack} />;
  }

  return (
    <div className="tool-window-action-frame">
      {!isTauri() && <ToolEmptyState compact title="等待桌面连接" description="连接设备或选择本地文件后显示真实数据。" />}
      <React.Suspense fallback={<div className="app-view-loading">正在载入工具...</div>}>{page}</React.Suspense>
    </div>
  );
};
