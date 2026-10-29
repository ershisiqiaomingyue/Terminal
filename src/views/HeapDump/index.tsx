import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import React, { useCallback, useEffect, useState } from "react";
import {
  Database,
  Play,
  Square,
  Download,
  AlertTriangle,
  CheckCircle2,
} from "lucide-react";
import { Button, Input, Badge, Select } from "@/components/ui";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import {
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openDirectory,
  waitForTask,
  type AdbDevice,
} from "@/platform/runtime";
import "./heap-dump.css";

interface ClassHeapRecord {
  className: string;
  instances: number;
  retainedSize: string;
  isLeaked: boolean;
}

interface HeapDumpReport {
  taskId: string;
  androidHprofPath: string;
  standardHprofPath: string;
  androidHprofSize: number;
  standardHprofSize: number;
  metadata: string;
}

interface HeapSharkLeak {
  signature: string;
  description: string;
  retainedBytes: number;
  retainedObjects: number;
}

interface HeapSharkAnalysis {
  status: string;
  applicationLeakCount: number;
  libraryLeakCount: number;
  applicationLeaks: HeapSharkLeak[];
  libraryLeaks: HeapSharkLeak[];
}

interface HeapInfo {
  androidSize: string;
  standardSize: string;
  leaks: string;
}

function formatBytes(size: number): string {
  if (!Number.isFinite(size)) return "0 B";
  const sign = size < 0 ? "-" : "";
  let value = Math.abs(size);
  const units = ["B", "KB", "MB", "GB"];
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = unit === 0 ? String(Math.round(value)) : value.toFixed(value >= 10 ? 1 : 2);
  return `${sign}${text} ${units[unit]}`;
}

function leakRows(leaks: HeapSharkLeak[]): ClassHeapRecord[] {
  return leaks.map((leak) => ({
    className: leak.signature,
    instances: leak.retainedObjects,
    retainedSize: formatBytes(leak.retainedBytes),
    isLeaked: true,
  }));
}

