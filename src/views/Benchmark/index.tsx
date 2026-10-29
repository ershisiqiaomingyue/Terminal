import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import React, { useCallback, useEffect, useState } from "react";
import {
  BarChart3,
  Play,
  TrendingDown,
  TrendingUp,
  CheckCircle2,
  FolderOpen,
} from "lucide-react";
import { Button, Badge, Select } from "@/components/ui";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import {
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openFile,
  waitForTask,
  type AdbDevice,
} from "@/platform/runtime";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import "./benchmark.css";

interface BenchmarkComparison {
  caseName: string;
  metric: string;
  current: number | null;
  baseline: number | null;
  change: number | null;
  regression: boolean;
  unit: string;
}

interface BenchmarkMetric {
  name: string;
  median: number | null;
  minimum: number | null;
  maximum: number | null;
  p50: number | null;
  p90: number | null;
  p95: number | null;
  p99: number | null;
}

interface BenchmarkSummary {
  sourcePath: string;
  deviceModel: string | null;
  sdk: number | null;
  benchmarks: Array<{
    name: string;
    className: string;
    metrics: BenchmarkMetric[];
  }>;
}

interface AndroidInstrumentation {
  component: string;
  targetPackage: string;
}

function formatNumber(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return "未提供";
  return Number.isInteger(value) ? String(value) : String(Math.round(value * 100) / 100);
}

function flattenBenchmarks(summary: BenchmarkSummary): BenchmarkComparison[] {
  const rows: BenchmarkComparison[] = [];
  for (const benchmark of summary.benchmarks ?? []) {
    for (const metric of benchmark.metrics ?? []) {
      const current = metric.median ?? metric.p50;
      const baseline = null;
      const change = null;
      rows.push({
        caseName: benchmark.name,
        metric: metric.name,
        current,
        baseline,
        change,
        regression: false,
        unit: "",
      });
    }
  }
  return rows;
}

