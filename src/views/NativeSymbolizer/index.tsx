import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import React, { useCallback, useState } from "react";
import {
  Bug,
  Sparkles,
  Copy,
  CheckCircle2,
  FileCode2,
  Terminal,
} from "lucide-react";
import { Button, Input, Badge } from "@/components/ui";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import {
  formatInvokeError,
  invoke,
  isTauri,
  openDirectory,
  openFile,
  waitForTask,
} from "@/platform/runtime";
import "./native-symbolizer.css";

interface SymbolReport {
  output: string;
  ndkStackSymbolCount: number;
  addr2lineSymbolCount: number;
}

export const NativeSymbolizerView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [tombstonePath, setTombstonePath] = useToolWindowValue("native-symbolizer", "tombstonePath", "");
  const [symbolsPath, setSymbolsPath] = useToolWindowValue("native-symbolizer", "symbolsPath", "");
  const [isProcessing, setIsProcessing] = useState(false);
  const [error, setError] = useState("");
  const [symbolizedOutput, setSymbolizedOutput] = useToolWindowValue("native-symbolizer", "symbolizedOutput", "尚未符号化");
  const [symbolCounts, setSymbolCounts] = useToolWindowValue<{ ndk: number; addr: number } | null>("native-symbolizer", "symbolCounts", null);
  useToolDetachGuard("native-symbolizer", isProcessing);

  const showToast = useCallback((msg: string) => {
    notify(msg, "info");
  }, []);

  const handleRunSymbolize = async (tombstoneOverride?: string, symbolsOverride?: string, actionId?: string) => {
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    let tombstone = tombstoneOverride || tombstonePath;
    let symbols = symbolsOverride || symbolsPath;
    if (!tombstone || !symbols) {
      const pickedTombstone = await openFile("选择 Tombstone 文件（txt/log）");
      if (!pickedTombstone) return;
      const pickedSymbols = await openDirectory("未剥离符号目录");
      if (!pickedSymbols) return;
      tombstone = pickedTombstone;
      symbols = pickedSymbols;
      setTombstonePath(pickedTombstone);
      setSymbolsPath(pickedSymbols);
    }
    setIsProcessing(true);
    setError("");
    setSymbolCounts(null);
    setSymbolizedOutput("正在符号化…");
    try {
      const taskId = await invoke<string>("start_native_tombstone_symbolization", {
        request: { tombstonePath: tombstone, symbolsPath: symbols },
      });
      const task = await waitForTask(taskId, (update) => {
        if (update.message) showToast(`${update.progress}% ${update.message}`);
        if (actionId) publishAiTaskActivity({ running: update.status === "running", status: update.status === "done" ? "done" : update.status === "running" ? "running" : "error", title: "正在执行 Native 符号化", detail: update.message || "正在还原 Native 调用栈。", actionId, progress: update.progress });
      });
      if (task.status !== "done") {
        throw new Error(task.error || task.message || "符号化未完成");
      }
      const report = await invoke<SymbolReport>("read_native_symbolization_report", { taskId });
      setSymbolizedOutput(report.output);
      setSymbolCounts({ ndk: report.ndkStackSymbolCount, addr: report.addr2lineSymbolCount });
      showToast(`符号化完成：ndk-stack ${report.ndkStackSymbolCount}，addr2line ${report.addr2lineSymbolCount}`);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Native 符号化完成", detail: `已解析 ${report.ndkStackSymbolCount + report.addr2lineSymbolCount} 个符号。`, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      setError(message);
      setSymbolizedOutput(message);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Native 符号化失败", detail: message, actionId });
    } finally {
      setIsProcessing(false);
    }
  };

  useApplicationPageAction("open.tool.native-symbolizer", (action) => {
    const tombstone = action.parameters.tombstonePath;
    const symbols = action.parameters.symbolsPath;
    if (!tombstone || !symbols) {
      const message = "Native 符号化需要 tombstone 文件和未剥离符号目录；请在页面中选择。";
      notify(message, "warning");
      publishAiTaskActivity({ running: false, status: "error", title: "Native 符号化等待输入", detail: message, actionId: action.actionId });
      return;
    }
    setTombstonePath(tombstone);
    setSymbolsPath(symbols);
    publishAiTaskActivity({ running: true, status: "running", title: "正在执行 Native 符号化", detail: "正在读取 tombstone 与符号目录。", actionId: action.actionId, progress: 10 });
    void handleRunSymbolize(tombstone, symbols, action.actionId);
  });

  const symProps: PropertyItem[] = symbolCounts
      ? [
          { icon: <FileCode2 size={14} />, label: "ndk-stack 符号数", value: String(symbolCounts.ndk), valueType: "green" },
          { icon: <Terminal size={14} />, label: "addr2line 符号数", value: String(symbolCounts.addr), valueType: "accent" },
          { icon: <CheckCircle2 size={14} />, label: "符号化状态", value: "已完成", valueType: "green" },
        ]
      : [
          { icon: <Bug size={14} />, label: "崩溃信号", value: "尚未符号化", valueType: "orange" },
          { icon: <FileCode2 size={14} />, label: "ndk-stack 符号数", value: "尚未符号化", valueType: "green" },
          { icon: <Terminal size={14} />, label: "addr2line 符号数", value: "尚未符号化", valueType: "accent" },
        ];

  return (
    <div className="native-sym-container">

      <header className="native-sym-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            Native 符号化分析 (Tombstone & NDK-Stack)
          </h2>
          <Badge variant="neutral">NDK 工具链：未检测</Badge>
        </div>

        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Button
            variant="secondary"
            size="sm"
            icon={<Copy size={13} />}
            onClick={() => {
              navigator.clipboard.writeText(symbolizedOutput);
              showToast("已复制符号化崩溃栈");
            }}
          >
            复制符号化栈
          </Button>
        </div>
      </header>

      <main className="native-sym-content custom-scrollbar">
        <PropertyGrid items={symProps} columns={2} />

        <div className="native-sym-config-card">
          <div className="native-sym-fields">
            <div>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                系统 Tombstone 文件或崩溃日志路径
              </label>
              <Input
                size="sm"
                value={tombstonePath}
                placeholder="选择本地 Tombstone 文件"
                onChange={(e) => setTombstonePath(e.target.value)}
              />
            </div>

            <div>
              <label style={{ display: "block", fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}>
                未剥离符号目录 (Unstripped Symbols Root)
              </label>
              <Input
                size="sm"
                value={symbolsPath}
                placeholder="选择未剥离符号目录"
                onChange={(e) => setSymbolsPath(e.target.value)}
              />
            </div>

            <Button
              variant="primary"
              size="sm"
              icon={<Sparkles size={13} />}
              onClick={() => void handleRunSymbolize()}
              disabled={isProcessing}
            >
              {isProcessing ? "解析中..." : "还原代码符号栈"}
            </Button>
          </div>
        </div>

        <div className="native-sym-viewport">
          <div className="native-sym-titlebar">
            <span>符号化崩溃回溯追踪 (Symbolized Backtrace)</span>
            <Badge variant={error ? "danger" : symbolCounts ? "success" : "neutral"}>{error ? "符号化失败" : isProcessing ? "正在符号化" : symbolCounts ? "符号化完成，详见输出" : "尚未符号化"}</Badge>
          </div>

          <pre className="native-sym-logs custom-scrollbar">
            {symbolizedOutput}
          </pre>
        </div>
      </main>
    </div>
  );
};

export default NativeSymbolizerView;
