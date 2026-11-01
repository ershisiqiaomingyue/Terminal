import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  Copy,
  Eye,
  EyeOff,
  Folder,
  FolderOpen,
  GitBranch,
  Link2,
  ListTree,
  LocateFixed,
  MemoryStick,
  Minus,
  MoreVertical,
  Network,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  X,
  Cpu,
  Download,
  ExternalLink,
  FolderUp,
  Gauge,
  Save,
  History,
  MoreHorizontal,
  Upload,
} from "lucide-react";
import { formatBytes, formatCpuTime, formatRate, networkRates, readHostStatus, type HostStatusSnapshot } from "@/platform/hostStatus";
import { invoke, formatInvokeError, isTauri } from "@/platform/runtime";
import { systemStatusMetrics } from "@/components/Sidebar/systemStatusMetrics";
import { buildMutationRequest } from "../CodeViewer/mutationRequest";
import { ContextMenu } from "rain-ui";
import { notify } from "@/platform/notifications";
import { Button, StepSpinner } from "rain-ui";
import {
  listLocalDirectory,
  listSshDirectory,
  readRemoteTextFile,
  readSshWorkspaceGit,
  readSshWorkingDirectory,
  readWorkspaceGit,
  readWorkspaceGitChanges,
  type DirectoryEntry,
  type WorkspaceGitChange,
  type WorkspaceGitSummary,
} from "@/platform/terminalWorkspace";
import {
  inspectPreviewEntry,
  type PreviewSource,
} from "@/platform/terminalFilePreview";
import { MAX_HIGHLIGHT_BYTES } from "@/views/CodeViewer/sourceHighlight";
import { CodeHighlightEditor } from "@/views/CodeViewer/CodeHighlightEditor";
import { CodeBrowserControls } from "@/views/CodeViewer/CodeBrowserControls";
import type { FileMetadata } from "@/views/CodeViewer/FileMetadataPopover";
import { CodeFileTree } from "@/views/CodeViewer/CodeFileTree";
import { useCodeBrowser, type BrowserEntry, type BrowserSnapshot, type BrowserTarget } from "@/views/CodeViewer/useCodeBrowser";
// 右键菜单与文件预览页 1:1：同一套确认弹窗流程（重命名/新建/永久删除/移动），
// 同一套"在新标签打开目录"通路。用户要求「右键菜单的功能全部都要 copy 过来」。
import { useFileMutations } from "@/views/CodeViewer/useFileMutations";
import { pathInMutation } from "@/views/CodeViewer/mutationState";
import { CODE_VIEWER_OPEN_TAB_EVENT } from "@/views/CodeViewer/events";
import { isToolWindow, openToolWindowWithState } from "@/platform/toolWindows";
import { MarkdownFilePreview } from "@/views/CodeViewer/MarkdownFilePreview";
import { FileTabStrip } from "@/views/CodeViewer/FileTabStrip";
import { fileName, isMarkdownFile } from "@/views/CodeViewer/fileTabs";
import { DownloadHistory, UploadHistory } from "@/views/CodeViewer/DownloadHistoryPanel";
import { useTransferActions } from "@/views/CodeViewer/useTransferActions";
import { ToolbarPopover } from "@/views/CodeViewer/ToolbarPopover";
import { Switch } from "rain-ui";
import { FloatingWindow } from "./FloatingWindow";
import type { DrawerPane, TerminalTab } from "./types";
// 右栏 rail 激活态等补充样式；单独文件是为了不碰 terminal-workspace.css 里的禁区。
import "./drawer-bindings.css";

interface TerminalDrawerProps {
  pane: DrawerPane;
  activeTab: TerminalTab | null;
  /** 本机工作目录兜底：本地/新建会话还没回填 root 时用它 */
  fallbackRoot: string;
  /**
   * 当前活跃**会话**解析出的 SSH connectionId（可选兜底）。
   *
   * `activeTab.profileId` 在大多数路径下已经被赋上（从已保存服务器/新建连接开
   * 页签时都会传），但恢复出来的旧页签可能没有它。这时由 index.tsx 从
   * sessionsRef / 连接表反查出真实 id 传进来，右栏才有东西可用——
   * 拿不到就明确报错，绝不把空串丢给后端。
   */
  sshConnectionId?: string;
  onClose: () => void;
}

const PANE_TITLE: Record<DrawerPane, string> = {
  host: "主机状态",
  git: "版本控制",
  files: "文件浏览",
};

const PANE_ICON: Record<DrawerPane, React.ReactNode> = {
  host: <Gauge size={13} />,
  git: <GitBranch size={13} />,
  // 用户明确要求用普通文件夹图标，不要 FolderTree。
  files: <Folder size={13} />,
};

/**
 * 解析右栏要用的 SSH connectionId。
 *
 * 顺序：`activeTab.profileId`（页签绑定）→ 会话反查结果（index.tsx 透传）。
 * 两者都没有时返回 undefined，调用方据此显示「未绑定」而不是发一个空 id 出去。
 */
function resolveSshConnectionId(activeTab: TerminalTab | null, fallback?: string): string | undefined {
  const fromTab = activeTab?.profileId?.trim();
  if (fromTab) return fromTab;
  const fromSession = fallback?.trim();
  return fromSession || undefined;
}

/** 右栏：按 pane 切换内容，全部走真实数据源。 */
const TerminalDrawerImpl: React.FC<TerminalDrawerProps> = ({
  pane,
  activeTab,
  fallbackRoot,
  sshConnectionId,
  onClose,
}) => {
  // 只在 SSH 页签下解析；其他类型返回 undefined，避免把别的东西当连接 id 用。
  const connectionId = activeTab?.type === "ssh" ? resolveSshConnectionId(activeTab, sshConnectionId) : undefined;
  // Git 面板的跟随开关放在头部 ⋮ 里（− 左侧，v3 §22 九轮）：状态在抽屉层，
  // GitPane 经 prop 消费；设置会话级保存（DRAWER_FOLLOW）。
  const [followGit, setFollowGit] = useState(() => DRAWER_FOLLOW.git);
  const [gitMenu, setGitMenu] = useState<{ x: number; y: number; trigger: HTMLElement } | null>(null);
  return (
    <aside id="terminal-drawer-panel" className="terminal-drawer" aria-label={PANE_TITLE[pane]}>
      <div className="terminal-drawer-head">
        <span className="terminal-drawer-title">
          {PANE_ICON[pane]}
          {PANE_TITLE[pane]}
        </span>
        {pane === "git" && (
          <Button
            type="button" variant="bare"
            className="terminal-drawer-close"
            aria-label="面板选项"
            title="面板选项"
            aria-haspopup="menu"
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              setGitMenu({ x: rect.left, y: rect.bottom, trigger: event.currentTarget });
            }}
          >
            <MoreVertical size={13} />
          </Button>
        )}
        <Button type="button" variant="bare" className="terminal-drawer-close" aria-label="收起面板" title="收起" onClick={onClose}>
          <Minus size={13} />
        </Button>
      </div>
      {gitMenu && pane === "git" && (
        <ContextMenu
          x={gitMenu.x}
          y={gitMenu.y}
          label="Git 面板选项"
          trigger={gitMenu.trigger}
          onClose={() => setGitMenu(null)}
          items={[
            {
              label: "Git 跟随终端目录",
              icon: <LocateFixed size={14} />,
              checked: followGit,
              onSelect: () => {
                DRAWER_FOLLOW.git = !DRAWER_FOLLOW.git;
                setFollowGit(DRAWER_FOLLOW.git);
              },
            },
          ]}
        />
      )}
      <div className="terminal-drawer-body">
        {pane === "host" && <HostPane activeTab={activeTab} connectionId={connectionId} />}
        {pane === "git" && <GitPane activeTab={activeTab} fallbackRoot={fallbackRoot} connectionId={connectionId} followTerminal={followGit} />}
        {pane === "files" && (
          <FilesPane
            activeTab={activeTab}
            fallbackRoot={fallbackRoot}
            connectionId={connectionId}
          />
        )}
      </div>
    </aside>
  );
};

/**
 * 抽屉数据缓存（SWR）：切走再切回**直接显示上次数据**，挂载后照常后台刷新；
 * 只有第一次（无缓存）才显示加载态——用户不再"每次进来都看转圈"。
 * 缓存随模块存活（切页签不丢），上限 32 条按最旧淘汰。
 */
const drawerDataCache = new Map<string, { at: number; data: unknown }>();

function readDrawerCache<T>(key: string): T | null {
  const hit = drawerDataCache.get(key);
  return hit ? (hit.data as T) : null;
}

function writeDrawerCache(key: string, data: unknown): void {
  drawerDataCache.set(key, { at: Date.now(), data });
  if (drawerDataCache.size > 32) {
    let oldestKey: string | null = null;
    let oldestAt = Number.POSITIVE_INFINITY;
    for (const [key, entry] of drawerDataCache) {
      if (entry.at < oldestAt) {
        oldestAt = entry.at;
        oldestKey = key;
      }
    }
    if (oldestKey) drawerDataCache.delete(oldestKey);
  }
}

