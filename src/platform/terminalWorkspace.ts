// ==========================================================================
// 终端工作区数据源 - 左侧栏「已保存服务器」与右侧栏的「文件 / Git」面板
//
// 三栏终端页需要三类真实数据：
//   1. 已保存的 SSH 连接、WSL 发行版  → 左侧「已保存服务器」列表
//   2. 当前目录的文件列表            → 右侧「文件」面板
//   3. 工作区 Git 状态               → 右侧「Git」面板
// 三者后端命令都已存在，这里只做一层带兜底的封装，避免视图里散落 invoke。
// ==========================================================================

import { invoke } from "./runtime";
import { isTauri } from "./runtime";
import { ensureSshHostConnected } from "./sshHostConnection";

/** 已保存的远程/环境目标（SSH 与 WSL 统一成一种视图模型） */
export interface SavedServerEntry {
  id: string;
  kind: "ssh" | "wsl";
  name: string;
  /** 副标题，例如 `root@10.0.0.12:22` 或 `Ubuntu-22.04 · /home/dev` */
  detail: string;
  /** 默认工作目录 */
  root: string;
}

interface SshConnectionRow {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  defaultRoot?: string | null;
}

interface WslProfileRow {
  id: string;
  name: string;
  distribution: string;
  defaultRoot?: string | null;
  shell: string;
}

/**
 * 读取已保存的 SSH 连接与 WSL 配置。
 * 任一命令失败都不影响另一个，失败的那一侧返回空数组。
 */
export async function listSavedServers(): Promise<SavedServerEntry[]> {
  if (!isTauri()) return [];
  const [ssh, wsl] = await Promise.all([
    invoke<SshConnectionRow[]>("list_ssh_connections").catch(() => [] as SshConnectionRow[]),
    invoke<WslProfileRow[]>("list_wsl_profiles").catch(() => [] as WslProfileRow[]),
  ]);
  const sshEntries: SavedServerEntry[] = ssh.map((item) => ({
    id: item.id,
    kind: "ssh",
    name: item.name,
    detail: `${item.username}@${item.host}:${item.port}`,
    root: item.defaultRoot?.trim() || "~",
  }));
  const wslEntries: SavedServerEntry[] = wsl.map((item) => ({
    id: item.id,
    kind: "wsl",
    name: item.name,
    detail: `${item.distribution} · ${item.defaultRoot?.trim() || "/"}`,
    root: item.defaultRoot?.trim() || "/",
  }));
  return [...wslEntries, ...sshEntries];
}

/** 文件面板的目录条目 */
export interface DirectoryEntry {
  name: string;
  path: string;
  isDirectory: boolean;
  size?: number | null;
}

interface CodeViewerEntryRow {
  name: string;
  path: string;
  /** 后端 `#[serde(rename_all = "camelCase")]` 序列化成 `isDirectory`；
      snake_case 保留兼容旧版本后端。两者都读，别只认一种。 */
  isDirectory?: boolean;
  is_directory?: boolean;
  size?: number | null;
  kind: string;
}

/**
 * 读取本地目录。
 * 复用代码查看器的目录命令，它会过滤隐藏文件与构建产物。
 */
export async function listLocalDirectory(path: string): Promise<DirectoryEntry[]> {
  if (!isTauri() || !path) return [];
  const rows = await invoke<CodeViewerEntryRow[]>("list_code_viewer_directory", { path });
  return rows.map((row) => ({
    name: row.name,
    path: row.path,
    isDirectory: Boolean(row.isDirectory ?? row.is_directory),
    size: row.size ?? null,
  }));
}

/** 工作区 Git 概况 */
export interface WorkspaceGitSummary {
  available: boolean;
  repository: boolean;
  branch: string;
  dirty: boolean;
  changedFiles: number;
  addedLines: number;
  removedLines: number;
  /** 后端解析出的绝对路径（传 "." 时用于回填真实工作目录） */
  path: string;
}

