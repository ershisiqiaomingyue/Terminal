import { useEffect, useState } from "react";
import { Copy, Download, FolderOpen, RefreshCw } from "lucide-react";
import { Button, Badge } from "@/components/ui";
import { isTauri, formatInvokeError } from "@/platform/runtime";
import { exportDiagnosticLogs, getDiagnosticLogInfo, openDiagnosticLogDirectory, type DiagnosticLogInfo } from "@/platform/diagnosticLogs";
import { useSettingsNotification } from "./SettingsNotification";

export function DiagnosticLogsPanel() {
  const [info, setInfo] = useState<DiagnosticLogInfo>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const notify = useSettingsNotification();
  const refresh = async () => {
    if (!isTauri()) return;
    try { setInfo(await getDiagnosticLogInfo()); setError(""); }
    catch (reason) { setError(formatInvokeError(reason)); }
  };
  useEffect(() => { let active = true; if (isTauri()) void getDiagnosticLogInfo().then(value => { if (active) setInfo(value); }, reason => { if (active) setError(formatInvokeError(reason)); }); return () => { active = false; }; }, []);
  const run = async (action: () => Promise<unknown>, success?: (result: unknown) => string) => {
    setBusy(true);
    try { const result = await action(); if (success) notify(success(result), "success"); }
    catch (reason) { notify(formatInvokeError(reason), "error"); }
    finally { setBusy(false); }
  };
  return <div>
    <div className="setting-group-title">诊断日志</div>
    <div className="setting-card-group">
      <div className="setting-row-item">
        <div className="setting-label-block"><span className="setting-title">日志状态</span><span className="setting-desc">{error || info?.lastError || (isTauri() ? "记录异常、页面心跳、卡顿、WebView 故障及启动退出。" : "诊断日志由桌面程序保存。")}</span></div>
        <Badge variant={info?.enabled ? "success" : error || info?.lastError ? "danger" : "neutral"}>{info?.enabled ? "正在记录" : error || info?.lastError ? "写入异常" : "未检测"}</Badge>
      </div>
      <div className="setting-row-item"><div className="setting-label-block"><span className="setting-title">保存位置</span><span className="setting-desc" style={{ overflowWrap: "anywhere" }}>{info?.directory || "-"}</span></div></div>
      <div className="setting-row-item"><div className="setting-label-block"><span className="setting-title">保留范围</span><span className="setting-desc">{info ? `最近 ${info.retentionDays} 天，最多 ${info.maxFiles} 个文件，每个 ${(info.maxFileBytes / 1024 / 1024).toFixed(0)} MB；当前 ${info.fileCount} 个文件，共 ${(info.totalBytes / 1024 / 1024).toFixed(2)} MB。` : "-"}</span></div></div>
      <div className="setting-row-item"><div className="setting-label-block"><span className="setting-title">本次运行编号</span><span className="setting-desc">{info?.runId || "-"}</span></div></div>
    </div>
    <div className="settings-preference-actions" style={{ flexWrap: "wrap" }}>
      <Button size="sm" variant="surface" icon={<RefreshCw size={14} />} disabled={!isTauri() || busy} onClick={() => void refresh()}>刷新</Button>
      <Button size="sm" variant="surface" icon={<Copy size={14} />} disabled={!info || busy} onClick={() => void run(() => navigator.clipboard.writeText(info!.directory), () => "日志路径已复制。")}>复制路径</Button>
      <Button size="sm" variant="surface" icon={<FolderOpen size={14} />} disabled={!info || busy} onClick={() => void run(openDiagnosticLogDirectory)}>打开日志目录</Button>
      <Button size="sm" variant="surface" icon={<Download size={14} />} disabled={!info || busy} onClick={() => void run(exportDiagnosticLogs, result => `诊断包已保存：${result}`)}>导出诊断包</Button>
    </div>
  </div>;
}
