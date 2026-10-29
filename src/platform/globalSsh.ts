import { useSyncExternalStore } from "react";
import { disconnectSshConnection, getSshConnectionProgress, listSshConnections, startSshConnection, type SshConnection, type SshConnectionProgress } from "./api";
import { formatInvokeError, isTauri } from "./runtime";
import { setRemoteAutoConnectPaused } from "./remoteConnectionPolicy";

export type GlobalExecutionTarget = { kind: "local" | "ssh" | "wsl"; id?: string; label: string; root?: string };

export type GlobalSshSnapshot = {
  connections: SshConnection[];
  connection: SshConnection | null;
  progress: SshConnectionProgress | null;
  /** 每条连接各自的最新状态（R8：左栏「已保存的服务器」逐台绿点）。 */
  progressById: Record<string, SshConnectionProgress>;
  loaded: boolean;
  error: string;
};

let snapshot: GlobalSshSnapshot = { connections: [], connection: null, progress: null, progressById: {}, loaded: false, error: "" };
let selectedId: string | null = null;
let generation = 0;
let pendingActionGeneration: number | null = null;
let refreshPromise: Promise<GlobalSshSnapshot> | null = null;
let timer: number | null = null;
const listeners = new Set<() => void>();

/**
 * 终端页签直起 `ssh.exe` 时的本地状态覆盖。
 *
 * 终端里的 SSH 走的是 PTY 直起子进程（`src-tauri/src/terminal.rs` 的
 * `TerminalTarget::Ssh`），完全不经过 Rust 的 `SshRuntimeState`；而全局状态的
 * 轮询读的是 `getSshConnectionProgress`，对这条连接永远返回 `disconnected`。
 * 结果就是用户明明已经在 SSH 终端里敲命令了，右上角还是灰的。
 *
 * 这里让终端侧显式登记，轮询结果里处于连接态的后端值仍然优先（后端更权威），
 * 只有后端说"没连"时才采用本地覆盖。**不新增任何轮询**：只在开/关终端时写一次。
 */
const terminalSessions = new Map<string, { status: "connecting" | "connected"; updatedAt: number }>();

function terminalProgress(connectionId: string, base: SshConnectionProgress): SshConnectionProgress {
  const local = terminalSessions.get(connectionId);
  if (!local) return base;
  // 后端已经给出连接态：以后端为准，本地记录退场。
  if (base.status === "connected" || base.status === "connecting") return base;
  return {
    ...base,
    status: local.status,
    progress: local.status === "connected" ? 100 : 0,
    message: local.status === "connected" ? "终端 SSH 会话已连接" : "终端 SSH 会话连接中",
    error: undefined,
    updatedAt: local.updatedAt,
  };
}

/**
 * 终端侧上报 SSH 会话状态。
 *
 * - `connecting`：PTY 已创建，等待 ssh 建立连接；
 * - `connected`：首次收到远端输出，认为连接已建立；
 * - `closed`：终端关闭/会话退出，撤销本地覆盖。
 *
 * 只在开/关终端时调用，**不参与任何定时器**（性能要求：不得引入常驻轮询）。
 */
export function reportTerminalSshSession(connectionId: string, status: "connecting" | "connected" | "closed") {
  const id = connectionId.trim();
  if (!id) return;
  if (status === "closed") {
    if (!terminalSessions.delete(id)) return;
    // 只有当前展示的就是这条连接时才需要推动一次快照，否则等下一轮刷新即可。
    if (snapshot.connection?.id !== id) return;
    const fallback = snapshot.progress;
    publish(withConnectionProgress(snapshot, id, {
      connectionId: id,
      status: "disconnected",
      progress: 0,
      message: fallback && fallback.connectionId === id ? "终端已关闭" : "",
      updatedAt: Date.now(),
    }));
    return;
  }
  terminalSessions.set(id, { status, updatedAt: Date.now() });
  // 幂等：终端每收到一批输出都会上报一次 `connected`，这是数据热路径，
  // 状态没变就必须直接返回，否则每帧都会 publish 一次、把所有订阅者重渲染。
  const shown = snapshot.connection?.id === id ? snapshot.progress?.status : undefined;
  if (shown === status) return;
  const connection = snapshot.connections.find((item) => item.id === id) || null;
  if (!connection) {
    // 连接列表还没加载过（终端是启动后第一个操作）：拉一次列表，拉到后会自动采用覆盖值。
    if (!snapshot.loaded) void refreshGlobalSshConnection();
    return;
  }
  selectedId = id;
  const base: SshConnectionProgress = snapshot.progress?.connectionId === id
    ? snapshot.progress
    : { connectionId: id, status: "disconnected", progress: 0, message: "", updatedAt: 0 };
  publish(withConnectionProgress({ ...snapshot, connection, loaded: true, error: "" }, id, terminalProgress(id, base)));
}