export const HeapDumpView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("heap-dump", "serial", "");
  const [packageName, setPackageName] = useToolWindowValue("heap-dump", "packageName", "com.android.camera2");
  const [isDumping, setIsDumping] = useState(false);
  const [classes, setClasses] = useToolWindowValue<ClassHeapRecord[]>("heap-dump", "classes", []);
  const [heapInfo, setHeapInfo] = useToolWindowValue<HeapInfo | null>("heap-dump", "heapInfo", null);
  const [standardHprofPath, setStandardHprofPath] = useToolWindowValue<string | null>("heap-dump", "standardHprofPath", null);
  useToolDetachGuard("heap-dump", isDumping);

  const showToast = useCallback((msg: string) => {
    notify(msg, "info");
  }, []);

  useEffect(() => {
    if (!tauri) return;
    let cancelled = false;
    listAdbDevices()
      .then((list) => {
        if (cancelled) return;
        const online = list.filter((device) => device.state === "device");
        setDevices(online);
        setSerial((current) => (current && online.some((device) => device.serial === current) ? current : online[0]?.serial ?? ""));
      })
      .catch((reason) => {
        if (!cancelled) notify(formatInvokeError(reason), "error");
      });
    return () => {
      cancelled = true;
    };
  }, [showToast, tauri]);

  const selected = devices.find((device) => device.serial === serial);

  const handleStartDump = async (serialOverride?: string, packageOverride?: string, destinationOverride?: string, actionId?: string) => {
    if (isDumping) return;
    const targetSerial = serialOverride || serial;
    const targetPackage = packageOverride || packageName;
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial) {
      const message = "请先选择在线设备";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 堆转储未启动", detail: message, actionId });
      return;
    }
    if (!targetPackage.trim()) {
      const message = "请填写目标应用包名";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 堆转储未启动", detail: message, actionId });
      return;
    }
    const destination = destinationOverride || await openDirectory("堆转储产物目录");
    if (!destination) {
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 堆转储等待输出目录", detail: "未选择本地输出目录，转储尚未启动。", actionId, progress: 0 });
      return;
    }
    setIsDumping(true);
    setClasses([]);
    setHeapInfo(null);
    setStandardHprofPath(null);
    try {
      const taskId = await invoke<string>("start_heap_dump", {
        request: {
          serial: targetSerial,
          destination,
          packageName: targetPackage,
          excludeZygote: true,
        },
      });
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在导出 Java 堆转储", detail: update.message || targetPackage, actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "堆转储未完成");
      }
      const report = await invoke<HeapDumpReport>("read_heap_dump_report", { taskId });
      setStandardHprofPath(report.standardHprofPath);
      setHeapInfo({
        androidSize: formatBytes(report.androidHprofSize),
        standardSize: formatBytes(report.standardHprofSize),
        leaks: "Shark 分析中",
      });
      try {
        const sharkTaskId = await invoke<string>("start_heap_shark_analysis", { taskId: report.taskId });
        const sharkTask = await waitForTask(sharkTaskId, (update) => {
          if (update.message) showToast(`${update.progress}% ${update.message}`);
          if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在分析 Java 堆转储", detail: update.message || "正在执行 Shark 泄漏分析。", actionId, progress: update.progress });
        });
        if (sharkTask.status !== "done") {
          throw new Error(sharkTask.error || sharkTask.message || "Shark 分析未完成");
        }
        const analysis = await invoke<HeapSharkAnalysis>("read_heap_shark_analysis", { taskId: sharkTaskId });
        setClasses([...leakRows(analysis.applicationLeaks), ...leakRows(analysis.libraryLeaks)]);
        setHeapInfo({
          androidSize: formatBytes(report.androidHprofSize),
          standardSize: formatBytes(report.standardHprofSize),
          leaks: `应用泄漏 ${analysis.applicationLeakCount} · 库泄漏 ${analysis.libraryLeakCount}`,
        });
        showToast(`Shark 完成：应用泄漏 ${analysis.applicationLeakCount}，库泄漏 ${analysis.libraryLeakCount}`);
        if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Java 堆转储完成", detail: report.standardHprofPath, actionId, progress: 100 });
      } catch (reason) {
        setClasses([]);
        setHeapInfo({
          androidSize: formatBytes(report.androidHprofSize),
          standardSize: formatBytes(report.standardHprofSize),
          leaks: "Shark 分析失败",
        });
        if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Shark 分析失败", detail: formatInvokeError(reason), actionId });
        notify(`Shark 分析失败：${formatInvokeError(reason)}。Android HPROF：${report.androidHprofPath}；标准 HPROF：${report.standardHprofPath}`, "error");
      }
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 堆转储失败", detail: message, actionId });
    } finally {
      setIsDumping(false);
    }
  };

  useApplicationPageAction("open.tool.heap-dump", (action) => {
    if (isDumping) {
      publishAiTaskActivity({ running: false, status: "error", title: "Java 堆正在转储", detail: "请等待当前转储完成。", actionId: action.actionId });
      return;
    }
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (action.parameters.packageName) setPackageName(action.parameters.packageName);
    publishAiTaskActivity({ running: true, status: "running", title: "正在准备 Java 堆转储", detail: "正在交接设备、包名与输出目录。", actionId: action.actionId, progress: 10 });
    void handleStartDump(action.parameters.serial, action.parameters.packageName, action.parameters.destination, action.actionId);
  });

  const heapProps: PropertyItem[] = [
        { icon: <Database size={14} />, label: "Android HPROF 大小", value: heapInfo?.androidSize ?? "尚未转储", valueType: "accent" },
        { icon: <CheckCircle2 size={14} />, label: "标准 HPROF 大小", value: heapInfo?.standardSize ?? "尚未转储", valueType: "green" },
        { icon: <AlertTriangle size={14} />, label: "Shark 泄漏计数", value: heapInfo?.leaks ?? "尚未转储", valueType: heapInfo?.leaks === "Shark 分析失败" ? "danger" : "orange" },
      ];

  return (
    <div className="heap-dump-container">
      <header className="heap-dump-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            Java 堆转储与内存分析 (Heap Dump)
          </h2>
          <Badge variant={selected ? "primary" : "neutral"}>{selected ? deviceOptionLabel(selected) : "无在线设备"}</Badge>
          {tauri && (
            <Select
              size="sm"
              placeholder="选择设备"
              options={devices.map((device) => ({ value: device.serial, label: deviceOptionLabel(device) }))}
              value={serial}
              disabled={isDumping}
              onChange={(next) => { setSerial(next); setClasses([]); setHeapInfo(null); setStandardHprofPath(null); }}
              style={{ width: 260, flex: "0 0 260px" }}
            />
          )}
        </div>

        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Button
            variant="secondary"
            size="sm"
            icon={<Download size={13} />}
            onClick={() => {
              if (!tauri) {
      notifyDesktopRequired();
      return;
    }
              showToast(standardHprofPath ?? "尚无标准 HPROF");
            }}
          >
            导出标准 HPROF
          </Button>
        </div>
      </header>

      <main className="heap-dump-content custom-scrollbar">
        <PropertyGrid items={heapProps} columns={2} />

        <div className="heap-dump-config-card">
          <div style={{ display: "flex", gap: 12, alignItems: "flex-end", flexWrap: "wrap" }}>
            <div style={{ flex: 1, minWidth: 240 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                目标调试应用包名 (需具备可调试标记或 root)
              </label>
              <Input
                size="sm"
                placeholder="例如 com.android.camera2"
                value={packageName}
                onChange={(e) => setPackageName(e.target.value)}
              />
            </div>

            <Button
              variant={isDumping ? "danger" : "primary"}
              size="sm"
              icon={isDumping ? <Square size={13} /> : <Play size={13} />}
              onClick={() => void handleStartDump()}
              disabled={isDumping || !tauri || !serial}
            >
              {isDumping ? "转储与转换中..." : "导出并分析 HPROF 堆快照"}
            </Button>
          </div>
        </div>

        <div className="heap-dump-table-wrap">
          <div className="heap-dump-table-head">
            <span>类名 (Class Name)</span>
            <span>存活实例数</span>
            <span>Retained 内存</span>
            <span style={{ textAlign: "right" }}>泄漏状态</span>
          </div>

          <div>
            {classes.length === 0 && <ToolEmptyState title="尚无泄漏分析结果" description="完成堆转储和 Shark 分析后显示检测结果。" />}
            {classes.map((c, idx) => (
              <div key={idx} className="heap-dump-table-row">
                <span style={{ fontFamily: "var(--font-mono, monospace)" }} title={c.className}>
                  {c.className}
                </span>
                <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{c.instances.toLocaleString()}</span>
                <span style={{ fontWeight: 600, color: c.isLeaked ? "var(--danger-text)" : "var(--text-primary)" }}>
                  {c.retainedSize}
                </span>
                <div style={{ textAlign: "right" }}>
                  <Badge variant={c.isLeaked ? "danger" : "neutral"}>
                    {c.isLeaked ? "疑似泄漏" : "正常"}
                  </Badge>
                </div>
              </div>
            ))}
          </div>
        </div>
      </main>
    </div>
  );
};

export default HeapDumpView;
