import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  CircleAlert,
  Download,
  FolderOpen,
  LoaderCircle,
  RefreshCw,
  Wrench,
} from "lucide-react";
import { Badge, Button } from "@/components/ui";
import {
  checkEnvironmentTool,
  downloadEnvironmentTool,
  listEnvironmentTools,
  saveEnvironmentToolPath,
  type EnvironmentToolDownloadProgress,
  type EnvironmentToolStatus,
} from "@/platform/api";
import { formatInvokeError, isTauri, listen, openFile, type DialogFilter } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";

const FILE_FILTERS: Record<string, DialogFilter[]> = {
  adb: [{ name: "ADB", extensions: ["exe"] }],
  aapt2: [{ name: "aapt2", extensions: ["exe"] }],
  java: [{ name: "Java", extensions: ["exe"] }],
  apktool: [{ name: "Java Archive", extensions: ["jar"] }],
  jadx: [{ name: "JADX", extensions: ["bat", "cmd"] }],
  bundletool: [{ name: "Java Archive", extensions: ["jar"] }],
  scrcpy: [{ name: "Scrcpy", extensions: ["exe"] }],
  gradle: [{ name: "Gradle", extensions: ["bat", "cmd", "exe"] }],
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
  if (progress.phase === "resolving") return "正在获取下载信息...";
  if (progress.phase === "installing") return "正在安装并校验文件...";
  if (progress.totalBytes && progress.totalBytes > 0) {
    const percent = Math.min(100, Math.floor((progress.downloadedBytes / progress.totalBytes) * 100));
    return `正在下载 ${formatByteSize(progress.downloadedBytes)} / ${formatByteSize(progress.totalBytes)} (${percent}%)`;
  }
  return `正在下载 ${formatByteSize(progress.downloadedBytes)}`;
}

function downloadProgressPercent(progress: ToolDownloadState) {
  if (!progress.totalBytes || progress.totalBytes <= 0) return null;
  return Math.min(100, (progress.downloadedBytes / progress.totalBytes) * 100);
}

export const EnvironmentToolsPanel: React.FC = () => {
  const notify = useSettingsNotification();
  const [tools, setTools] = useState<EnvironmentToolStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [choosingToolIds, setChoosingToolIds] = useState<Record<string, true>>({});
  const [checkingToolIds, setCheckingToolIds] = useState<Record<string, true>>({});
  const [downloads, setDownloads] = useState<Record<string, ToolDownloadState>>({});
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
      notify("请在桌面应用中检查环境工具。", "error");
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
      if (announce) notify("环境工具状态已刷新。", "success");
    } catch (reason) {
      if (requestId === refreshRequestId.current) notify(formatInvokeError(reason), "error");
    } finally {
      if (requestId === refreshRequestId.current) setLoading(false);
    }
  }, [notify]);

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
      notify(`${tool.name} 已切换为选择的本地文件。`, "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
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
      notify(checked.localStatus === "ready" ? `${tool.name} 检测通过。` : `${tool.name}：${checked.localMessage}`, checked.localStatus === "ready" ? "success" : "error");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
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
      notify(`${tool.name} 已下载并配置。`, "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      removeDownload(tool.id);
    }
  };

  return (
    <section className="settings-pane">
      <header className="pane-header settings-environment-header">
        <div>
          <h2 className="pane-title">环境与工具</h2>
          <p className="settings-environment-intro">检测应用会实际调用的本地工具，并从官方发布源检查可用版本。</p>
        </div>
        <Button
          size="sm"
          variant="secondary"
          icon={<RefreshCw size={14} />}
          loading={loading}
          onClick={() => void refresh(true)}
        >
          刷新
        </Button>
      </header>

      <div>
        <div className="setting-group-title">工具链与脚本运行时</div>
        <div className="setting-card-group settings-environment-list">
          {loading && !tools.length && (
            <div className="settings-environment-loading"><LoaderCircle size={16} />正在检测本地环境与远端版本...</div>
          )}
          {tools.map((tool) => {
            const download = downloads[tool.id];
            const downloading = Boolean(download);
            const choosing = Boolean(choosingToolIds[tool.id]);
            const checking = Boolean(checkingToolIds[tool.id]);
            const busy = downloading || choosing || checking;
            const missing = tool.localStatus === "missing";
            const downloadPercent = download ? downloadProgressPercent(download) : null;
            return (
              <article className="settings-environment-tool" key={tool.id}>
                <div className={`settings-environment-tool-icon is-${tool.localStatus}`}>
                  {tool.localStatus === "ready" ? <CheckCircle2 size={16} /> : tool.localStatus === "available" ? <CircleAlert size={16} /> : <Wrench size={16} />}
                </div>
                <div className="settings-environment-tool-main">
                  <div className="settings-environment-tool-title">
                    <strong>{tool.name}</strong>
                    {localBadge(tool)}
                    {tool.id === "gradle" && <Badge variant="neutral">项目优先</Badge>}
                  </div>
                  <span>{tool.description}</span>
                  {(tool.localVersion || tool.remoteVersion || tool.remoteMessage) && <div className="settings-environment-tool-meta">
                    {tool.localVersion && <span title={tool.localPath || tool.localMessage}>
                      {tool.localVersion}
                    </span>}
                    {tool.remoteVersion && <span>远端 {tool.remoteVersion}</span>}
                    {!tool.remoteVersion && tool.remoteMessage && <span className="is-remote-error">远端版本暂不可用</span>}
                  </div>}
                  {download && (
                    <div className="settings-environment-download-progress" role="status" aria-live="polite">
                      <span>{downloadProgressLabel(download)}</span>
                      <div className="settings-environment-download-track" aria-hidden="true">
                        <span
                          className={downloadPercent === null ? "is-indeterminate" : undefined}
                          style={downloadPercent === null ? undefined : { width: `${downloadPercent}%` }}
                        />
                      </div>
                    </div>
                  )}
                  {tool.localPath && <code title={tool.localPath}>{tool.localPath}</code>}
                </div>
                <div className="settings-environment-tool-actions">
                  <Button
                    size="xs"
                    variant="subtle"
                    icon={<RefreshCw size={13} />}
                    loading={checking}
                    disabled={busy || loading}
                    onClick={() => void checkTool(tool)}
                    title={`重新检测 ${tool.name} 的本地可用性`}
                  >
                    重新检测
                  </Button>
                  <Button
                      size="xs"
                      variant="secondary"
                      icon={<FolderOpen size={13} />}
                      disabled={busy}
                      loading={choosing}
                      onClick={() => void chooseLocalTool(tool)}
                      title={tool.selectionHint}
                    >
                      选择本地
                    </Button>
                  {tool.supportsDownload && (
                    <Button
                      size="xs"
                      variant="primary"
                      icon={<Download size={13} />}
                      loading={downloading}
                      disabled={choosing || checking}
                      onClick={() => void downloadTool(tool)}
                    >
                      {downloading ? (download?.phase === "downloading" && downloadPercent !== null ? `下载 ${Math.floor(downloadPercent)}%` : "下载中") : missing ? "下载" : "下载最新版"}
                    </Button>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      </div>

      <p className="settings-environment-footnote">下载会保存到 %LOCALAPPDATA%\Android Toolkit\tools；Gradle 项目优先使用项目目录中的 gradlew / gradlew.bat，也可在此选择系统 Gradle。</p>
    </section>
  );
};

export default EnvironmentToolsPanel;