function publish(next: GlobalSshSnapshot) {
  snapshot = next;
  for (const listener of listeners) listener();
}

/**
 * 把某条连接的最新进度写进快照：选中连接的 `progress` 与逐台表 `progressById`
 * 一起更新，保证顶栏呼吸灯、左栏绿点、状态面板三处看到的是同一份状态
 * （v3 设计文档 §3.4：单一事实源，不新增轮询）。
 */
function withConnectionProgress(base: GlobalSshSnapshot, id: string, progress: SshConnectionProgress): GlobalSshSnapshot {
  return {
    ...base,
    progress: base.connection?.id === id ? progress : base.progress,
    progressById: { ...base.progressById, [id]: progress },
  };
}

export function getGlobalSshSnapshot() {
  return snapshot;
}

export function globalSshTarget(state: GlobalSshSnapshot, fallback: GlobalExecutionTarget): GlobalExecutionTarget {
  const connection = state.connection;
  if (!connection || state.progress?.status === "disconnected") return fallback.kind === "ssh" ? { kind: "local", label: "本机工作区" } : fallback;
  return { kind: "ssh", id: connection.id, label: `SSH · ${connection.name}`, root: connection.defaultRoot?.trim() || "~" };
}

export function refreshGlobalSshConnection(): Promise<GlobalSshSnapshot> {
  if (pendingActionGeneration !== null) return Promise.resolve(snapshot);
  if (refreshPromise) return refreshPromise;
  const refreshGeneration = generation;
  refreshPromise = (async () => {
    if (!isTauri()) {
      publish({ ...snapshot, loaded: true });
      return snapshot;
    }
    try {
      const connections = await listSshConnections();
      const states = await Promise.all(connections.map(async (connection) => ({
        connection,
        // 终端直起的 ssh.exe 不在后端 SshRuntimeState 里，这里合并本地覆盖，
        // 否则用户在 SSH 终端里敲命令时右上角永远是灰色的。
        progress: terminalProgress(connection.id, await getSshConnectionProgress(connection.id).catch((error): SshConnectionProgress => ({ connectionId: connection.id, status: "failed", progress: 0, message: formatInvokeError(error), error: formatInvokeError(error), updatedAt: Date.now() }))),
      })));
      if (refreshGeneration !== generation) return snapshot;
      const rank = (status: string) => status === "connected" ? 3 : status === "connecting" ? 2 : status === "failed" ? 1 : 0;
      const selected = (selectedId ? states.find((state) => state.connection.id === selectedId) : null)
        || states.filter((state) => rank(state.progress.status) > 0)
          .sort((left, right) => rank(right.progress.status) - rank(left.progress.status) || right.progress.updatedAt - left.progress.updatedAt)[0];
      selectedId = selected?.connection.id || null;
      const progressById: Record<string, SshConnectionProgress> = {};
      for (const state of states) progressById[state.connection.id] = state.progress;
      publish({ connections, connection: selected?.connection || null, progress: selected?.progress || null, progressById, loaded: true, error: "" });
    } catch (error) {
      if (refreshGeneration === generation) publish({ ...snapshot, loaded: true, error: formatInvokeError(error) });
    }
    return snapshot;
  })().finally(() => { refreshPromise = null; });
  return refreshPromise;
}

