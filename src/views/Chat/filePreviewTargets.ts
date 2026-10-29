import type { ExecutionTarget } from "./types.ts";

/**
 * 文件预览页要读取哪台机器。
 *
 * 预览是**单选**：同一时刻只从一个环境读文件。候选清单固定是「本机工作区 +
 * 该对话绑定的远程环境」，默认选中该对话的默认执行环境；若打开的是 Unix 绝对
 * 路径（远程路径形态），则优先落到远程主机，否则会拿 Windows 的读文件命令去
 * 读 `/home/...` 并直接失败。
 */
export type FilePreviewTarget = {
  /** 稳定的选择键：`local` 或 `${kind}:${id}`。 */
  key: string;
  kind: "local" | "ssh";
  id: string;
  /** 完整称呼，用于下拉里的说明。 */
  label: string;
  /** 触发按钮上的短称呼：面板头部很窄，只放得下主机名。 */
  short: string;
  /** 该环境的根目录；本机是工作区目录，远程是 `~` 或配置里的默认根。 */
  root: string;
};

export const LOCAL_PREVIEW_TARGET_KEY = "local";

export function filePreviewTargetKey(target: Pick<FilePreviewTarget, "kind" | "id">): string {
  return target.kind === "local" ? LOCAL_PREVIEW_TARGET_KEY : `${target.kind}:${target.id.trim()}`;
}

function remoteLabel(target: ExecutionTarget): string {
  const label = (target.label || "").trim();
  return label || `SSH · ${target.id}`;
}

/** 去掉 `SSH · ` 之类的前缀，只留主机名给触发按钮用。 */
function shortLabel(target: ExecutionTarget): string {
  const label = (target.label || "").trim().replace(/^SSH\s*·\s*/i, "");
  return label || (target.id || "").trim();
}

/**
 * 组装预览可选环境：本机工作区在前，随后是对话绑定的 SSH 主机（按绑定顺序、去重）。
 *
 * 只收 SSH：预览靠 `read_ssh_code_viewer_file` 读远程文件，WSL 目标没有对应的
 * 读文件命令，列出来只会让用户点到必然失败的一项。
 * 默认执行环境即使是 SSH 且不在绑定清单里也会补进来，保证「当前选哪个就用哪个」。
 *
 * `fallback` 是兜底主机：会话在发送时保存的执行环境、以及本机已保存的 SSH 服务器。
 * 它们不在绑定清单里也必须可选——工作区是 Windows 盘符的项目，`conversationExecutionTarget`
 * 会把执行环境判成本机（不允许把盘符路径发给远程工具），于是 AI 通过 `ssh__*` 改出来的
 * 远程路径就没有任何主机可以归属，点开预览只能拿本机命令去读 `/data/...` 并报
 * 「系统找不到指定的文件」。有兜底项时用户还能在预览环境下拉里手动指一台主机。
 */
export function filePreviewTargets(
  defaultTarget: ExecutionTarget,
  bound: ExecutionTarget[],
  workspacePath: string,
  fallback: ExecutionTarget[] = [],
): FilePreviewTarget[] {
  const local: FilePreviewTarget = {
    key: LOCAL_PREVIEW_TARGET_KEY,
    kind: "local",
    id: "",
    label: "本机工作区",
    short: "本机",
    root: workspacePath,
  };
  const seen = new Set<string>([LOCAL_PREVIEW_TARGET_KEY]);
  const remote: FilePreviewTarget[] = [];
  const push = (target: ExecutionTarget) => {
    if (target.kind !== "ssh") return;
    const id = (target.id || "").trim();
    if (!id) return;
    const key = `ssh:${id}`;
    if (seen.has(key)) return;
    seen.add(key);
    remote.push({
      key,
      kind: "ssh",
      id,
      label: remoteLabel(target),
      short: shortLabel(target),
      root: (target.root || "").trim() || "~",
    });
  };
  push(defaultTarget);
  for (const target of bound) push(target);
  for (const target of fallback) push(target);
  return [local, ...remote];
}

/** 看起来是远程（Unix）绝对路径：以 `/` 开头但不是 UNC。 */
export function looksRemotePath(path: string): boolean {
  const value = path.trim();
  return value.startsWith("/") && !value.startsWith("//");
}

/** 看起来是本机绝对路径：盘符或 UNC。 */
export function looksLocalPath(path: string): boolean {
  return /^(?:[A-Za-z]:[\\/]|\\\\)/.test(path.trim());
}

function preferredSshKey(defaultTarget: ExecutionTarget): string {
  if (defaultTarget.kind !== "ssh") return "";
  return filePreviewTargetKey({ kind: "ssh", id: (defaultTarget.id || "").trim() });
}

/**
 * 远程路径优先落到「根目录确实包含它」的那台主机。
 *
 * 同时绑定了多台机器时，`/srv/logs/a.log` 该给日志机而不是随手第一台；没有哪台
 * 明确包含时再退回第一台远程主机（例如根目录写 `~` 的机器，前端拿不到展开值）。
 */
function bestRemoteTarget(targets: FilePreviewTarget[], path: string): FilePreviewTarget | undefined {
  const remotes = targets.filter((item) => item.kind !== "local");
  if (remotes.length < 2) return remotes[0];
  const value = path.trim();
  return remotes.find((item) => {
    const root = item.root.trim().replace(/\/+$/, "");
    return isAbsoluteRemotePath(root) && (value === root || value.startsWith(`${root}/`));
  }) || remotes[0];
}

