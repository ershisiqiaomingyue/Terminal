import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Download,
  FolderOpen,
  LoaderCircle,
  RefreshCw,
  Wrench,
} from "lucide-react";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
  Badge,
  Button,
  Progress,
} from "rain-ui";
import {
  checkEnvironmentTool,
  downloadEnvironmentTool,
  listEnvironmentTools,
  saveEnvironmentToolPath,
  type EnvironmentToolDownloadProgress,
  type EnvironmentToolStatus,
} from "@/platform/api";
import { formatInvokeError, isTauri, listen, openFile, type DialogFilter } from "@/platform/runtime";
import { SettingsCard, SettingsPane } from "./settingsUI";
import { usePortal } from "rain-ui";
import "./download-progress.css";

const FILE_FILTERS: Record<string, DialogFilter[]> = {
  python: [{ name: "Python", extensions: ["exe"] }],
  node: [{ name: "Node.js", extensions: ["exe"] }],
};

function localBadge(status: EnvironmentToolStatus) {
  if (status.localStatus === "ready") return <Badge variant="success">可用</Badge>;
  if (status.localStatus === "available") return <Badge variant="warning">检测失败</Badge>;
  return <Badge variant="neutral">未安装</Badge>;
}

interface ToolDownloadState {
  phase: EnvironmentToolDownloadProgress["phase"];
  downloadedBytes: number;
  totalBytes?: number | null;
}

function formatByteSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

function downloadProgressLabel(progress: ToolDownloadState) {
  if (progress.phase === "resolving") return "正在获取下载信息…";
  if (progress.phase === "installing") return "正在安装并校验…";
  if (progress.totalBytes && progress.totalBytes > 0) {
    const percent = Math.min(100, Math.floor((progress.downloadedBytes / progress.totalBytes) * 100));
    return `正在下载 ${formatByteSize(progress.downloadedBytes)} / ${formatByteSize(progress.totalBytes)}（${percent}%）`;
  }
  return `正在下载 ${formatByteSize(progress.downloadedBytes)}`;
}

function downloadProgressPercent(progress: ToolDownloadState) {
  if (!progress.totalBytes || progress.totalBytes <= 0) return null;
  return Math.min(100, (progress.downloadedBytes / progress.totalBytes) * 100);
}