export async function connectGlobalSsh(connection: SshConnection) {
  const current = snapshot.connection;
  const sameConnection = current && (["id", "host", "port", "username", "privateKeyPath", "strictHostKey", "defaultRoot"] as const)
    .every((key) => current[key] === connection[key]);
  if (sameConnection && (snapshot.progress?.status === "connecting"
    || (snapshot.progress?.status === "connected" && pendingActionGeneration === null))) return snapshot.progress;
  setRemoteAutoConnectPaused("ssh", connection.id, false);
  const actionGeneration = ++generation;
  pendingActionGeneration = actionGeneration;
  selectedId = connection.id;
  publish(withConnectionProgress({ ...snapshot, connection, loaded: true, error: "" }, connection.id, { connectionId: connection.id, status: "connecting", progress: 0, message: "正在连接全局 SSH", updatedAt: Date.now() }));
  try {
    const progress = await startSshConnection(connection.id, connection.defaultRoot || undefined);
    if (actionGeneration === generation) publish(withConnectionProgress({ ...snapshot, connection, error: "" }, connection.id, progress));
    return progress;
  } catch (error) {
    if (actionGeneration === generation) publish(withConnectionProgress({ ...snapshot, connection }, connection.id, { connectionId: connection.id, status: "failed", progress: 0, message: formatInvokeError(error), error: formatInvokeError(error), updatedAt: Date.now() }));
    throw error;
  } finally {
    if (pendingActionGeneration === actionGeneration) pendingActionGeneration = null;
  }
}

export async function disconnectGlobalSsh(connection: SshConnection) {
  setRemoteAutoConnectPaused("ssh", connection.id, true);
  const actionGeneration = ++generation;
  pendingActionGeneration = actionGeneration;
  selectedId = connection.id;
  try {
    const progress = await disconnectSshConnection(connection.id);
    if (actionGeneration === generation) publish(withConnectionProgress({ ...snapshot, connection, loaded: true, error: "" }, connection.id, progress));
    return progress;
  } finally {
    if (pendingActionGeneration === actionGeneration) pendingActionGeneration = null;
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    void refreshGlobalSshConnection();
    timer = window.setInterval(() => void refreshGlobalSshConnection(), 2_000);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size && timer !== null) {
      window.clearInterval(timer);
      timer = null;
    }
  };
}

export function useGlobalSshConnection() {
  return useSyncExternalStore(subscribe, getGlobalSshSnapshot, getGlobalSshSnapshot);
}

/**
 * 等待某条连接到达终态（connected / failed）或超时。
 *
 * 供终端视图的「SSH 连接门控」用（v3 R8 附带修复）：开 SSH 终端前先真实建连，
 * `startSshConnection` 可能带着 connecting 异步返回，这里以 ~250ms 步进盯快照
 * （顺带驱动既有刷新，**不新增常驻轮询**，门控结束轮询即停）。
 */
export async function awaitSshConnected(id: string, timeoutMs = 35_000): Promise<SshConnectionProgress | null> {
  const startedAt = Date.now();
  const read = () => snapshot.progressById[id] ?? (snapshot.connection?.id === id ? snapshot.progress ?? null : null);
  let progress = read();
  while (Date.now() - startedAt < timeoutMs) {
    if (progress?.status === "connected" || progress?.status === "failed") return progress;
    await new Promise((resolve) => setTimeout(resolve, 250));
    void refreshGlobalSshConnection();
    progress = read();
  }
  return read();
}

/**
 * 服务器连接展示四态（v3 R8：顶栏呼吸灯与左栏服务器状态点共用同一映射）。
 * 绿=已连接（呼吸）、黄=连接中（呼吸）、红=连接失败、灰=未连接。
 */
export type SshServerDotStatus = "connected" | "connecting" | "failed" | "off";

export function sshServerDotStatus(progress: SshConnectionProgress | null | undefined): SshServerDotStatus {
  if (progress?.status === "connected") return "connected";
  if (progress?.status === "connecting") return "connecting";
  if (progress?.status === "failed") return "failed";
  return "off";
}

export function restoreGlobalSshSelection(id: string | null) { selectedId = id; }