/**
 * 选中哪个环境。
 *
 * 优先级：用户显式选择（仍存在时）→ 路径形态（Unix 绝对路径用远程、盘符/UNC 用本机）
 * → 对话默认执行环境 → 本机。
 *
 * 路径形态必须先于默认环境判断：对话默认在 SSH 上时，AI 引用的本机路径
 * （`D:\\repo\\a.ts`）不该被送去远程读，反之亦然。
 */
export function resolvePreviewTarget(
  targets: FilePreviewTarget[],
  path: string,
  defaultTarget: ExecutionTarget,
  selectedKey?: string,
): FilePreviewTarget | null {
  if (!targets.length) return null;
  const byKey = (key: string) => targets.find((item) => item.key === key);
  const selected = selectedKey ? byKey(selectedKey) : undefined;
  // 显式选择优先。选到 SSH 时本机路径仍可读：`remotePreviewLocation` 会把工作区
  // 前缀换成远程根目录，读的是「同一份文件的远程版本」；读不到再退回本机，不硬报错。
  if (selected) return selected;
  const local = byKey(LOCAL_PREVIEW_TARGET_KEY);
  if (looksRemotePath(path)) {
    const remote = bestRemoteTarget(targets, path) || byKey(preferredSshKey(defaultTarget));
    if (remote) return remote;
  }
  if (looksLocalPath(path) && local) return local;
  return byKey(preferredSshKey(defaultTarget)) || local || targets[0];
}

/**
 * 把路径解析到目标环境的绝对路径。
 *
 * 已经是绝对路径（Windows 盘符、UNC、Unix `/`）时原样返回；相对路径拼到该环境
 * 的根目录上，避免把相对路径交给远程命令去按远端当前目录猜测。
 */
export function resolvePreviewPath(path: string, target: FilePreviewTarget): string {
  const value = path.trim();
  if (!value) return value;
  if (/^(?:[A-Za-z]:[\\/]|\\\\|\/)/.test(value)) return value;
  const root = target.root.replace(/[\\/]+$/, "");
  if (!root) return value;
  const relative = value.replace(/^\.?[\\/]+/, "");
  return `${root}/${relative}`;
}

function isAbsoluteRemotePath(path: string): boolean {
  return path.startsWith("/") && !path.startsWith("//");
}

/**
 * 本机工作区路径 → 远程根目录下的同名路径。
 *
 * 工作区是 `D:\repo`、远程根是 `/srv/repo` 时，`D:\repo\src\a.ts` 变成 `/srv/repo/src/a.ts`。
 * 映射不出来（路径不在工作区里、或者远程根是 `~` 这种拿不到展开值的写法）就返回空串，
 * 让调用方给出「请改用远程绝对路径」的提示，而不是硬拼一个必然失败的路径。
 */
function mapLocalWorkspacePath(path: string, workspacePath: string, remoteRoot: string): string {
  const workspace = workspacePath.trim().replace(/[\\/]+$/, "");
  if (!workspace || !looksLocalPath(path)) return "";
  const normalizedPath = path.replace(/\\/g, "/").toLowerCase();
  const normalizedWorkspace = workspace.replace(/\\/g, "/").toLowerCase();
  if (normalizedPath !== normalizedWorkspace && !normalizedPath.startsWith(`${normalizedWorkspace}/`)) {
    return "";
  }
  const relative = path.slice(workspace.length).replace(/\\/g, "/").replace(/^\/+/, "");
  const root = remoteRoot.trim().replace(/\\/g, "/").replace(/\/+$/, "");
  const expandedRoot = root && root !== "~" && !root.startsWith("~") ? root : "";
  if (!expandedRoot.startsWith("/")) return "";
  return relative ? `${expandedRoot}/${relative}` : expandedRoot;
}

/**
 * 远程读文件命令需要的 `root` 与 `path`。
 *
 * `read_ssh_code_viewer_file` 要求两者都是 `/` 开头的绝对路径，并且 `path` 必须位于
 * `root` 之内。远端根目录常常写成 `~` 或留空（此时按用户 home 解析），前端拿不到
 * 展开后的真实值，所以只有在配置里明确写了绝对根目录、且文件确实落在里面时才用
 * 它；否则退回 `/`，让包含判断始终成立。
 *
 * 返回 null 表示这个路径没法远程读（相对路径又没有可用的绝对根目录）。
 */
export function remotePreviewLocation(
  path: string,
  target: FilePreviewTarget,
  workspacePath = "",
): { connectionId: string; root: string; path: string } | null {
  let resolved = resolvePreviewPath(path, target);
  if (!isAbsoluteRemotePath(resolved)) {
    // 远程环境里点开本机工作区的文件（`D:\repo\a.ts`）：后端只认 `/` 开头的路径，
    // 但用户要看的往往就是同名的那份远程文件，直接把工作区前缀换成远程根目录。
    const mapped = mapLocalWorkspacePath(resolved, workspacePath, target.root);
    if (!mapped) return null;
    resolved = mapped;
  }
  const configured = target.root.trim();
  const root = isAbsoluteRemotePath(configured)
    && (resolved === configured || resolved.startsWith(`${configured.replace(/\/+$/, "")}/`))
    ? configured.replace(/\/+$/, "") || "/"
    : "/";
  return { connectionId: target.id, root, path: resolved };
}
