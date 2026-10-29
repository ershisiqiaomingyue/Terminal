import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  Cpu,
  Play,
  Square,
  Download,
  Activity,
  CheckCircle2,
} from "lucide-react";
import { Button, Input, Badge, Select } from "@/components/ui";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import {
  cancelAndroidTask,
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openDirectory,
  waitForTask,
  type AdbDevice,
} from "@/platform/runtime";
import "./simpleperf.css";
import { readToolPreferences } from "@/platform/toolPreferences";

interface HotspotRecord {
  overhead: string;
  symbol: string;
  dso: string;
  samples: number;
}

interface SampleInfo {
  duration: string;
  packageName: string;
  truncated: string;
}

function splitCsvRow(line: string): string[] {
  const cells: string[] = [];
  let current = "";
  let quoted = false;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (char === '"') {
      if (quoted && line[index + 1] === '"') {
        current += '"';
        index += 1;
      } else {
        quoted = !quoted;
      }
      continue;
    }
    if (char === "," && !quoted) {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += char;
  }
  cells.push(current.trim());
  return cells;
}

function parseSimpleperfCsv(csv: string): HotspotRecord[] {
  const text = csv.replace(/^\uFEFF/, "").trim();
  if (!text) return [];
  const rawRow = (): HotspotRecord[] => [{ overhead: "", symbol: text, dso: "", samples: 0 }];
  try {
    const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0);
    if (lines.length === 0) return rawRow();
    const header = splitCsvRow(lines[0]).map((cell) => cell.toLowerCase());
    const overheadIndex = header.findIndex((cell) => cell.includes("overhead"));
    const symbolIndex = header.findIndex((cell) => cell === "symbol" || cell.endsWith(" symbol"));
    const dsoIndex = header.findIndex((cell) => cell.includes("shared object") || cell === "dso" || cell.includes("dso"));
    const samplesIndex = header.findIndex((cell) => cell.includes("sample"));
    if (overheadIndex < 0 && symbolIndex < 0 && dsoIndex < 0) return rawRow();
    return lines.slice(1).map((line) => {
      const cells = splitCsvRow(line);
      const parsedSamples = samplesIndex >= 0 ? Number(String(cells[samplesIndex] ?? "").replace(/,/g, "")) : 0;
      return {
        overhead: overheadIndex >= 0 ? cells[overheadIndex] ?? "" : "",
        symbol: symbolIndex >= 0 ? cells[symbolIndex] ?? "" : "",
        dso: dsoIndex >= 0 ? cells[dsoIndex] ?? "" : "",
        samples: Number.isFinite(parsedSamples) ? parsedSamples : 0,
      };
    });
  } catch {
    return rawRow();
  }
}