function useRefresh(run: () => void, deps: React.DependencyList, intervalMs?: number) {
  useEffect(() => {
    run();
    if (!intervalMs) return;
    const timer = window.setInterval(run, intervalMs);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

/**
 * 主机状态面板（需求 4）：**跟随激活终端切换数据源**。
 *
 * 三种会话拿到的东西天然不同，所以统一成 `RemoteStatus` 形状后交给
 * `systemStatusMetrics()` 渲染——那套函数已经在 `SystemStatusIndicator` 里
 * 处理过内存/磁盘/CPU 核数线程/负载/延迟，不重写第二份。
 *
 * - local：`readHostStatus`（Windows 原生采集，含进程与端口）
 * - ssh：`getSshStatus`（agent-core 的 SshRemoteStatus，含延迟/核数/内存/磁盘）
 *
 * 说明：网速上下行、线程占用明细在三种数据源里都**没有**采集能力，
 * 这里如实显示"暂不支持"，不编造数值。
 */
/** 负载小圆环（轻量 SVG，避免为三个小环引入带弹层的 ProgressRing）。 */
const LoadRing: React.FC<{ label: string; value: number | null; percent: number | null }> = ({ label, value, percent }) => {
  const clamped = Math.max(0, Math.min(100, percent ?? 0));
  const tone = percent == null ? "is-unknown" : percent >= 90 ? "is-danger" : percent >= 70 ? "is-warning" : "";
  const radius = 13;
  const circumference = 2 * Math.PI * radius;
  return (
    <div
      className={`terminal-load-ring ${tone}`}
      title={`${label}负载：${value ?? "-"}${percent == null ? "" : `（约 ${Math.round(percent)}%）`}`}
    >
      <svg viewBox="0 0 40 40" width={36} height={36} aria-hidden="true">
        <circle cx="20" cy="20" r={radius} className="terminal-load-ring-track" />
        <circle
          cx="20"
          cy="20"
          r={radius}
          className="terminal-load-ring-fill"
          strokeDasharray={`${(clamped / 100) * circumference} ${circumference}`}
        />
      </svg>
      <b className="terminal-load-ring-value">{value == null ? "-" : value >= 100 ? String(Math.round(value)) : String(Math.round(value * 10) / 10)}</b>
      <span className="terminal-load-ring-label">{label}</span>
    </div>
  );
};

/**
 * SSH 远端状态富视图（v3）：延迟+网速、每核热力条、内存分段条、负载圆环、
 * 关注挂载点（/ 与 /data）、CPU TOP 进程排名。字段全部可选——远端是受限 shell
 * 时缺哪块就整块不渲染，不写占位文案。
 */
/** 统计格专用紧凑速率："2.1 KB/s" → "2.1K/s"（完整格式在 title 悬停里）。 */
function compactRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0B/s";
  if (bytesPerSecond < 1024) return `${Math.round(bytesPerSecond)}B/s`;
  const kb = bytesPerSecond / 1024;
  if (kb < 1024) return `${kb >= 100 ? Math.round(kb) : Math.round(kb * 10) / 10}K/s`;
  const mb = kb / 1024;
  return `${mb >= 100 ? Math.round(mb) : Math.round(mb * 10) / 10}M/s`;
}

const SshRichStatus: React.FC<{ remote: Record<string, unknown> }> = ({ remote }) => {
  const num = (value: unknown): number | null =>
    typeof value === "number" && Number.isFinite(value) ? value : null;
  const latencyMs = num(remote.latencyMs);
  const rxRate = num(remote.netRxRateBps);
  const txRate = num(remote.netTxRateBps);
  const cpuPercent = num(remote.cpuUsagePercent);
  const cpuCount = num(remote.cpuCount);
  const cores = Array.isArray(remote.cpuCores)
    ? (remote.cpuCores as unknown[]).map((value) => num(value)).filter((value): value is number => value != null)
    : null;
  const memTotal = num(remote.memoryTotalBytes);
  const memAvailable = num(remote.memoryAvailableBytes);
  const memFree = num(remote.memoryFreeBytes);
  const memCached = num(remote.memoryCachedBytes);
  const memUsed = memTotal != null && memAvailable != null ? Math.max(0, memTotal - memAvailable) : null;
  const memPercent = memUsed != null && memTotal ? (memUsed / memTotal) * 100 : null;
  const loads = [num(remote.loadAverage), num(remote.loadAverage5), num(remote.loadAverage15)];
  const loadPercent = (load: number | null) =>
    load == null || !cpuCount ? null : Math.max(0, Math.min(100, (load / cpuCount) * 100));
  const disks = Array.isArray(remote.disks) ? (remote.disks as Record<string, unknown>[]) : [];
  const processes = Array.isArray(remote.topProcesses)
    ? (remote.topProcesses as Record<string, unknown>[]).slice(0, 5)
    : [];

  return (
    <div className="terminal-ssh-rich">
      <div className="terminal-stat-grid is-net" title="延迟为 SSH 往返耗时；网速按相邻两次采样求差">
        <div className="terminal-stat-cell">
          <span>
            <Network size={11} />
            延迟
          </span>
          <b>{latencyMs == null ? "-" : `${latencyMs} ms`}</b>
        </div>
        <div className="terminal-stat-cell">
          <span>
            <Activity size={11} />
            网速
          </span>
          <b title={`↓ ${rxRate == null ? "-" : formatRate(rxRate)}   ↑ ${txRate == null ? "-" : formatRate(txRate)}`}>
            ↓{rxRate == null ? "-" : compactRate(rxRate)} &nbsp;↑{txRate == null ? "-" : compactRate(txRate)}
          </b>
        </div>
      </div>

      <DrawerCard
        icon={<Cpu size={12} />}
        title="CPU"
        value={cpuPercent == null ? "-" : `${Math.round(cpuPercent)}%`}
        hint={cpuCount ? `${Math.round(cpuCount)} 线程` : undefined}
      >
        <Meter percent={cpuPercent ?? 0} tone="accent" />
        {/* 每核列表就在 CPU 卡内、总进度条下面（双列滚动收纳 64 核，纯展示无交互）。 */}
        {cores && cores.length > 0 && (
          <div className="terminal-core-list" aria-label="每个核心的使用率">
            {cores.map((load, index) => (
              <div className="terminal-core-row" key={index} title={`核心 ${index} · ${Math.round(load)}%`}>
                <span className="terminal-core-chip">{index}</span>
                <Meter percent={load} tone={load >= 90 ? "warning" : "accent"} />
                <span className="terminal-core-value">{Math.round(load)}%</span>
              </div>
            ))}
          </div>
        )}
      </DrawerCard>

      {(memTotal != null || memUsed != null) && (
        <DrawerCard
          icon={<MemoryStick size={12} />}
          title="内存"
          value={memUsed != null && memTotal != null ? `${formatBytes(memUsed)} / ${formatBytes(memTotal)}` : formatBytes(memTotal ?? 0)}
          hint={memPercent == null ? undefined : `${Math.round(memPercent)}%`}
        >
          <Meter percent={memPercent ?? 0} tone="warning" />
          <div className="terminal-stat-grid is-mem">
            <div className="terminal-stat-cell">
              <span>已用</span>
              <b>{memUsed == null ? "-" : formatBytes(memUsed)}</b>
            </div>
            <div className="terminal-stat-cell">
              <span>缓存</span>
              <b>{memCached == null ? "-" : formatBytes(memCached)}</b>
            </div>
            <div className="terminal-stat-cell">
              <span>空闲</span>
              <b>{memFree == null ? "-" : formatBytes(memFree)}</b>
            </div>
          </div>
        </DrawerCard>
      )}

      {loads.some((load) => load != null) && (
        <DrawerCard icon={<Activity size={12} />} title="负载">
          <div className="terminal-load-row">
            <LoadRing label="1m" value={loads[0]} percent={loadPercent(loads[0])} />
            <LoadRing label="5m" value={loads[1]} percent={loadPercent(loads[1])} />
            <LoadRing label="15m" value={loads[2]} percent={loadPercent(loads[2])} />
          </div>
        </DrawerCard>
      )}

      {disks.length > 0 && (
        <DrawerCard icon={<Gauge size={12} />} title="磁盘">
          {disks.map((disk) => {
            const mount = typeof disk.mount === "string" ? disk.mount : "?";
            const total = num(disk.totalBytes);
            const used = num(disk.usedBytes);
            const percent = num(disk.usedPercent) ?? (total && used != null ? (used / total) * 100 : null);
            return (
              <div className="terminal-disk-row" key={mount}>
                <code className="terminal-disk-mount">{mount}</code>
                <Meter percent={percent ?? 0} tone={(percent ?? 0) >= 90 ? "warning" : "accent"} />
                <span className="terminal-disk-detail">
                  {used == null || total == null ? "-" : `${formatBytes(used)} / ${formatBytes(total)}`}
                </span>
              </div>
            );
          })}
        </DrawerCard>
      )}

      {processes.length > 0 && (
        <DrawerCard icon={<Activity size={12} />} title="CPU TOP">
          {processes.map((process, index) => {
            const name = typeof process.name === "string" ? process.name : "process";
            const pid = num(process.pid);
            const cpu = num(process.cpuPercent);
            return (
              <div className="terminal-proc-row" key={`${pid ?? index}-${name}`}>
                <span className="terminal-proc-rank" data-rank={index < 3 ? index + 1 : 0}>{index + 1}</span>
                <span className="terminal-proc-name" title={`${name} · PID ${pid ?? "-"}`}>
                  {name}
                  {pid != null ? <em>· {pid}</em> : null}
                </span>
                <code>{cpu == null ? "-" : `${Math.round(cpu * 10) / 10}%`}</code>
              </div>
            );
          })}
        </DrawerCard>
      )}
    </div>
  );
};

const HostPane: React.FC<{ activeTab: TerminalTab | null; connectionId?: string }> = ({
  activeTab,
  connectionId,
}) => {
  const kind = activeTab?.type ?? "local";
  const [status, setStatus] = useState<HostStatusSnapshot | null>(null);
  const [remote, setRemote] = useState<Record<string, unknown> | null>(null);
  const [loading, setLoading] = useState(true);
  /**
   * 网速需要相邻两次快照求差（需求 4）。
   * 上一份快照与它的采样时刻都放 ref：它们是计算中间量，不该触发重渲染。
   */
  const previousStatusRef = useRef<HostStatusSnapshot | null>(null);
  const previousSampledAtRef = useRef<number>(0);
  /**
   * 轻量档（limit=0）后端不返回进程/线程/端口明细，这里留一份上一次完整快照的列表，
   * 否则三个列表每次刷新都会被清空再填一次，肉眼可见地闪。
   */
  const detailRef = useRef<Pick<HostStatusSnapshot, "topProcesses" | "topThreads" | "listeningPorts"> | null>(null);
  /** 每 6 拍（约 30 秒）拉一次带明细的完整快照，其余拍只拉指标。 */
  const tickRef = useRef(0);
  const [rates, setRates] = useState<{ down: number; up: number } | null>(null);
  /**
   * SSH 状态读取失败的原因。以前 catch 里把它丢掉了，面板只剩「SSH 未连接」，
   * 用户完全看不出是没连接、连不上、还是参数不对。
   */
  const [remoteError, setRemoteError] = useState<string | null>(null);

  // SWR：切回面板先用上次数据立即渲染（后台照常每 3/8s 刷新覆盖）。
  const hostCacheKey = `host:${kind}:${connectionId ?? ""}:${activeTab?.root ?? ""}`;
  useEffect(() => {
    const cached = readDrawerCache<{ status: HostStatusSnapshot | null; remote: Record<string, unknown> | null }>(hostCacheKey);
    if (cached) {
      setStatus(cached.status);
      setRemote(cached.remote);
      setLoading(false);
    }
  }, [hostCacheKey]);

  const refresh = useCallback(() => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    // 窗口不可见（最小化）时不必采集：原生调用和随之而来的合成都省掉。
    if (document.hidden && previousStatusRef.current) return;
    if (kind === "ssh") {
      if (!connectionId) {
        setLoading(false);
        return;
      }
      // 参数名必须与 Rust 签名一致：`get_ssh_status(id: String, path: Option<String>)`。
      // 之前传的是 { connectionId, root }，后端缺必填的 id，反序列化直接失败，
      // 结果主机状态永远显示「SSH 未连接」。
      void invoke<Record<string, unknown>>("get_ssh_status", {
        id: connectionId,
        path: activeTab?.root || null,
      })
        .then((next) => {
          setRemote(next);
          setRemoteError(null);
          setLoading(false);
          writeDrawerCache(hostCacheKey, { status: null, remote: next });
        })
        .catch((reason: unknown) => {
          setRemote(null);
          setRemoteError(reason instanceof Error ? reason.message : String(reason));
          setLoading(false);
        });
      return;
    }
    // 明细（进程/线程/端口）要遍历全机线程，一次约 0.4 秒 CPU；指标只要几次系统调用。
    // 所以快档传 limit=0 只取指标，每 6 拍（约 30 秒）才拉一次明细——端口和进程排行
    // 本来就变得慢，没必要跟着 CPU 数字一起刷。
    const full = tickRef.current++ % 6 === 0;
    void readHostStatus(full ? 5 : 0).then((snapshot) => {
      // 两次快照求差算网速；间隔用真实时间差，不用固定刷新周期。
      const now = Date.now();
      if (snapshot) {
        const elapsed = previousSampledAtRef.current ? now - previousSampledAtRef.current : 0;
        setRates(networkRates(previousStatusRef.current, snapshot, elapsed));
        previousStatusRef.current = snapshot;
        previousSampledAtRef.current = now;
      }
      let merged = snapshot;
      if (snapshot) {
        if (full) {
          detailRef.current = {
            topProcesses: snapshot.topProcesses,
            topThreads: snapshot.topThreads,
            listeningPorts: snapshot.listeningPorts,
          };
        } else if (detailRef.current) {
          // 轻量档没有明细：沿用上一份，避免三个列表每次刷新都闪一下。
          merged = { ...snapshot, ...detailRef.current };
        }
      }
      setStatus(merged);
      setLoading(false);
      writeDrawerCache(hostCacheKey, { status: merged, remote: null });
    });
  }, [kind, connectionId, activeTab?.root, hostCacheKey]);

  // 指标每 5 秒刷新；SSH 远端命令较重，放慢到 8 秒。
  // 3 秒→5 秒是为了省电：每刷新一次要重绘整个抽屉卡片，实测每次约 0.14 秒 CPU，
  // 3 秒一刷就吃掉 ~2.4% 的一个核，5 秒一刷约 1.4%。面板是"看个大概"的读数，
  // 5 秒的粒度足够。
  useRefresh(refresh, [refresh], kind === "ssh" ? 8000 : 5000);

  if (loading) {
    return <DrawerLoading text="正在读取主机状态" />;
  }

  // 统一成 RemoteStatus 形状：SSH 用远端快照，本机用原生快照。
  const unified: Record<string, unknown> | null =
    kind === "ssh"
      ? remote
      : status
        ? {
              // systemStatusMetrics 用「总量 - 可用」算已用，所以必须把可用量一起给；
              // 只给 memoryUsedBytes 的话它读的是 memoryAvailableBytes，结果永远是 "- / 31.5 GB"。
              memoryTotalBytes: status.memoryTotalBytes,
              memoryAvailableBytes: Math.max(0, status.memoryTotalBytes - status.memoryUsedBytes),
              cpuCount: status.cpuCount,
              cpuPhysicalCores: status.cpuPhysicalCores ?? null,
              cpuUsagePercent: status.cpuPercent,
              diskTotalBytes: status.diskTotalBytes,
              diskUsedBytes: status.diskUsedBytes,
              diskAvailableBytes: status.diskAvailableBytes,
              diskUsedPercent: status.diskUsedPercent,
              // checkedPath 是面板上「统计的是哪个卷」的提示文案。
              checkedPath: status.diskPath,
            }
          : null;

  const metrics = systemStatusMetrics(unified);
  // CPU 拓扑优先显示「物理核 · 逻辑线程」，与本机任务管理器口径一致。
  const cores = status?.cpuPhysicalCores;
  const cpuTopology =
    kind === "local" && cores != null && cores > 0
      ? `${cores} 核 · ${status?.cpuCount ?? 0} 线程`
      : metrics.cpu.topology;

  return (
    <>


      {/* SSH 走富视图（每核/内存明细/网速/负载环/挂载点/进程），本机与 WSL 用通用卡。 */}
      {kind === "ssh" && remote ? (
        <SshRichStatus remote={remote} />
      ) : (
        <>
          <DrawerCard
            icon={<Cpu size={12} />}
            title="CPU"
            value={metrics.cpu.usage}
            hint={cpuTopology}
          >
            <Meter percent={metrics.cpu.percent ?? 0} tone="accent" />
          </DrawerCard>

          <DrawerCard
            icon={<MemoryStick size={12} />}
            title="内存"
            value={`${metrics.memory.used} / ${metrics.memory.total}`}
            hint={metrics.memory.usage}
          >
            <Meter percent={metrics.memory.percent ?? 0} tone="warning" />
          </DrawerCard>

          <DrawerCard
            icon={<Gauge size={12} />}
            title="磁盘"
            value={`${metrics.disk.used} / ${metrics.disk.total}`}
            hint={metrics.disk.path || metrics.disk.usage}
          >
            <Meter percent={metrics.disk.percent ?? 0} tone="accent" />
          </DrawerCard>
        </>
      )}

      {/* 网速：只有本机后端采集了累计收发字节，能算出真实速率；
          SSH / WSL 没有对应采集，整张卡不渲染（不写"暂不支持"）。 */}
      {kind === "local" && rates && (
        <DrawerCard
          icon={<Network size={12} />}
          title="网速"
          value={`↓ ${formatRate(rates.down)}  ↑ ${formatRate(rates.up)}`}
        />
      )}

      {/* 延迟与负载：SSH 由富视图承担（与网速同排），这里只服务非 SSH。 */}
      {kind !== "ssh" && metrics.latency != null && (
        <DrawerCard icon={<Network size={12} />} title="延迟" value={metrics.latency} />
      )}
      {kind !== "ssh" && metrics.load.value !== "-" && (
        <DrawerCard
          icon={<Activity size={12} />}
          title="负载"
          value={metrics.load.value}
          hint={metrics.load.description}
        />
      )}

      {kind === "local" && status && (
        <>
          <div className="terminal-drawer-list-card">
            <div className="terminal-drawer-list-head">
              <span>
                <Network size={12} />
                监听端口
              </span>
              <span className="terminal-drawer-count">{status.listeningPorts.length}</span>
            </div>
            {status.listeningPorts.length === 0 && (
              <div className="terminal-drawer-empty">没有监听中的 TCP 端口</div>
            )}
            {status.listeningPorts.map((entry) => (
              <div className="terminal-drawer-row" key={`${entry.port}-${entry.protocol}`}>
                <code>:{entry.port}</code>
                <span className="terminal-drawer-row-main">
                  {entry.processName}
                  {entry.pid ? ` · ${entry.pid}` : ""}
                </span>
              </div>
            ))}
          </div>

          <div className="terminal-drawer-list-card">
            <div className="terminal-drawer-list-head">
              <span>
                <Activity size={12} />
                内存占用最高的进程
              </span>
            </div>
            {status.topProcesses.map((entry) => (
              <div className="terminal-drawer-row" key={`${entry.name}-${entry.pid}`}>
                <span className="terminal-drawer-row-main">{entry.name}</span>
                <code>{formatBytes(entry.memoryBytes)}</code>
              </div>
            ))}
          </div>

          {/* 线程占用明细（需求 4）：按累计 CPU 时间排序，看清"哪些线程在吃 CPU"。
              非 Windows 或采集失败时为空，整块不渲染。 */}
          {status.topThreads && status.topThreads.length > 0 && (
            <div className="terminal-drawer-list-card">
              <div className="terminal-drawer-list-head">
                <span>
                  <Cpu size={12} />
                  CPU 占用最高的线程
                </span>
                <span className="terminal-drawer-count">{status.topThreads.length}</span>
              </div>
              {status.topThreads.map((thread) => (
                <div className="terminal-drawer-row" key={`${thread.pid}-${thread.threadId}`}>
                  <span className="terminal-drawer-row-main" title={`${thread.processName} · ${thread.pid}`}>
                    {thread.processName || `PID ${thread.pid}`}
                    <em className="terminal-drawer-thread-id">T{thread.threadId}</em>
                  </span>
                  <code>{formatCpuTime(thread.cpuTimeMs)}</code>
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {unified == null && (
        <div className="terminal-drawer-empty">
          {kind === "ssh"
            ? connectionId
              ? remoteError
                ? `读取 SSH 主机状态失败：${remoteError}`
                : "SSH 未连接"
              : "当前会话没有绑定 SSH 连接，无法读取主机状态。"
            : "仅在桌面应用内可读取"}
        </div>
      )}
    </>
  );
};

const GitPane: React.FC<{ activeTab: TerminalTab | null; fallbackRoot: string; connectionId?: string; followTerminal?: boolean }> = ({ activeTab, fallbackRoot, connectionId, followTerminal = true }) => {
  /**
   * 版本控制跟随激活终端（需求 7 / v3 §16）：
   * - local / wsl：读该会话的工作目录（wsl 的 root 是远端路径，本地 git 读不到，退到本机工作区）
   * - ssh：走 `ssh_workspace_git` 在远端读同一份 git 数据（形状与本地完全一致）
   *
   * SSH 分支的 root 用 activeTab.root（终端页签的起始目录）：用户 cd 进仓库后
   * 由 OSC 7 / pwd 轮询把 root 更新过来，这里随之重取，不需要额外状态。
   */
  const kind = activeTab?.type ?? "local";
  const isSsh = kind === "ssh";
  const sshId = isSsh ? connectionId?.trim() || undefined : undefined;
  // 后端对空 root 会按远端家目录处理，所以 SSH 下 root 为空也照常发请求。
  // SSH 页签的 root 在早期版本是空串，此时拉到 "~" 而不是留空：空 root 会让
  // 界面把路径说成「远端家目录」，而真实路径要等 cwd 跟随回来后才有。
  // 跟随关闭时锚定在终端打开时的工作目录（§22）：不跟着 cd 走。
  const effectiveRoot = followTerminal ? activeTab?.root : (activeTab?.rootAtOpen ?? activeTab?.root);
  const root = effectiveRoot || (isSsh ? "~" : fallbackRoot);
  const [summary, setSummary] = useState<WorkspaceGitSummary | null>(null);
  const [changes, setChanges] = useState<WorkspaceGitChange[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // SWR：切回先显上次数据（见 drawerDataCache）。
  const cacheKey = `git:${kind}:${sshId ?? ""}:${root}`;

  useEffect(() => {
    const cached = readDrawerCache<{ summary: WorkspaceGitSummary | null; changes: WorkspaceGitChange[] }>(cacheKey);
    if (cached) {
      setSummary(cached.summary);
      setChanges(cached.changes);
      setLoading(false);
    }
  }, [cacheKey]);

  const refresh = useCallback(() => {
    // SSH 拿不到连接 id 时直接说明原因：后端会报「缺少连接 ID」，回显给用户没有意义。
    if (isSsh && !sshId) {
      setError("当前会话没有可用的 SSH 连接 ID，无法读取远端 Git 状态。");
      setLoading(false);
      return;
    }
    const load = isSsh
      ? readSshWorkspaceGit(sshId!, root)
      : Promise.all([readWorkspaceGit(root), readWorkspaceGitChanges(root)]).then(
          ([nextSummary, nextChanges]) => ({ summary: nextSummary, changes: nextChanges }),
        );
    void load
      .then(({ summary: nextSummary, changes: nextChanges }) => {
        setSummary(nextSummary);
        setChanges(nextChanges);
        setError(null);
        writeDrawerCache(cacheKey, { summary: nextSummary, changes: nextChanges });
      })
      .catch((reason: unknown) => {
        // SSH 读取失败要如实显示：把失败伪装成「不是 Git 仓库」会让用户以为是仓库问题。
        setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => setLoading(false));
  }, [cacheKey, isSsh, root, sshId]);

  useRefresh(refresh, [refresh]);

  if (error) {
    return <div className="terminal-drawer-empty">读取 Git 状态失败：{error}</div>;
  }
  // SSH 没解析到连接 id 时，后面所有分支都读不出数据（会一直停在 loading），
  // 直接说清原因。
  if (isSsh && !sshId) {
    return (
      <div className="terminal-drawer-empty">
        当前 SSH 会话没有绑定连接，无法读取远端 Git 状态。
        请从服务器列表重新打开该会话。
      </div>
    );
  }
  if (!root) {
    return <div className="terminal-drawer-empty">还没有解析到本地工作目录。</div>;
  }
  if (loading) {
    return <DrawerLoading text="正在读取仓库状态" />;
  }
  if (!summary?.repository) {
    // SSH 下 root 可能为空串（尚未解析到远端目录），此时把家目录说清楚，别显示成空路径。
    return <div className="terminal-drawer-empty">{root || (isSsh ? "远端家目录" : "当前目录")} 不是 Git 仓库。</div>;
  }

  return (
    <>
      <DrawerCard
        icon={<GitBranch size={12} />}
        title="当前分支"
        value={summary.branch || "(detached)"}
        hint={summary.dirty ? "有未提交改动" : "工作区干净"}
      >
        <div className="terminal-drawer-diff">
          <span className="is-added">+{summary.addedLines}</span>
          <span className="is-removed">-{summary.removedLines}</span>
          <span>{summary.changedFiles} 个文件变更</span>
        </div>
      </DrawerCard>

      {/* v3 R6：变更文件列表固定高度、内部滚动，不把右栏撑长。 */}
      <div className="terminal-drawer-list-card terminal-drawer-changes">
        <div className="terminal-drawer-list-head">
          <span>变更文件</span>
          <Button type="button" variant="bare" className="terminal-sidebar-mini-btn" aria-label="刷新" onClick={refresh}>
            <RefreshCw size={11} />
          </Button>
        </div>
        {changes.length === 0 && <div className="terminal-drawer-empty">没有未提交的改动</div>}
        {changes.map((change) => (
          <div className="terminal-drawer-change" key={change.path} title={change.path}>
            <span className={"terminal-drawer-status is-" + change.status}>{change.status[0]}</span>
            {/* 优先展示文件末尾（需求 7）：文件名在前会挤掉真正关心的部分，
                这里改成 basename 为主、目录为辅且从尾部截断，一眼看到最后几级路径。 */}
            <span className="terminal-drawer-change-name">{basename(change.path)}</span>
            <span className="terminal-drawer-change-dir">{dirnameTail(change.path)}</span>
            {(change.addedLines > 0 || change.removedLines > 0) && (
              <span className="terminal-drawer-change-diff">
                <em className="is-added">+{change.addedLines}</em>
                <em className="is-removed">-{change.removedLines}</em>
              </span>
            )}
          </div>
        ))}
      </div>
    </>
  );
};

/**
 * 每页签独立记住的浏览位置（5.5）：切回该页签直接落到上次的目录/文件。
 * 用模块级 Map 而不是组件内 ref：FilePane 会随右栏页签切换卸载重建，
 * 组件内的 ref 撑不过一次卸载。条目数上界 = 打开过的终端数，可忽略。
 * 这里只记路径与文件名，不缓存正文——正文可能到 8 MB，按页签缓存会把内存吃满。
 */
/**
 * 右栏「跟随终端目录」设置（§22 三轮）：会话级、跨抽屉收起保留。默认跟随；
 * 关闭时文件/Git 面板锚定在终端打开时的工作目录（tab.rootAtOpen），不跟着 cd 走。
 */
const DRAWER_FOLLOW = { files: true, git: true };

/**
 * 每页签记住上次的浏览目录与预览文件。`root` 是记忆生成时的会话根目录（cd 跟随
 * 的锚点）：恢复时只有 root 与当前一致才生效——用户在终端里 cd 走了，面板就该
 * 整体跟到新目录，而不是把旧目录/旧预览硬塞回来（v3 §15 用户反馈修复）。
 */
const FILES_MEMORY = new Map<
  string,
  { root: string; path: string; file: { path: string; name: string; size?: number } | null }
>();

const FilesPane: React.FC<{
  activeTab: TerminalTab | null;
  fallbackRoot: string;
  connectionId?: string;
}> = ({ activeTab, fallbackRoot, connectionId }) => {
  // 跟随开关在面板自己的 ⋮ 菜单里（v3 §22 六轮）：设置会话级保存。
  const [followFiles, setFollowFiles] = useState(() => DRAWER_FOLLOW.files);
  const kind = activeTab?.type === "ssh" ? "ssh" : "local";
  /**
   * 起始目录：SSH/本地用会话 root 或工作目录兜底。
   */
  const effectiveRoot = followFiles ? activeTab?.root : (activeTab?.rootAtOpen ?? activeTab?.root);
  // SSH 空 root 回退 ~（与 GitPane 同语义）：不回退的话请求打到后端只会得到
  // 「请输入 SSH 绝对目录或 ~/ 路径」，这条报错没有可操作性（v3 §22 十七轮）。
  const rootPath =
    effectiveRoot || (kind === "ssh" ? "~" : fallbackRoot);
  const tabKey = activeTab?.id ?? "none";
  const source: PreviewSource = { kind, root: rootPath };
  const [path, setPath] = useState(
    () => (FILES_MEMORY.get(tabKey)?.root === rootPath ? FILES_MEMORY.get(tabKey)!.path : "") || rootPath,
  );
  const [entries, setEntries] = useState<DirectoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** 目录**加载**错误：与操作错误分开——加载错误只在面板真没内容可看时显示，
   * 有文件树/列表就不叠「无法访问路径」这类文案（用户定稿：真访问不了就不该有树）。 */
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadTick, setReloadTick] = useState(0);
  // 工具栏（交互复制自代码查看器文件页）：搜索过滤 + 显示隐藏文件 + 新建。
  const [filter, setFilter] = useState("");
  const [showHidden, setShowHidden] = useState(false);
  // 预览状态：blocked 非空表示「前置校验没通过」，此时不发请求，只把原因显示出来。
  const [preview, setPreview] = useState<{
    path: string;
    name: string;
    size?: number;
    blocked?: string;
  } | null>(() => {
    const entry = FILES_MEMORY.get(tabKey);
    const memo = entry?.root === rootPath ? entry?.file : null;
    return memo ? { ...memo } : null;
  });
  // 头部弹层（v3 §22 十一轮）：下载历史 / 查看器设置，1:1 复用代码查看器组件。
  // 上传记录与下载历史是同一个弹层的两面，所以状态从 `"download" | null` 扩成
  // 三态——原来写死 `"download"` 时上传入口根本无处可挂。
  //
  // 两个入口各用一份状态：文件树控制行里的上传按钮（`browserUploadOpen`）和
  // 预览窗口工具栏里的上传按钮（`transferPopup === "upload"`）。合成一个的话，
  // 预览窗口开着时点控制行会把两个弹层同时弹出来。
  const [transferPopup, setTransferPopup] = useState<"upload" | "download" | null>(null);
  const [browserUploadOpen, setBrowserUploadOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const transferAnchor = useRef<HTMLSpanElement>(null);
  const uploadAnchor = useRef<HTMLSpanElement>(null);
  const settingsAnchor = useRef<HTMLSpanElement>(null);
  /**
   * 下载当前激活文件。
   *
   * 原来这里是自己写的一套简化通路：写死 `downloadAccess: { elevated: false }`、
   * 用 `window.confirm` 问大文件、sudo 不可读时直接失败。用户要求右栏和文件
   * 预览页一致，所以改成复用 `useTransferActions().download` —— 同一套 sudo 授权
   * 弹窗、同一套大文件二次确认、同一份记录，失败也走同一套文案。
   */
  const downloadActive = async () => {
    const activePath = editorMode ? browser.activeFile : preview?.path;
    if (!activePath) return;
    await transfers.download(activePath);
  };
  const [previewText, setPreviewText] = useState("");
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  // 请求序号防竞态：快速连点目录时旧响应可能后到，序号不匹配就丢弃（5.5）。
  const dirSeqRef = useRef(0);
  const fileSeqRef = useRef(0);

  // ------------------------------------------------------------------
  // 文件树（v3 §22 四轮用户需求）：ssh/local 的面板主体换成代码查看器左侧
  // 同款 CodeFileTree（1:1：同一组件、同一套树行/悬浮元数据/拖拽样式），
  // 数据源用同一个 useCodeBrowser hook。
  // ------------------------------------------------------------------
  const treeTarget: BrowserTarget | null =
    kind === "ssh"
      ? connectionId?.trim()
        ? { kind: "ssh", connectionId: connectionId.trim(), name: activeTab?.title || connectionId.trim() }
        : null
      : kind === "local"
        ? { kind: "local" }
        : null;
  const browser = useCodeBrowser();
  const treeRoot = browser.root || rootPath;
  const [dirMenu, setDirMenu] = useState<{ entry: BrowserEntry; x: number; y: number; trigger: HTMLElement } | null>(null);
  // 根目录变化（cd 跟随 / 锚定切换 / 换页签换机器）就整棵树重载。
  useEffect(() => {
    if (!treeTarget || !rootPath) return;
    void browser.loadFolder(rootPath, true, treeTarget);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, connectionId, rootPath]);
  /** 树内 mutation（重命名/删除/移动/新建）成功后刷新整棵树。 */
  const runTreeMutation = async (request: ReturnType<typeof buildMutationRequest>) => {
    if (!treeTarget) return;
    const command = kind === "ssh" ? "execute_ssh_code_viewer_mutation" : "execute_code_viewer_mutation";
    try {
      await invoke(command, kind === "ssh" ? { request, connectionId } : { request });
      void browser.loadFolder(treeRoot, true, treeTarget);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const reloadTree = () => {
    if (treeTarget) void browser.loadFolder(treeRoot, true, treeTarget);
  };
  /** 根目录显示名（与代码查看器 folderName 同语义）。 */
  const folderName = (pathValue: string) => pathValue.split("/").filter(Boolean).pop() || pathValue || "/";

  // ---- 上传 / 下载（用户反馈：右栏「没法下载上传文件」）-------------------
  // 复用代码查看器那套 useTransferActions：同一批后端命令
  // （inspect_code_download / start_code_download / inspect_code_upload /
  // start_code_upload），同一套 sudo 授权与大文件二次确认，同一份传输记录。
  // 原来这两个入口在这里被写死成 `() => undefined` + `uploadDisabled`，所以
  // 面板里点上传完全没反应、非 SSH 会话也没有下载按钮。
  // 上传目标是树根（与文件预览页一致）：右栏是"浏览当前目录"，选中项不改变
  // 落点，避免用户在深层目录里误传。
  // 这两个值都进 useTransferActions 的依赖数组，必须稳定：每次渲染新建对象会
  // 让 download/upload 回调跟着换引用，正在进行的传输状态被判成"目标变了"。
  const transferTarget = useMemo<BrowserTarget>(() => treeTarget ?? { kind: "local" }, [treeTarget]);
  const openTransferPopup = useCallback((direction: "upload" | "download") => setTransferPopup(direction), []);
  const transfers = useTransferActions(transferTarget, Boolean(treeTarget), openTransferPopup);
  // 文件操作走与文件预览页**完全相同的确认流程**（useFileMutations）：重命名/新建
  // 先预检再确认，永久删除带「递归删除」和「不可恢复」两个勾选。以前这里用的是
  // `window.prompt` / `window.confirm`，既没法预检重名、也没法确认递归删除。
  const browserMutations = useFileMutations({
    target: transferTarget,
    root: treeRoot,
    connected: Boolean(treeTarget),
    loading: browser.loading,
    dirty: browser.dirty,
    affectsDraft: (target) => pathInMutation(browser.activeFile, target, kind === "local"),
    onComplete: async (entry, result, action) => {
      if (!treeTarget) return;
      if (action.startsWith("create-")) await browser.loadFolder(entry.path, false, treeTarget);
      else await browser.applyMutation(entry, result);
      void browser.loadFolder(treeRoot, false, treeTarget);
    },
  });
  /** 与文件预览页一致：把这个目录放到新标签（工具窗口模式下是新窗口）里打开。 */
  const openFolderInNewTab = (folder: string) => {
    const snapshot: BrowserSnapshot = { target: treeTarget ?? { kind: "local" }, root: folder, showHiddenFiles: showHidden };
    if (isToolWindow()) {
      void openToolWindowWithState("code-viewer", "代码查看", { "code-viewer": snapshot })
        .catch((error) => notify(formatInvokeError(error), "error"));
    } else {
      window.dispatchEvent(new CustomEvent(CODE_VIEWER_OPEN_TAB_EVENT, { detail: snapshot }));
    }
  };
  const transferPanel = {
    preparing: transfers.preparing,
    preparationError: transfers.error?.direction === "upload" ? transfers.error.message : undefined,
    onUploadFiles: () => void transfers.pickUpload(treeRoot, false),
    onUploadFolder: () => void transfers.pickUpload(treeRoot, true),
    uploadDisabled: !treeTarget || transfers.busy || !treeRoot,
  };

  // ---- 性能（v3 §22 十三轮）：编辑器每敲一个字都会改 browser 草稿并重渲染
  // 整个 FilesPane；树几百行 DOM 跟着重渲染是打字卡顿的主源。这里把树的
  // 处理回调全部 useCallback 稳定，再把树整体 useMemo——草稿变化时树元素
  // 引用不变，React 直接跳过子树。
  const handleTreeLoadMetadata = useCallback(
    (entry: BrowserEntry): Promise<FileMetadata> =>
      kind === "ssh"
        ? invoke<FileMetadata>("ssh_code_viewer_file_metadata", { connectionId, root: treeRoot, path: entry.path })
        : invoke<FileMetadata>("code_viewer_file_metadata", { path: entry.path }),
    [kind, connectionId, treeRoot],
  );
  const handleTreeToggle = useCallback((entry: BrowserEntry) => void browser.toggleFolder(entry), [browser]);
  const handleTreeOpen = useCallback(
    (entry: BrowserEntry) => {
      if (entry.isDirectory) {
        void browser.toggleFolder(entry);
      } else {
        openPreview({
          path: entry.path,
          name: entry.name,
          isDirectory: false,
          size: typeof entry.size === "number" ? entry.size : undefined,
        });
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [browser, source],
  );
  const handleTreeDirectoryMenu = useCallback((entry: BrowserEntry, event: React.MouseEvent<HTMLButtonElement>) => {
    setDirMenu({ entry, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
  }, []);
  // 编辑模式（v3 §22 十二轮）：ssh/local 的文件编辑直接用 useCodeBrowser 的
  // 完整机制（多标签/草稿/脏标/保存/自动刷新/冲突检测），其余情况保留旧只读
  // 预览通路。
  const editorMode = Boolean(treeTarget);
  const closeAllEditorTabs = () => {
    for (const path of browser.openFiles) browser.closeFile(path);
  };


  const rememberPath = (nextPath: string) => {
    const current = FILES_MEMORY.get(tabKey);
    FILES_MEMORY.set(tabKey, {
      root: rootPath,
      path: nextPath,
      file: current?.root === rootPath ? (current?.file ?? null) : null,
    });
  };

  const rememberFile = (file: { path: string; name: string; size?: number } | null) => {
    const current = FILES_MEMORY.get(tabKey);
    FILES_MEMORY.set(tabKey, {
      root: rootPath,
      path: current?.root === rootPath ? (current?.path ?? path) : path,
      file,
    });
  };

  // 切页签：同 root 才恢复记忆位置（避免每次重拉根目录）；root 变了（终端里
  // cd 过，或切到另一台机器的页签）说明旧记忆已失效，直接跟到新根目录并清预览。
  useEffect(() => {
    const memo = FILES_MEMORY.get(tabKey);
    if (editorMode) {
      closeAllEditorTabs();
      if (memo && memo.root === rootPath && memo.file) {
        void browser.openFile({ name: memo.file.name, path: memo.file.path, isDirectory: false });
      }
      return;
    }
    if (memo && memo.root === rootPath) {
      setPath(memo.path || rootPath);
      setPreview(memo.file ? { ...memo.file } : null);
    } else {
      setPath(rootPath);
      setPreview(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabKey, rootPath]);

  // 目录加载：path / 通路 / 连接任一变化就重拉，序号不匹配的结果直接丢弃。
  useEffect(() => {
    if (!path) {
      setLoading(false);
      return;
    }
    const seq = ++dirSeqRef.current;
    const dirCacheKey = `files:${kind}:${connectionId ?? ""}:${path}`;
    const cached = readDrawerCache<DirectoryEntry[]>(dirCacheKey);
    if (cached) {
      // SWR：先显示上次目录，后台拉新（拉到再覆盖）；此时不显示加载态。
      setEntries(cached);
      setLoading(false);
    } else {
      setLoading(true);
    }
    setError(null);
    setLoadError(null);
    // SSH 且没解析到 connectionId 时**不发请求**：传空串只会换来后端一句
    // 「SSH 执行目标缺少连接 ID」，对用户没有任何信息量。这里直接把原因说清楚。
    if (kind === "ssh" && !connectionId) {
      setEntries([]);
      setLoadError("当前会话没有绑定 SSH 连接，无法浏览远端目录。");
      setLoading(false);
      return;
    }
    const task =
      kind === "ssh"
        ? listSshDirectory(connectionId!, path)
        : listLocalDirectory(path);
    void task
      .then((list) => {
        if (seq !== dirSeqRef.current) return;
        setEntries(list);
        setLoading(false);
        writeDrawerCache(dirCacheKey, list);
      })
      .catch((reason: unknown) => {
        if (seq !== dirSeqRef.current) return;
        // 有缓存数据时保留旧列表（只是这次刷新失败），避免一进面板白屏。
        if (!cached) setEntries([]);
        // 加载错误走 loadError（渲染侧有内容就不显示），不与文件树同屏。
        setLoadError(reason instanceof Error ? reason.message : String(reason));
        setLoading(false);
      });
  }, [path, kind, connectionId, reloadTick]);

  const refresh = useCallback(() => setReloadTick((tick) => tick + 1), []);

  useRefresh(refresh, [refresh]);

  /**
   * 打开文件预览：先做 5.3 的廉价判定（只看 name/size，不发请求），
   * 判定不通过就把原因直接显示在预览面板里，一次多余的读取都不白跑。
   */
  const openPreview = (entry: DirectoryEntry) => {
    const verdict = inspectPreviewEntry(source, entry);
    const size = entry.size ?? undefined;
    rememberFile({ path: entry.path, name: entry.name, size });
    if (editorMode) {
      if (!verdict.ok) {
        notify(verdict.message, "warning");
        return;
      }
      // 编辑模式：交给 browser 钩子的完整文件机制（读取/草稿/脏标/保存）。
      void browser.openFile({ name: entry.name, path: entry.path, isDirectory: false });
      return;
    }
    const next = {
      path: entry.path,
      name: entry.name,
      size,
      blocked: verdict.ok ? undefined : verdict.message,
    };
    setPreview(next);
    setPreviewText("");
    setPreviewError(null);
  };

  // 真正读取正文：SSH 走远端读取命令，本地走代码查看器。
  useEffect(() => {
    if (editorMode) return; // 编辑模式由 browser 钩子负责读取
    if (!preview) {
      setPreviewText("");
      setPreviewError(null);
      setPreviewLoading(false);
      return;
    }
    if (preview.blocked) {
      setPreviewText("");
      setPreviewError(preview.blocked);
      setPreviewLoading(false);
      return;
    }
    const seq = ++fileSeqRef.current;
    setPreviewLoading(true);
    setPreviewError(null);
    // 与目录加载同一条纪律：SSH 没拿到 connectionId 就不发请求，直接给原因。
    if (kind === "ssh" && !connectionId) {
      setPreviewText("");
      setPreviewError("当前会话没有绑定 SSH 连接，无法读取远端文件。");
      setPreviewLoading(false);
      return;
    }
    const task =
      kind === "local"
        ? invoke<string>("read_code_viewer_file", { path: preview.path })
        : readRemoteTextFile({
            kind: "ssh",
            connectionId,
            // root 用树根而不是实时 cwd：cd 跟随/锚定切换后两者可能不一致，
            // 后端按 root 校验文件在目录内，错位即读取失败（十轮修复）。
            root: treeTarget ? treeRoot : activeTab?.root,
            path: preview.path,
          });
    void task
      .then((text) => {
        if (seq !== fileSeqRef.current) return;
        setPreviewText(typeof text === "string" ? text : "");
        setPreviewLoading(false);
      })
      .catch((reason: unknown) => {
        if (seq !== fileSeqRef.current) return;
        setPreviewError(reason instanceof Error ? reason.message : String(reason));
        setPreviewLoading(false);
      });
  }, [preview, kind, connectionId, activeTab?.root]);

  // 超过 256 KB 关闭高亮（hljs 在大文件上是 O(n) 且生成大量 span），面板会提示。
  const highlightOff = previewText.length > MAX_HIGHLIGHT_BYTES;
  const previewSize = preview?.size ?? previewText.length;
  // .md 走与代码查看器文件页完全相同的 MarkdownFilePreview（默认预览态）。
  const markdownPreview = Boolean(preview && isMarkdownFile(preview.path));
  const [outlineOpen, setOutlineOpen] = useState(false);
  const previewOutlineId = "terminal-file-preview-outline";

  if (!path) {
    return <div className="terminal-drawer-empty">当前终端没有可浏览的目录。</div>;
  }

  // 预览是**浮在整个工作区之上的可拖拽窗口**（设计要求 5.3 + 用户反馈）：
  // 既能继续看到并浏览目录，又能拖到任意位置、从八条边缩放。窗口本身 portal 到
  // body，所以这里的 DOM 层级只是"声明位置"，不影响它的定位基准。
  // 浮窗可见性：编辑模式看钩子的标签列表，旧预览通路看预览状态。
  const windowOpen = editorMode ? browser.openFiles.length > 0 : preview !== null;
  const activeName = editorMode ? fileName(browser.activeFile) : preview?.name ?? "";
  const editorFile = editorMode ? browser.files[browser.activeFile] : undefined;
  const editorDirty = Boolean(
    editorFile && editorFile.content !== editorFile.draft && !editorFile.readOnlyFile,
  );
  const editorMarkdown = editorMode && isMarkdownFile(browser.activeFile) && !browser.readOnlyFile;

  // 树整体 memo：仅目录数据/过滤/高亮变化才重建元素（草稿变化被排除在外）。
  const treeElement = useMemo(() => {
    if (!treeTarget) return null;
    return (
      <div
        className="archive-tree-scroll-list custom-scrollbar"
        onContextMenu={(event) => {
          if ((event.target as HTMLElement).closest(".code-viewer-tree-item")) return;
          event.preventDefault();
          setDirMenu({ entry: { name: folderName(treeRoot), path: treeRoot, isDirectory: true }, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
        }}
      >
        <CodeFileTree
          root={treeRoot}
          tree={browser.tree}
          openFolders={browser.openFolders}
          loadingFolders={browser.loadingFolders}
          activeFile={browser.activeFile}
          query={filter}
          connected
          metadataScope={`${kind}:${connectionId ?? "local"}`}
          loadMetadata={handleTreeLoadMetadata}
          showHiddenFiles={kind !== "ssh" || showHidden}
          fileStatus={() => undefined}
          directoryChanged={() => false}
          onOpen={handleTreeOpen}
          onToggle={handleTreeToggle}
          onDirectoryMenu={handleTreeDirectoryMenu}
          onMove={(entry, destinationDirectory) => {
            void runTreeMutation(
              buildMutationRequest({
                root: treeRoot,
                path: entry.path,
                action: "move",
                name: "",
                destinationDirectory,
                recursive: false,
                overwrite: false,
                acknowledged: true,
              }),
            );
          }}
        />
        {browser.loadingFolders[treeRoot] && <DrawerLoading text="正在读取目录" />}
        {error && <div className="terminal-drawer-empty">{error}</div>}
        {/* 目录加载失败只有**真的没内容可看**时才提示：树还在就说明目录实际
            访问得到（或至少有已加载的内容），「无法访问路径」和文件树同屏
            自相矛盾（用户定稿：真访问不了就不该有树）。操作错误（error）
            不受此限制，树在也要提示。 */}
        {loadError && !error && !(browser.tree[treeRoot] || []).length && !entries.length && (
          <div className="terminal-drawer-empty">{loadError}</div>
        )}
        {!browser.loadingFolders[treeRoot] && !error && !loadError && !(browser.tree[treeRoot] || []).length && (
          <p className="code-viewer-tree-empty">此目录为空</p>
        )}
      </div>
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [treeTarget, treeRoot, browser.tree, browser.openFolders, browser.loadingFolders, browser.activeFile, filter, showHidden, handleTreeLoadMetadata, handleTreeOpen, handleTreeToggle, handleTreeDirectoryMenu, error, loadError, entries.length]);

  const closeFloatingWindow = () => {
    rememberFile(null);
    if (editorMode) closeAllEditorTabs();
    setPreview(null);
  };

  const previewCard = windowOpen ? (
    <FloatingWindow label={`预览 ${activeName}`} onClose={closeFloatingWindow}>
      {/* 文件页 1:1（v3 §22 十二轮）：编辑模式直接绑 browser 钩子的草稿/保存，
          其余情况保持只读旧通路。 */}
      <article className="archive-main-article code-viewer-main code-viewer-pane-card">
        <div className="archive-preview-titlebar code-viewer-tab-bar">
          <FileTabStrip
            tabs={(editorMode ? browser.openFiles : [preview!.path]).map((path) => {
              const file = editorMode ? browser.files[path] : undefined;
              return {
                path,
                title: fileName(path),
                dirty: Boolean(file && file.content !== file.draft),
                readOnly: file?.readOnlyFile,
              };
            })}
            activePath={editorMode ? browser.activeFile : preview!.path}
            onSelect={(nextPath) => {
              if (!editorMode) return;
              browser.setActiveFile(nextPath);
              rememberFile({ path: nextPath, name: fileName(nextPath) });
            }}
            onSelectFromOverflow={(nextPath) => {
              if (!editorMode) return;
              browser.promoteFile(nextPath);
              rememberFile({ path: nextPath, name: fileName(nextPath) });
            }}
            onClose={(closing) => {
              if (editorMode) {
                browser.closeFile(closing);
                if (browser.openFiles.length <= 1 && browser.openFiles[0] === closing) {
                  rememberFile(null);
                }
              } else {
                rememberFile(null);
                setPreview(null);
              }
            }}
            // 右键批量关闭：路径全部关掉时把记忆的清单位置一起清掉，动作与单个关闭一致。
            // 旧只读预览只有一个标签，也允许「关闭全部」把它关掉。
            onCloseMany={(closing) => {
              if (!editorMode) {
                if (preview && closing.includes(preview.path)) {
                  rememberFile(null);
                  setPreview(null);
                }
                return;
              }
              browser.closeFiles(closing);
              if (browser.openFiles.every((path) => closing.includes(path))) rememberFile(null);
            }}
            label="预览文件"
          />
          <div className="archive-preview-actions">
            {editorMarkdown ? (
              <>
                <Button
                  className={`code-viewer-pane-toggle${browser.previewing ? " is-active" : ""}`}
                  variant="ghost"
                  icon={<ListTree size={14} strokeWidth={1.6} />}
                  aria-label={outlineOpen ? "隐藏文档目录" : "显示文档目录"}
                  title={outlineOpen ? "隐藏文档目录" : "显示文档目录"}
                  aria-expanded={outlineOpen}
                  aria-controls={previewOutlineId}
                  onClick={() => setOutlineOpen((open) => !open)}
                />
                <Button
                  className={`code-viewer-pane-toggle${browser.previewing ? " is-active" : ""}`}
                  variant="ghost"
                  aria-label={browser.previewing ? "编辑 Markdown" : "预览 Markdown"}
                  title={browser.previewing ? "编辑 Markdown" : "预览 Markdown"}
                  aria-pressed={browser.previewing}
                  onClick={() => browser.setPreviewing(!browser.previewing)}
                >
                  {browser.previewing ? "编辑" : "预览"}
                </Button>
              </>
            ) : undefined}
            {!editorMode && markdownPreview && !previewLoading && !previewError ? (
              <Button
                className={`code-viewer-pane-toggle${outlineOpen ? " is-active" : ""}`}
                variant="ghost"
                icon={<ListTree size={14} strokeWidth={1.6} />}
                aria-label={outlineOpen ? "隐藏文档目录" : "显示文档目录"}
                title={outlineOpen ? "隐藏文档目录" : "显示文档目录"}
                aria-expanded={outlineOpen}
                aria-controls={previewOutlineId}
                onClick={() => setOutlineOpen((open) => !open)}
              />
            ) : undefined}
            {editorMode && browser.activeFile && !browser.readOnlyFile && !browser.fileLoading && (
              <span
                className={`code-file-state is-${browser.externalChanged || browser.saveState === "error" ? "error" : browser.saveState === "saving" ? "saving" : "ok"}`}
                role="status"
                title={browser.externalChanged ? "文件冲突" : undefined}
              >
                {browser.externalChanged
                  ? "文件冲突"
                  : browser.saveState === "saving"
                    ? "保存中…"
                    : browser.saveState === "error"
                      ? "保存失败"
                      : browser.saveState === "saved"
                        ? "已保存"
                        : editorDirty
                          ? "未保存"
                          : ""}
              </span>
            )}
            {editorMode && (
              <Button
                variant="default"
                icon={<Save size={12} />}
                disabled={!editorDirty || browser.loading || browser.externalChanged}
                style={{ visibility: editorDirty ? "visible" : "hidden" }}
                onClick={() => void browser.saveFile()}
              >
                保存
              </Button>
            )}
            {kind === "ssh" && connectionId && (
              <span ref={uploadAnchor} className="code-transfer-anchor">
                <Button
                  className="code-viewer-pane-toggle"
                  variant="ghost"
                  icon={<Upload size={14} strokeWidth={1.6} />}
                  aria-label="上传记录"
                  title="上传记录"
                  aria-expanded={transferPopup === "upload"}
                  aria-haspopup="dialog"
                  onClick={() => setTransferPopup((current) => (current === "upload" ? null : "upload"))}
                />
              </span>
            )}
            {kind === "ssh" && connectionId && (
              <Button
                className="code-viewer-pane-toggle"
                variant="ghost"
                icon={<Download size={14} strokeWidth={1.6} />}
                aria-label="下载当前文件"
                title="下载当前文件"
                disabled={transfers.busy || !activeName}
                onClick={() => void downloadActive()}
              />
            )}
            <span ref={transferAnchor} className="code-transfer-anchor">
              <Button
                className="code-viewer-pane-toggle"
                variant="ghost"
                icon={<History size={14} strokeWidth={1.6} />}
                aria-label="下载历史"
                title="下载历史"
                aria-expanded={transferPopup === "download"}
                aria-haspopup="dialog"
                onClick={() => setTransferPopup((current) => (current === "download" ? null : "download"))}
              />
            </span>
            <span ref={settingsAnchor} className="code-transfer-anchor">
              <Button
                className="code-viewer-pane-toggle"
                variant="ghost"
                icon={<MoreHorizontal size={14} strokeWidth={1.6} />}
                aria-label="预览设置"
                title="预览设置"
                aria-expanded={settingsOpen}
                aria-haspopup="dialog"
                onClick={() => setSettingsOpen((current) => !current)}
              />
            </span>
            <Button
              className="code-viewer-pane-toggle"
              variant="ghost"
              icon={<X size={14} strokeWidth={1.6} />}
              aria-label="关闭预览窗口"
              title="关闭预览窗口"
              onClick={closeFloatingWindow}
            />
          </div>
        </div>
        <div className="code-viewer-file-content" aria-busy={editorMode ? browser.fileLoading : previewLoading}>
          {editorMode ? (
            browser.fileLoading ? (
              <div className="code-viewer-file-loading" role="status" aria-live="polite">
                <StepSpinner size={13} />
                <span>{kind === "ssh" ? "正在读取 SSH 文件…" : "正在读取文件…"}</span>
              </div>
            ) : !browser.activeFile || !editorFile ? (
              <div className="archive-preview-empty code-viewer-empty-state">
                <strong>未选择文件</strong>
                <span>从左侧文件树中选择文件。</span>
              </div>
            ) : editorFile.readOnlyFile && !editorFile.content ? (
              <div className="archive-preview-empty code-viewer-empty-state">
                <strong>无法打开此文件</strong>
                <span>该文件是二进制或超出预览大小限制。</span>
              </div>
            ) : editorMarkdown && browser.previewing ? (
              <MarkdownFilePreview source={browser.content} showOutline={outlineOpen} outlineId={previewOutlineId} />
            ) : (
              <CodeHighlightEditor
                value={browser.draft}
                path={browser.activeFile}
                onChange={browser.setDraft}
                readOnly={browser.readOnlyFile || browser.loading}
                ariaLabel={`${activeName} 编辑器`}
              />
            )
          ) : previewError ? (
            <div className="archive-preview-empty code-viewer-empty-state">
              <strong>无法预览此文件</strong>
              <span>{preview!.blocked ? previewError : `读取失败：${previewError}`}</span>
            </div>
          ) : previewLoading ? (
            <div className="code-viewer-file-loading" role="status" aria-live="polite">
              <StepSpinner size={13} />
              <span>正在读取文件…</span>
            </div>
          ) : markdownPreview ? (
            <MarkdownFilePreview source={previewText} showOutline={outlineOpen} outlineId={previewOutlineId} />
          ) : (
            <CodeHighlightEditor
              value={previewText}
              path={preview?.path ?? ""}
              onChange={() => undefined}
              readOnly
              ariaLabel={`${preview?.name ?? "文件"} 预览`}
            />
          )}
        </div>
      </article>
      {transferPopup === "download" && transferAnchor.current && (
        <ToolbarPopover anchor={transferAnchor.current} label="下载历史" onClose={() => setTransferPopup(null)}>
          <DownloadHistory
            preparing={transfers.preparing}
            preparationError={transfers.error?.direction === "download" ? transfers.error.message : undefined}
            onClose={() => setTransferPopup(null)}
          />
        </ToolbarPopover>
      )}
      {transferPopup === "upload" && uploadAnchor.current && (
        <ToolbarPopover anchor={uploadAnchor.current} label="上传记录" onClose={() => setTransferPopup(null)}>
          <UploadHistory {...transferPanel} onClose={() => setTransferPopup(null)} />
        </ToolbarPopover>
      )}
      {settingsOpen && settingsAnchor.current && (
        <ToolbarPopover anchor={settingsAnchor.current} label="预览设置" onClose={() => setSettingsOpen(false)}>
          <div className="code-viewer-settings">
            <div className="code-toolbar-popover-heading"><strong>预览设置</strong></div>
            <div className="code-viewer-setting-row">
              <span><b>自动刷新</b><small>每 5 秒检查磁盘上的当前文件，有外部改动时提示冲突</small></span>
              <Switch
                checked={browser.autoRefresh}
                aria-label="自动刷新当前文件"
                onCheckedChange={(checked) => browser.setAutoRefresh(checked)}
              />
            </div>
          </div>
        </ToolbarPopover>
      )}
    </FloatingWindow>
  ) : null;

  return (
    <>
      {treeTarget ? (
        <>
          {/* 文件预览页左侧控制行 1:1：同一组件（过滤框 + SSH 上传记录 + ⋮ 菜单）。 */}
          <CodeBrowserControls
            query={filter}
            onQuery={setFilter}
            ssh={kind === "ssh"}
            uploadOpen={browserUploadOpen}
            onUploadToggle={() => setBrowserUploadOpen((open) => !open)}
            onUploadClose={() => setBrowserUploadOpen(false)}
            transferPanel={transferPanel}
            onMenu={(anchor) => {
              const rect = anchor.getBoundingClientRect();
              setDirMenu({ entry: { name: folderName(treeRoot), path: treeRoot, isDirectory: true }, x: rect.left, y: rect.bottom, trigger: anchor });
            }}
          />
          {treeElement}
        </>
      ) : (
        <div className="terminal-drawer-empty">当前会话没有可用的 SSH 连接 ID，无法读取远端目录。</div>
      )}
      {dirMenu && treeTarget && (
        <ContextMenu
          x={dirMenu.x}
          y={dirMenu.y}
          label="文件操作"
          trigger={dirMenu.trigger}
          onClose={() => setDirMenu(null)}
          items={[
            ...(dirMenu.entry.isDirectory
              ? [
                  {
                    label: "新建文件",
                    icon: <Plus size={14} />,
                    disabled: !treeTarget || browserMutations.busy,
                    onSelect: () => browserMutations.begin(dirMenu.entry, "create-file", treeRoot),
                  },
                  {
                    label: "新建文件夹",
                    icon: <FolderOpen size={14} />,
                    disabled: !treeTarget || browserMutations.busy,
                    onSelect: () => browserMutations.begin(dirMenu.entry, "create-directory", treeRoot),
                  },
                  {
                    label: "在新标签中打开目录",
                    icon: <ExternalLink size={14} />,
                    onSelect: () => openFolderInNewTab(dirMenu.entry.path),
                  },
                  {
                    label: "刷新此目录",
                    icon: <RefreshCw size={14} />,
                    onSelect: () => void browser.loadFolder(dirMenu.entry.path, false, treeTarget),
                  },
                ]
              : [
                  {
                    label: "打开文件",
                    icon: <Eye size={14} />,
                    onSelect: () =>
                      openPreview({
                        path: dirMenu.entry.path,
                        name: dirMenu.entry.name,
                        isDirectory: false,
                        size: typeof dirMenu.entry.size === "number" ? dirMenu.entry.size : undefined,
                      }),
                  },
                ]),
            // 下载/上传项与文件预览页完全一致：文件和文件夹都能下载（后端对目录会先
            // 在远端 tar 打包再拉回），超过 1 GB 由 useTransferActions 弹二次确认。
            ...(kind === "ssh"
              ? [
                  {
                    label: "下载",
                    icon: <Download size={14} />,
                    disabled: !treeTarget || transfers.busy || dirMenu.entry.symlinkTarget != null,
                    onSelect: () => void transfers.download(dirMenu.entry.path),
                  },
                ]
              : []),
            ...(kind === "ssh" && dirMenu.entry.isDirectory && dirMenu.entry.symlinkTarget == null
              ? [
                  {
                    label: "上传文件到此目录",
                    icon: <Upload size={14} />,
                    disabled: !treeTarget || transfers.busy,
                    onSelect: () => void transfers.pickUpload(dirMenu.entry.path, false),
                  },
                  {
                    label: "上传文件夹到此目录",
                    icon: <FolderUp size={14} />,
                    disabled: !treeTarget || transfers.busy,
                    onSelect: () => void transfers.pickUpload(dirMenu.entry.path, true),
                  },
                ]
              : []),
            ...(kind === "ssh" && dirMenu.entry.path === treeRoot
              ? [
                  {
                    label: "显示隐藏文件",
                    icon: showHidden ? <Eye size={14} /> : <EyeOff size={14} />,
                    checked: showHidden,
                    onSelect: () => setShowHidden((value) => !value),
                  },
                ]
              : []),
            ...(dirMenu.entry.path === treeRoot
              ? [
                  {
                    label: "文件跟随终端目录",
                    icon: <LocateFixed size={14} />,
                    checked: followFiles,
                    onSelect: () => {
                      DRAWER_FOLLOW.files = !DRAWER_FOLLOW.files;
                      setFollowFiles(DRAWER_FOLLOW.files);
                    },
                  },
                ]
              : []),
            {
              label: "复制路径",
              icon: <Copy size={14} />,
              onSelect: () => {
                void navigator.clipboard
                  .writeText(dirMenu.entry.path)
                  .then(() => notify("已复制路径", "success"))
                  .catch(() => notify(`无法写入剪贴板，请手动复制：${dirMenu.entry.path}`, "error"));
              },
            },
            ...(dirMenu.entry.path !== treeRoot
              ? [
                  {
                    label: "重命名",
                    icon: <Pencil size={14} />,
                    disabled: !treeTarget || browserMutations.busy,
                    onSelect: () => browserMutations.begin(dirMenu.entry, "rename", treeRoot),
                  },
                  {
                    label: "永久删除",
                    icon: <Trash2 size={14} />,
                    danger: true,
                    disabled: !treeTarget || browserMutations.busy,
                    onSelect: () => browserMutations.begin(dirMenu.entry, "delete", treeRoot),
                  },
                ]
              : []),
            ...(dirMenu.entry.symlinkTarget != null
              ? [
                  {
                    label: "复制链接目标",
                    icon: <Link2 size={14} />,
                    onSelect: () => {
                      void navigator.clipboard
                        .writeText(dirMenu.entry.symlinkTarget!)
                        .then(() => notify("已复制链接目标", "success"))
                        .catch(() => notify(`无法写入剪贴板，请手动复制：${dirMenu.entry.symlinkTarget}`, "error"));
                    },
                  },
                ]
              : []),
          ]}
        />
      )}
      {previewCard}
      {/* 上传/下载的确认弹窗（同名覆盖、大文件、sudo 授权）由 useTransferActions
          提供；它挂在 FloatingWindow 之外，所以没有窗口时也能弹出来。 */}
      {transfers.dialog}
      {/* 重命名/新建/永久删除的确认弹窗，与文件预览页共用同一个组件与同一套预检。 */}
      {browserMutations.dialog}
    </>
  );
};

/** 抽屉里的统一加载态：居中 + 组件库 Spinner（"正在读取…"这类都走这里）。 */
const DrawerLoading: React.FC<{ text: string }> = ({ text }) => (
  <div className="terminal-drawer-loading">
    <StepSpinner size={18} label={text} />
    <span>{text}</span>
  </div>
);

const DrawerCard: React.FC<{
  icon: React.ReactNode;
  title: string;
  /** 主数值行；负载圆环/磁盘行这类「内容即数据」的卡可以不传。 */
  value?: string;
  hint?: string;
  children?: React.ReactNode;
}> = ({ icon, title, value, hint, children }) => (
  <div className="terminal-drawer-card">
    <div className="terminal-drawer-card-head">
      <span className="terminal-drawer-card-title">
        {icon}
        {title}
      </span>
      {hint && <span className="terminal-drawer-card-hint">{hint}</span>}
    </div>
    {value && <div className="terminal-drawer-card-value">{value}</div>}
    {children}
  </div>
);

const Meter: React.FC<{ percent: number; tone: "accent" | "warning" }> = ({ percent, tone }) => (
  <div className="terminal-drawer-meter">
    <div
      className={"terminal-drawer-meter-fill is-" + tone}
      style={{ width: `${Math.max(0, Math.min(100, percent))}%` }}
    />
  </div>
);

/** 变更文件显示用：文件名（最后一段）。 */
function basename(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const index = normalized.lastIndexOf("/");
  return index < 0 ? normalized : normalized.slice(index + 1);
}

/**
 * 变更文件显示用：目录部分，**从尾部截断**。
 * 需求 7 要求优先展示文件末尾——路径太长时前面几级目录通常最不重要，
 * 用 CSS 的 `direction: rtl` 会打乱标点，所以这里直接取最后两段目录。
 */
function dirnameTail(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const index = normalized.lastIndexOf("/");
  if (index <= 0) return "";
  const dir = normalized.slice(0, index);
  const parts = dir.split("/").filter(Boolean);
  const tail = parts.slice(-2).join("/");
  return parts.length > 2 ? `…/${tail}` : tail;
}

/** 右栏四个入口按钮组成的竖排工具条。 */
// 性能（十三轮）：抽屉整体 memo——index 的偶发重渲染（改标题等）
// 不再把三个面板（含文件树）整棵重算；onClose 语义恒定不参与比较。
export const TerminalDrawer = React.memo(
  TerminalDrawerImpl,
  (prev, next) =>
    prev.pane === next.pane &&
    prev.activeTab === next.activeTab &&
    prev.fallbackRoot === next.fallbackRoot &&
    prev.sshConnectionId === next.sshConnectionId,
);

export const TerminalDrawerRail: React.FC<{
  pane: DrawerPane | null;
  onToggle: (pane: DrawerPane) => void;
}> = ({ pane, onToggle }) => (
  <nav className="terminal-drawer-rail" aria-label="终端辅助面板">
    <Button
      type="button" variant="bare"
      className={"terminal-rail-btn" + (pane === "host" ? " active" : "")}
      aria-label="主机状态"
      aria-pressed={pane === "host"}
      title="主机状态：CPU、内存、端口、进程"
      onClick={() => onToggle("host")}
    >
      <Gauge size={16} />
    </Button>
    <Button
      type="button" variant="bare"
      className={"terminal-rail-btn" + (pane === "git" ? " active" : "")}
      aria-label="版本控制"
      aria-pressed={pane === "git"}
      title="版本控制：分支与变更文件"
      onClick={() => onToggle("git")}
    >
      <GitBranch size={16} />
    </Button>
    <Button
      type="button" variant="bare"
      className={"terminal-rail-btn" + (pane === "files" ? " active" : "")}
      aria-label="文件浏览"
      aria-pressed={pane === "files"}
      title="文件浏览：本地或远程目录"
      onClick={() => onToggle("files")}
    >
      {/* 用户明确要求 rail 上用普通文件夹图标（不要 FolderTree）。 */}
      <Folder size={16} />
    </Button>
  </nav>
);