interface WorkspaceEnvironmentRow {
  path: string;
  workspaceName: string;
  git: {
    available: boolean;
    repository: boolean;
    branch?: string | null;
    dirty: boolean;
    changedFiles: number;
    addedLines: number;
    removedLines: number;
  };
}

/** 读取工作区 Git 概况；路径为空或非仓库时返回 available = false 的占位。 */
export async function readWorkspaceGit(path: string): Promise<WorkspaceGitSummary> {
  const empty: WorkspaceGitSummary = {
    available: false,
    repository: false,
    branch: "",
    dirty: false,
    changedFiles: 0,
    addedLines: 0,
    removedLines: 0,
    path: "",
  };
  if (!isTauri() || !path) return empty;
  try {
    const environment = await invoke<WorkspaceEnvironmentRow>("get_workspace_environment", { path });
    const git = environment.git;
    return {
      available: git.available,
      repository: git.repository,
      branch: git.branch || "",
      dirty: git.dirty,
      changedFiles: git.changedFiles,
      addedLines: git.addedLines,
      removedLines: git.removedLines,
      path: environment.path || "",
    };
  } catch {
    return empty;
  }
}

/** 工作区变更文件条目 */
export interface WorkspaceGitChange {
  path: string;
  status: string;
  addedLines: number;
  removedLines: number;
}

/** 读取工作区变更文件列表 */
export async function readWorkspaceGitChanges(path: string): Promise<WorkspaceGitChange[]> {
  if (!isTauri() || !path) return [];
  try {
    return await invoke<WorkspaceGitChange[]>("get_workspace_git_changes", { path });
  } catch {
    return [];
  }
}

/**
 * 通过 SSH 读取远端工作区的 Git 概况（右栏「版本控制」页签在 SSH 页签下用）。
 *
 * 形状与本地 `readWorkspaceGit` / `readWorkspaceGitChanges` 完全一致：
 * 后端 `ssh_workspace_git` 一次返回 summary + changes，这里拆成同一对类型，
 * 右栏渲染与缓存逻辑不必区分本地/远端。
 *
 * 与本地版的两点差别都是**故意的**：
 * 1. 会先 `ensureSshReady`——终端页签的 SSH 是独立 ssh.exe 子进程，不登记连接
 *    状态，不先连一次的话后端会直接报「尚未连接」。
 * 2. 失败**不吞异常**：本地版失败退化成空占位，SSH 版必须把原因交给界面显示，
 *    否则用户看到的是「不是 Git 仓库」，而真实原因其实是通道断了。
 */
export async function readSshWorkspaceGit(
  connectionId: string,
  root: string,
): Promise<{ summary: WorkspaceGitSummary; changes: WorkspaceGitChange[] }> {
  const unavailable: WorkspaceGitSummary = {
    available: false,
    repository: false,
    branch: "",
    dirty: false,
    changedFiles: 0,
    addedLines: 0,
    removedLines: 0,
    path: root,
  };
  if (!isTauri()) return { summary: unavailable, changes: [] };
  if (!connectionId.trim()) throw new Error("当前会话没有可用的 SSH 连接 ID，无法读取远端 Git 状态。");
  await ensureSshReady(connectionId);
  const result = await invoke<{ summary?: WorkspaceGitSummary; changes?: WorkspaceGitChange[] }>(
    "ssh_workspace_git",
    { connectionId, root },
  );
  return { summary: result?.summary ?? unavailable, changes: result?.changes ?? [] };
}

/**
 * 通过 SSH 查询远端当前工作目录（cwd 跟随的兜底通路，返回绝对路径）。
 *
 * `root` 是上一次已知的目录：这条通路每条命令都是新建的远端进程，不先 cd 过去
 * 就永远只会拿到登录目录。传空串表示从登录目录开始探。
 */
export async function readSshWorkingDirectory(connectionId: string, root = ""): Promise<string> {
  if (!isTauri() || !connectionId.trim()) return "";
  return (await invoke<string>("query_ssh_cwd", { connectionId, root })) || "";
}