export const SimpleperfView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("simpleperf", "serial", "");
  const [packageName, setPackageName] = useToolWindowValue("simpleperf", "packageName", "com.android.systemui");
  const [duration, setDuration] = useToolWindowValue("simpleperf", "duration", () => String(readToolPreferences().simpleperfDuration));
  const [isRunning, setIsRunning] = useState(false);
  const [records, setRecords] = useToolWindowValue<HotspotRecord[]>("simpleperf", "records", []);
  const [sampleInfo, setSampleInfo] = useToolWindowValue<SampleInfo | null>("simpleperf", "sampleInfo", null);
  const [artifactNote, setArtifactNote] = useToolWindowValue<string | null>("simpleperf", "artifactNote", null);
  useToolDetachGuard("simpleperf", isRunning);
  const taskIdRef = useRef<string | null>(null);

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

  const handleCancel = async () => {
    const id = taskIdRef.current;
    if (!id) {
      showToast("没有正在进行的采样任务");
      return;
    }
    try {
      await cancelAndroidTask(id);
      showToast("已请求取消采样");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleStartSampling = async (serialOverride?: string, packageOverride?: string, durationOverride?: string, destinationOverride?: string, actionId?: string) => {
    if (isRunning) return;
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
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Simpleperf 未启动", detail: message, actionId });
      return;
    }
    const destination = destinationOverride || await openDirectory("Simpleperf 产物目录");
    if (!destination) {
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Simpleperf 等待输出目录", detail: "未选择本地输出目录，采样尚未启动。", actionId, progress: 0 });
      return;
    }
    setIsRunning(true);
    setRecords([]);
    setSampleInfo(null);
    setArtifactNote(null);
    try {
      const taskId = await invoke<string>("start_simpleperf_capture", {
        request: {
          serial: targetSerial,
          destination,
          durationSeconds: Number(targetDuration) || 10,
          packageName: targetPackage,
        },
      });
      taskIdRef.current = taskId;
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在执行 Simpleperf 采样", detail: update.message || targetPackage, actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "采样未完成");
      }
      const report = await invoke<{ taskId: string; csv: string; truncated: boolean }>("read_simpleperf_report", { taskId });
      setRecords(parseSimpleperfCsv(report.csv));
      setSampleInfo({
        duration: `${Number(targetDuration) || 10} 秒`,
        packageName: targetPackage.trim() || "未指定包名",
        truncated: report.truncated ? "报告已截断" : "报告完整",
      });
      const note = task.artifactPath
        ? `产物目录：${task.artifactPath}`
        : (task.message || "采样完成，但任务没有返回产物目录");
      setArtifactNote(note);
      showToast(note);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Simpleperf 采样完成", detail: note, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Simpleperf 采样失败", detail: message, actionId });
    } finally {
      taskIdRef.current = null;
      setIsRunning(false);
    }
  };

  useApplicationPageAction("open.tool.simpleperf", (action) => {
    if (isRunning) {
      publishAiTaskActivity({ running: false, status: "error", title: "Simpleperf 正在采样", detail: "请等待当前采样完成。", actionId: action.actionId });
      return;
    }
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (action.parameters.packageName) setPackageName(action.parameters.packageName);
    if (action.parameters.durationSeconds) setDuration(action.parameters.durationSeconds);
    publishAiTaskActivity({ running: true, status: "running", title: "正在准备 Simpleperf 采样", detail: "正在交接设备、包名与采样参数。", actionId: action.actionId, progress: 10 });
    void handleStartSampling(action.parameters.serial, action.parameters.packageName, action.parameters.durationSeconds, action.parameters.destination, action.actionId);
  });

  const perfProps: PropertyItem[] = [
        { icon: <Cpu size={14} />, label: "采样时长", value: sampleInfo?.duration ?? "尚未采样", valueType: "accent" },
        { icon: <Activity size={14} />, label: "目标包名", value: sampleInfo?.packageName ?? "尚未采样" },
        { icon: <CheckCircle2 size={14} />, label: "CSV 是否截断", value: sampleInfo?.truncated ?? "尚未采样", valueType: sampleInfo?.truncated === "报告已截断" ? "orange" : "green" },
      ];

  return (
    <div className="simpleperf-container">

      <header className="simpleperf-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            Native CPU 热点分析 (Simpleperf)
          </h2>
          <Badge variant={selected ? "primary" : "neutral"}>{selected ? deviceOptionLabel(selected) : "无在线设备"}</Badge>
          {tauri && (
            <Select
              size="sm"
              placeholder="选择设备"
              options={devices.map((device) => ({ value: device.serial, label: deviceOptionLabel(device) }))}
              value={serial}
              disabled={isRunning}
              onChange={(next) => { setSerial(next); setRecords([]); setSampleInfo(null); setArtifactNote(null); }}
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
              showToast(artifactNote ?? "尚无采样产物目录，未复制文件");
            }}
          >
            导出原始采样
          </Button>
        </div>
      </header>

      <main className="simpleperf-content custom-scrollbar">
        <PropertyGrid items={perfProps} columns={2} />

        <div className="simpleperf-config-card">
          <div style={{ display: "flex", gap: 12, alignItems: "flex-end", flexWrap: "wrap" }}>
            <div style={{ flex: 1, minWidth: 220 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                目标应用包名 / 进程 PID
              </label>
              <Input
                size="sm"
                placeholder="例如 com.android.systemui"
                value={packageName}
                onChange={(e) => setPackageName(e.target.value)}
              />
            </div>

            <div style={{ width: 140 }}>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                采样时长 (秒)
              </label>
              <Input
                size="sm"
                type="number"
                value={duration}
                onChange={(e) => setDuration(e.target.value)}
              />
            </div>

            <Button
              variant={isRunning ? "danger" : "primary"}
              size="sm"
              icon={isRunning ? <Square size={13} /> : <Play size={13} />}
              onClick={() => void handleStartSampling()}
              disabled={isRunning || !tauri || !serial}
            >
              {isRunning ? "采样中..." : "开始采集 perf.data"}
            </Button>
            {tauri && isRunning && (
              <Button
                variant="danger"
                size="sm"
                icon={<Square size={13} />}
                onClick={() => void handleCancel()}
              >
                取消采样
              </Button>
            )}
          </div>
        </div>

        <div className="simpleperf-table-wrap">
          <div className="simpleperf-table-head">
            <span>占比</span>
            <span>核心函数符号 (Symbol)</span>
            <span>动态库 (DSO)</span>
            <span style={{ textAlign: "right" }}>样本计数</span>
          </div>

          <div>
            {records.length === 0 && <ToolEmptyState title="尚未采样" description="选择在线设备后启动 Simpleperf，热点结果来自采样报告。" />}
            {records.map((r, idx) => (
              <div key={idx} className="simpleperf-table-row">
                <span style={{ fontWeight: 700, color: idx === 0 ? "var(--danger-text)" : idx === 1 ? "var(--warning-text)" : "var(--accent-primary)" }}>
                  {r.overhead}
                </span>
                <span style={{ fontFamily: "var(--font-mono, monospace)" }} title={r.symbol}>
                  {r.symbol}
                </span>
                <span style={{ color: "var(--text-tertiary)" }} title={r.dso}>{r.dso}</span>
                <span style={{ textAlign: "right", fontFamily: "var(--font-mono, monospace)" }}>
                  {r.samples.toLocaleString()}
                </span>
              </div>
            ))}
          </div>
        </div>
      </main>
    </div>
  );
};

export default SimpleperfView;
