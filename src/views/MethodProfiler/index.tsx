import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import React, { useCallback, useEffect, useState } from "react";
import {
  Play,
  Square,
  Download,
  Clock,
  Layers,
  Activity,
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
import "./method-profiler.css";
import { readToolPreferences } from "@/platform/toolPreferences";

interface MethodRecord {
  inclusive: string;
  exclusive: string;
  calls: number;
  signature: string;
}

interface MethodProfileReport {
  taskId: string;
  tracePath: string;
  traceSize: number;
  metadata: string;
  summary: {
    version: string | null;
    threadCount: number;
    methodCount: number;
    classCount: number;
    limitation: string;
  };
}

export const MethodProfilerView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("method-profiler", "serial", "");
  const [packageName, setPackageName] = useToolWindowValue("method-profiler", "packageName", "com.android.settings");
  const [duration, setDuration] = useToolWindowValue("method-profiler", "duration", () => String(readToolPreferences().methodDuration));
  const [interval, setInterval] = useToolWindowValue("method-profiler", "interval", () => String(readToolPreferences().methodInterval));
  const [isSampling, setIsSampling] = useState(false);
  const [methods, setMethods] = useToolWindowValue<MethodRecord[]>("method-profiler", "methods", []);
  const [tracePath, setTracePath] = useToolWindowValue<string | null>("method-profiler", "tracePath", null);
  const [summaryText, setSummaryText] = useToolWindowValue<{ threads: string; methods: string; path: string } | null>("method-profiler", "summaryText", null);
  useToolDetachGuard("method-profiler", isSampling);

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

  const handleStartTrace = async (serialOverride?: string, packageOverride?: string, durationOverride?: string, destinationOverride?: string, actionId?: string) => {
    if (isSampling) return;
    const targetSerial = serialOverride || serial;
    const targetPackage = packageOverride || packageName;
    const targetDuration = durationOverride || duration;
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial) {
      const message = "请先选择在线设备";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 方法采样未启动", detail: message, actionId });
      return;
    }
    if (!targetPackage.trim()) {
      const message = "请填写目标应用包名";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 方法采样未启动", detail: message, actionId });
      return;
    }
    const destination = destinationOverride || await openDirectory("方法采样产物目录");
    if (!destination) {
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 方法采样等待输出目录", detail: "未选择本地输出目录，采样尚未启动。", actionId, progress: 0 });
      return;
    }
    setIsSampling(true);
    setMethods([]);
    setTracePath(null);
    setSummaryText(null);
    try {
      const taskId = await invoke<string>("start_method_profile_capture", {
        request: {
          serial: targetSerial,
          destination,
          packageName: targetPackage,
          durationSeconds: Number(targetDuration) || 5,
          samplingIntervalUs: Number(interval) || 1000,
        },
      });
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在执行 Java 方法采样", detail: update.message || targetPackage, actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "方法采样未完成");
      }
      const report = await invoke<MethodProfileReport>("read_method_profile_report", { taskId });
      const { summary } = report;
      setTracePath(report.tracePath);
      setMethods([
        {
          inclusive: String(summary.methodCount),
          exclusive: String(summary.classCount),
          calls: summary.threadCount,
          signature: `${report.tracePath} — ${summary.limitation}`,
        },
      ]);
      setSummaryText({
        threads: `${summary.threadCount} 条线程`,
        methods: `${summary.methodCount} 个方法`,
        path: report.tracePath,
      });
      showToast(report.tracePath);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Java 方法采样完成", detail: report.tracePath, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Java 方法采样失败", detail: message, actionId });
    } finally {
      setIsSampling(false);
    }
  };

  useApplicationPageAction("open.tool.method-profiler", (action) => {
    if (isSampling) {
      publishAiTaskActivity({ running: false, status: "error", title: "Java 方法正在采样", detail: "请等待当前采样完成。", actionId: action.actionId });
      return;
    }
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (action.parameters.packageName) setPackageName(action.parameters.packageName);
    if (action.parameters.durationSeconds) setDuration(action.parameters.durationSeconds);
    publishAiTaskActivity({ running: true, status: "running", title: "正在准备 Java 方法采样", detail: "正在交接设备、包名与采样参数。", actionId: action.actionId, progress: 10 });
    void handleStartTrace(action.parameters.serial, action.parameters.packageName, action.parameters.durationSeconds, action.parameters.destination, action.actionId);
  });

  const profilerProps: PropertyItem[] = [
        { icon: <Layers size={14} />, label: "采样活跃线程", value: summaryText?.threads ?? "尚未采样", valueType: "accent" },
        { icon: <Activity size={14} />, label: "采样命中方法总数", value: summaryText?.methods ?? "尚未采样" },
        { icon: <Clock size={14} />, label: "Trace 路径", value: summaryText?.path ?? "尚未采样", valueType: "green" },
      ];

  return (
    <div className="method-profiler-container">

      <header className="method-profiler-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            Java 方法采样与 Trace 分析
          </h2>
          <Badge variant={selected ? "primary" : "neutral"}>{selected ? deviceOptionLabel(selected) : "无在线设备"}</Badge>
          {tauri && (
            <Select
              size="sm"
              placeholder="选择设备"
              options={devices.map((device) => ({ value: device.serial, label: deviceOptionLabel(device) }))}
              value={serial}
              disabled={isSampling}
              onChange={(next) => { setSerial(next); setMethods([]); setTracePath(null); setSummaryText(null); }}
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
              showToast(tracePath ?? "尚无 Trace 文件");
            }}
          >
            导出 AS Trace 文件
          </Button>
        </div>
      </header>

      <main className="method-profiler-content custom-scrollbar">
        <PropertyGrid items={profilerProps} columns={2} />

        <div className="method-profiler-config-card">
          <div style={{ display: "flex", gap: 12, alignItems: "flex-end", flexWrap: "wrap" }}>
            <div style={{ flex: 1, minWidth: 220 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                目标应用包名
              </label>
              <Input
                size="sm"
                placeholder="例如 com.android.settings"
                value={packageName}
                onChange={(e) => setPackageName(e.target.value)}
              />
            </div>

            <div style={{ width: 130 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                采样间隔 (μs)
              </label>
              <Input
                size="sm"
                type="number"
                value={interval}
                onChange={(e) => setInterval(e.target.value)}
              />
            </div>

            <div style={{ width: 110 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                时长 (秒)
              </label>
              <Input
                size="sm"
                type="number"
                value={duration}
                onChange={(e) => setDuration(e.target.value)}
              />
            </div>

            <Button
              variant={isSampling ? "danger" : "primary"}
              size="sm"
              icon={isSampling ? <Square size={13} /> : <Play size={13} />}
              onClick={() => void handleStartTrace()}
              disabled={isSampling || !tauri || !serial}
            >
              {isSampling ? "采集采样中..." : "启动 ART 方法 Trace"}
            </Button>
          </div>
        </div>

        <div className="method-profiler-table-wrap">
          <div className="method-profiler-table-head">
            {(
              <>
                <span>方法数</span>
                <span>类数</span>
                <span>线程数</span>
                <span>Trace 与限制</span>
              </>
            )}
          </div>

          <div>
            {methods.length === 0 && <ToolEmptyState title="尚未采样" description="选择在线设备后采集 ART Trace。" />}
            {methods.map((m, idx) => (
              <div key={idx} className="method-profiler-table-row">
                <span style={{ fontWeight: 700, color: "var(--text-primary)" }}>
                  {m.inclusive}
                </span>
                <span style={{ color: "var(--text-secondary)" }}>{m.exclusive}</span>
                <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{m.calls.toLocaleString()}</span>
                <span style={{ fontFamily: "var(--font-mono, monospace)" }} title={m.signature}>
                  {m.signature}
                </span>
              </div>
            ))}
          </div>
        </div>
      </main>
    </div>
  );
};

export default MethodProfilerView;