/**
 * 确保某台 SSH 主机在右栏操作前处于「已连接」状态。
 *
 * 为什么需要它：终端页签里的 SSH 会话是**独立 ssh.exe 子进程**（terminal.rs 的
 * ssh_command 直接 load_connection_for_agent 起进程），它不会往 SshRuntimeState
 * 里登记 connection_progress；而代码查看器那套命令（list_ssh_code_viewer_directory /
 * read_ssh_code_viewer_file）走 connected_connection_for_agent，没有登记就直接
 * 返回「指定的 SSH 尚未连接，请先连接后再发送。」——这正是右栏文件浏览报错的根因。
 *
 * 这里复用 `sshHostConnection.ts` 已有的做法：`connect_ssh_host_command` 只建立
 * 传输通道、**不改写**菜单栏的全局执行环境。已经 connected 时它内部会直接返回，
 * 只剩一次 IPC 往返，不会重复认证。
 *
 * 并发的同主机请求共用同一个 Promise，避免同时发起多条认证；
 * 失败后从表里摘掉，下一次请求可以重试。
 */
const sshReadyInFlight = new Map<string, Promise<unknown>>();

async function ensureSshReady(connectionId: string): Promise<void> {
  const id = connectionId.trim();
  if (!id) throw new Error("当前会话没有可用的 SSH 连接 ID，无法浏览远端目录。");
  const pending = sshReadyInFlight.get(id);
  if (pending) {
    await pending;
    return;
  }
  const task = ensureSshHostConnected(id);
  sshReadyInFlight.set(id, task);
  try {
    await task;
  } finally {
    sshReadyInFlight.delete(id);
  }
}

/**
 * 通过 SSH 读取远端目录（需求 5：SSH 会话下文件浏览可用）。
 *
 * 走 `list_ssh_code_viewer_directory`：它是代码查看器已在用的那条链路，
 * 返回 name/path/isDirectory/size，比 `list_ssh_directories_command`（只给目录）
 * 更完整——文件面板要能同时看到文件。
 */
export async function listSshDirectory(connectionId: string, path: string): Promise<DirectoryEntry[]> {
  if (!isTauri()) return [];
  // 空串绝不传给后端：后端会先报「SSH 执行目标缺少连接 ID」，回显给用户没有意义。
  if (!connectionId.trim()) throw new Error("当前会话没有可用的 SSH 连接 ID，无法浏览远端目录。");
  if (!path) return [];
  await ensureSshReady(connectionId);
  interface SshEntryRow {
    name: string;
    path: string;
    isDirectory?: boolean;
    is_directory?: boolean;
    size?: number | null;
  }
  const result = await invoke<{ entries?: SshEntryRow[] } | SshEntryRow[]>(
    "list_ssh_code_viewer_directory",
    { connectionId, path },
  );
  const rows = Array.isArray(result) ? result : result?.entries ?? [];
  return rows.map((row) => ({
    name: row.name,
    path: row.path,
    isDirectory: Boolean(row.isDirectory ?? row.is_directory),
    size: row.size ?? null,
  }));
}

/**
 * 通过 SSH 读取远端文本文件（需求 6：点文件能预览）。
 * 视图只调这一个入口。
 */
export async function readRemoteTextFile(input: {
  kind: "ssh";
  connectionId?: string;
  root?: string;
  path: string;
}): Promise<string> {
  if (!isTauri()) return "";
  // 与 listSshDirectory 同理：空 connectionId 兜成明确错误，不把空串传给后端。
  const connectionId = (input.connectionId ?? "").trim();
  if (!connectionId) throw new Error("当前会话没有可用的 SSH 连接 ID，无法读取远端文件。");
  await ensureSshReady(connectionId);
  return await invoke<string>("read_ssh_code_viewer_file", {
    connectionId,
    root: input.root || "~",
    path: input.path,
  });
}