export const EnvironmentToolsPanel: React.FC = () => {
  const portal = usePortal();
  const [tools, setTools] = useState<EnvironmentToolStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [choosingToolIds, setChoosingToolIds] = useState<Record<string, true>>({});
  const [checkingToolIds, setCheckingToolIds] = useState<Record<string, true>>({});
  const [downloads, setDownloads] = useState<Record<string, ToolDownloadState>>({});
  const [openTools, setOpenTools] = useState<string[]>([]);
  const initialRefreshStarted = useRef(false);
  const refreshRequestId = useRef(0);

  const removeDownload = (toolId: string) => {
    setDownloads((current) => {
      const { [toolId]: _removed, ...remaining } = current;
      return remaining;
    });
  };

  const setChoosing = (toolId: string, choosing: boolean) => {
    setChoosingToolIds((current) => {
      if (choosing) return { ...current, [toolId]: true };
      const { [toolId]: _removed, ...remaining } = current;
      return remaining;
    });
  };

  const refresh = useCallback(async (announce = false) => {
    const requestId = ++refreshRequestId.current;
    if (!isTauri()) {
      setLoading(false);
      portal("请在桌面应用中检查环境工具。", "error");
      return;
    }
    setLoading(true);
    try {
      const local = await listEnvironmentTools(false);
      if (requestId !== refreshRequestId.current) return;
      setTools(local);
      const withRemoteVersion = await listEnvironmentTools(true);
      if (requestId !== refreshRequestId.current) return;
      setTools(withRemoteVersion);
      if (announce) portal("环境工具状态已刷新。", "success");
    } catch (reason) {
      if (requestId === refreshRequestId.current) portal(formatInvokeError(reason), "error");
    } finally {
      if (requestId === refreshRequestId.current) setLoading(false);
    }
  }, [portal]);

  useEffect(() => {
    if (initialRefreshStarted.current) return;
    initialRefreshStarted.current = true;
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!isTauri()) return;
    let stopped = false;
    let unlisten = () => {};
    void listen<EnvironmentToolDownloadProgress>("environment-tool-download-progress", (progress) => {
      if (stopped) return;
      if (progress.phase === "completed" || progress.phase === "failed") {
        removeDownload(progress.toolId);
        return;
      }
      setDownloads((current) => ({
        ...current,
        [progress.toolId]: {
          phase: progress.phase,
          downloadedBytes: progress.downloadedBytes,
          totalBytes: progress.totalBytes,
        },
      }));
    }).then((stop) => {
      if (stopped) stop();
      else unlisten = stop;
    }).catch(() => undefined);
    return () => {
      stopped = true;
      unlisten();
    };
  }, []);

  const replaceTool = (next: EnvironmentToolStatus) => {
    setTools((current) => current.map((tool) => tool.id === next.id ? next : tool));
  };

  const chooseLocalTool = async (tool: EnvironmentToolStatus) => {
    try {
      const path = await openFile(`选择 ${tool.name}`, FILE_FILTERS[tool.id]);
      if (!path) return;
      setChoosing(tool.id, true);
      replaceTool(await saveEnvironmentToolPath(tool.id, path));
      portal(`${tool.name} 已切换为选择的本地文件。`, "success");
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setChoosing(tool.id, false);
    }
  };

  const checkTool = async (tool: EnvironmentToolStatus) => {
    if (checkingToolIds[tool.id]) return;
    setCheckingToolIds((current) => ({ ...current, [tool.id]: true }));
    try {
      const checked = await checkEnvironmentTool(tool.id);
      replaceTool({ ...checked, remoteVersion: tool.remoteVersion, remoteMessage: tool.remoteMessage });
      portal(checked.localStatus === "ready" ? `${tool.name} 检测通过。` : `${tool.name}：${checked.localMessage}`, checked.localStatus === "ready" ? "success" : "error");
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setCheckingToolIds((current) => {
        const { [tool.id]: _removed, ...remaining } = current;
        return remaining;
      });
    }
  };

  const downloadTool = async (tool: EnvironmentToolStatus) => {
    if (downloads[tool.id]) return;
    setDownloads((current) => ({
      ...current,
      [tool.id]: { phase: "resolving", downloadedBytes: 0, totalBytes: null },
    }));
    try {
      replaceTool(await downloadEnvironmentTool(tool.id));
      portal(`${tool.name} 已下载并配置。`, "success");
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      removeDownload(tool.id);
    }
  };

  return (
    <SettingsPane title="环境">
      <SettingsCard>
        {loading && !tools.length ? (
          <div className="flex items-center gap-2 px-[17px] py-5 text-xs text-muted-foreground">
            <LoaderCircle className="animate-spin" size={15} />正在检测环境…
          </div>
        ) : (
          <Accordion type="multiple" value={openTools} onValueChange={setOpenTools}>
            {tools.map((tool) => {
              const download = downloads[tool.id];
              const downloading = Boolean(download);
              const choosing = Boolean(choosingToolIds[tool.id]);
              const checking = Boolean(checkingToolIds[tool.id]);
              const busy = downloading || choosing || checking;
              const missing = tool.localStatus === "missing";
              const downloadPercent = download ? downloadProgressPercent(download) : null;
              return (
                <AccordionItem value={tool.id} key={tool.id}>
                  <AccordionTrigger className="group px-[17px] py-3 hover:no-underline [&>svg:last-child]:hidden">
                    <div className="flex min-w-0 flex-1 items-center gap-3 text-left">
                      <span className={`environment-status-icon is-${tool.localStatus}`}>
                        {tool.localStatus === "ready" ? <CheckCircle2 size={16} /> : tool.localStatus === "available" ? <CircleAlert size={16} /> : <Wrench size={16} />}
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="flex flex-wrap items-center gap-2">
                          <span className="text-[13px] font-medium text-foreground">{tool.name}</span>
                          {localBadge(tool)}
                        </span>
                        <span className="mt-1 block truncate text-xs font-normal text-muted-foreground">
                          {tool.localVersion || tool.localMessage}
                        </span>
                      </span>
                      <ChevronRight size={15} className="text-muted-foreground transition-transform duration-200 group-data-[state=open]:rotate-90" />
                    </div>
                  </AccordionTrigger>
                  <AccordionContent className="px-[17px] pb-4 pt-1">
                    <div className="environment-expanded">
                      <p>{tool.description}</p>

                      <div className="environment-detail-grid">
                        <div className="environment-detail">
                          <span>本地版本</span>
                          <strong>{tool.localVersion || "未检测到"}</strong>
                        </div>
                        <div className="environment-detail">
                          <span>远端版本</span>
                          <strong>{tool.remoteVersion || (tool.remoteMessage ? "暂不可用" : "未检查")}</strong>
                        </div>
                      </div>

                      <div className="environment-path">
                        <span>本地路径</span>
                        <code title={tool.localPath ?? undefined}>{tool.localPath || "尚未选择本地文件"}</code>
                      </div>

                      {download && (
                        <div className="environment-download" role="status" aria-live="polite">
                          <div className="environment-download-label">
                            <span>{downloadProgressLabel(download)}</span>
                            {downloadPercent !== null && <span>{Math.floor(downloadPercent)}%</span>}
                          </div>
                          <Progress value={downloadPercent ?? 0} />
                        </div>
                      )}

                      <div className="environment-actions">
                        <div className="flex flex-wrap gap-2">
                          <Button
                            size="sm"
                            variant="ghost"
                            icon={<RefreshCw size={13} />}
                            loading={checking}
                            disabled={busy || loading}
                            onClick={() => void checkTool(tool)}
                          >
                            重新检测
                          </Button>
                          <Button
                            size="sm"
                            variant="outline"
                            icon={<FolderOpen size={13} />}
                            disabled={busy}
                            loading={choosing}
                            onClick={() => void chooseLocalTool(tool)}
                            title={tool.selectionHint}
                          >
                            选择本地
                          </Button>
                        </div>
                        {tool.supportsDownload && (
                          <Button
                            size="sm"
                            variant="default"
                            icon={<Download size={13} />}
                            loading={downloading}
                            disabled={choosing || checking}
                            onClick={() => void downloadTool(tool)}
                          >
                            {downloading ? "下载中" : missing ? "下载" : "下载最新版"}
                          </Button>
                        )}
                      </div>
                    </div>
                  </AccordionContent>
                </AccordionItem>
              );
            })}
          </Accordion>
        )}
      </SettingsCard>
    </SettingsPane>
  );
};

export default EnvironmentToolsPanel;