export const BenchmarkView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("benchmark", "serial", "");
  const [busy, setBusy] = useState(false);
  const [comparisons, setComparisons] = useToolWindowValue<BenchmarkComparison[]>("benchmark", "comparisons", []);
  const [summary, setSummary] = useToolWindowValue<BenchmarkSummary | null>("benchmark", "summary", null);
  useToolDetachGuard("benchmark", busy);
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

  const applySummary = (next: BenchmarkSummary) => {
    const rows = flattenBenchmarks(next);
    setSummary(next);
    setComparisons(rows);
    showToast(`已读取 ${next.sourcePath || "Benchmark 结果"}，${rows.length} 条指标`);
  };

  const readResult = async (taskId: string) => {
    const result = await invoke<BenchmarkSummary>("read_android_benchmark_result", { taskId });
    applySummary(result);
  };

  const handleImport = async (pathOverride?: string, actionId?: string) => {
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    const path = pathOverride || await openFile("导入 AndroidX Benchmark JSON", [
      { name: "JSON / TXT", extensions: ["json", "txt"] },
    ]);
    if (!path) return;
    setBusy(true);
    setSummary(null);
    setComparisons([]);
    try {
      const taskId = await invoke<string>("import_android_benchmark_result", { path });
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在导入 Benchmark 结果", detail: update.message || "正在读取结果。", actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "导入未完成");
      }
      await readResult(taskId);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Benchmark 结果已导入", detail: path, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Benchmark 导入失败", detail: message, actionId });
    } finally {
      setBusy(false);
    }
  };

  const handleRun = async (serialOverride?: string, actionId?: string) => {
    const targetSerial = serialOverride || serial;
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial) {
      const message = "请先选择在线设备";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Benchmark 未启动", detail: message, actionId });
      return;
    }
    setBusy(true);
    setSummary(null);
    setComparisons([]);
    try {
      const instrumentations = await invoke<AndroidInstrumentation[]>("list_android_instrumentations", { serial: targetSerial });
      if (!instrumentations.length) {
        showToast("当前设备没有 instrumentation");
        return;
      }
      const chosen = instrumentations.find((item) => item.component.toLowerCase().includes("benchmark")) ?? instrumentations[0];
      showToast(`正在运行 ${chosen.component}`);
      const taskId = await invoke<string>("start_android_benchmark_run", {
        request: { serial: targetSerial, instrumentation: chosen.component },
      });
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在执行 AndroidX Benchmark", detail: update.message || chosen.component, actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "基准测试未完成");
      }
      await readResult(taskId);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "AndroidX Benchmark 完成", detail: chosen.component, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "AndroidX Benchmark 失败", detail: message, actionId });
    } finally {
      setBusy(false);
    }
  };

  useApplicationPageAction("open.tool.benchmark", (action) => {
    const operation = action.parameters.operation || (action.parameters.path ? "import" : "run");
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (operation === "import" && !action.parameters.path) {
      const message = "导入 Benchmark 结果需要已知的本地文件路径；请在页面中选择文件。";
      notify(message, "warning");
      publishAiTaskActivity({ running: false, status: "error", title: "Benchmark 等待输入", detail: message, actionId: action.actionId });
      return;
    }
    publishAiTaskActivity({ running: true, status: "running", title: operation === "import" ? "正在导入 Benchmark 结果" : "正在启动 AndroidX Benchmark", detail: "已交接给基准分析页面。", actionId: action.actionId, progress: 10 });
    if (operation === "import") void handleImport(action.parameters.path, action.actionId);
    else void handleRun(action.parameters.serial, action.actionId);
  });

  const benchmarkProps: PropertyItem[] = summary
      ? [
          { icon: <BarChart3 size={14} />, label: "设备型号", value: summary.deviceModel || "未知型号", valueType: "accent" },
          { icon: <TrendingUp size={14} />, label: "SDK", value: summary.sdk == null ? "未知" : String(summary.sdk) },
          { icon: <CheckCircle2 size={14} />, label: "结果来源", value: summary.sourcePath || "未知路径", valueType: "green" },
        ]
      : [
          { icon: <BarChart3 size={14} />, label: "设备型号", value: "尚未导入", valueType: "accent" },
          { icon: <TrendingUp size={14} />, label: "SDK", value: "尚未导入" },
          { icon: <CheckCircle2 size={14} />, label: "结果来源", value: "尚未导入", valueType: "green" },
        ];

  return (
    <div className="benchmark-container">

      <header className="benchmark-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            基准与帧卡顿分析 (Macrobenchmark)
          </h2>
          <Badge variant={selected ? "primary" : "neutral"}>{selected ? deviceOptionLabel(selected) : "无在线设备"}</Badge>
          {tauri && (
            <Select
              size="sm"
              placeholder="选择设备"
              options={devices.map((device) => ({ value: device.serial, label: deviceOptionLabel(device) }))}
              value={serial}
              onChange={setSerial}
              style={{ width: 260, flex: "0 0 260px" }}
            />
          )}
        </div>

        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Button
            variant="secondary"
            size="sm"
            icon={<FolderOpen size={13} />}
            onClick={() => void handleImport()}
            disabled={!tauri || busy}
          >
            导入 JSON 测试结果
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon={<Play size={13} />}
            onClick={() => void handleRun()}
            disabled={!tauri || busy}
          >
            {tauri && busy ? "处理中..." : "运行基准测试"}
          </Button>
        </div>
      </header>

      <main className="benchmark-content custom-scrollbar">
        <PropertyGrid items={benchmarkProps} columns={2} />

        <div className="benchmark-table-wrap">
          <div className="benchmark-table-head">
            <span>基准测试用例</span>
            <span>核心指标</span>
            <span>本次度量</span>
            <span>基线基准</span>
            <span style={{ textAlign: "right" }}>性能波动对比</span>
          </div>

          <div>
            {comparisons.length === 0 && <ToolEmptyState title="尚未导入基准结果" description="导入 AndroidX Benchmark JSON 或连接设备运行测试。" />}
            {comparisons.map((c, idx) => (
              <div key={idx} className="benchmark-table-row">
                <span style={{ fontWeight: 600, color: "var(--text-primary)" }}>{c.caseName}</span>
                <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{c.metric}</span>
                <span style={{ fontFamily: "var(--font-mono, monospace)", fontWeight: 600 }}>
                  {formatNumber(c.current)}{c.unit ? ` ${c.unit}` : ""}
                </span>
                <span style={{ fontFamily: "var(--font-mono, monospace)", color: "var(--text-tertiary)" }}>
                  {formatNumber(c.baseline)}{c.unit ? ` ${c.unit}` : ""}
                </span>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 6 }}>
                  {c.change === null ? <Badge variant="neutral">未导入历史基线</Badge> : c.regression ? (
                    <Badge variant="danger">
                      <TrendingUp size={11} style={{ marginRight: 3 }} />
                      +{c.change.toFixed(1)}% 退化
                    </Badge>
                  ) : (
                    <Badge variant="success">
                      <TrendingDown size={11} style={{ marginRight: 3 }} />
                      {c.change.toFixed(1)}% 提升
                    </Badge>
                  )}
                </div>
              </div>
            ))}
          </div>
        </div>
      </main>
    </div>
  );
};

export default BenchmarkView;
