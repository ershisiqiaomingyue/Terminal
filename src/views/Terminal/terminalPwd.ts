/**
 * 终端当前目录（cwd 跟随）的路径归一化。
 *
 * 内核从 OSC 7 报上来的**不是路径，是 URI**：`file://host/path`。原来
 * `handlePanePwdChange` 直接把这个字符串当页签 `root` 用，于是右栏文件面板
 * 拿到 `file://FANGZIXUAN/D:/workspace/Android-Tool` 去列举目录——后端拼出来的
 * 路径根本不存在，面板永远停在打开时的目录，看起来就是「不跟随 cd」。
 *
 * 这里把三类来源统一成一条真实路径：
 *   1. OSC 7 的 URI（本机 / 远端都可能带 host）
 *   2. 已经是裸路径（`/home/u`、`D:\work`、`~`）——原样返回，只做分隔符归一
 *   3. 百分号转义过的路径（`/home/a%20b`）
 */

export type TerminalPwdKind = "local" | "ssh" | "wsl";

/** 取出 `file://host/path` 里的 path 部分；没有 scheme 就返回 null。 */
function uriPath(raw: string): string | null {
  const lower = raw.toLowerCase();
  if (!lower.startsWith("file:")) return null;
  let rest = raw.slice("file:".length);
  // `file:/path` 与 `file:///path` 都合法，`file://host/path` 里 host 到第一个
  // 斜杠为止。先砍掉 authority，再补回起始斜杠。
  if (rest.startsWith("//")) {
    rest = rest.slice(2);
    const slash = rest.indexOf("/");
    rest = slash < 0 ? "" : rest.slice(slash);
  }
  if (!rest.startsWith("/")) rest = `/${rest}`;
  return rest;
}

/** 百分号转义解码：URI 里的空格/中文必须以字面量交给后端。 */
function decodePath(value: string): string {
  if (!value.includes("%")) return value;
  try {
    return decodeURIComponent(value);
  } catch {
    // 半截转义（`%E4%B` 这种被截断的）不能整条丢掉，保留原样让后端报错。
    return value;
  }
}

/**
 * Windows 本地路径的形状修正：URI 给的是 `/D:/workspace/x`，去掉前导斜杠、
 * 盘符大写、分隔符换反斜杠——与 `list_code_viewer_directory` 期望的一致。
 */
function windowsPath(value: string): string {
  let next = value;
  const drive = /^\/?([a-zA-Z]):[\\/]/.exec(next);
  if (drive) {
    next = `${drive[1].toUpperCase()}:${next.slice(drive[0].length - 1)}`;
  }
  return next.replace(/\//g, "\\");
}

/**
 * 把内核/轮询报上来的当前目录变成可直接当 `root` 用的路径。
 *
 * 返回空串表示「这条上报不可用」，调用方应当忽略（例如 OSC 7 的 host 段被
 * 截断成空）。
 */
export function normalizeTerminalPwd(raw: string, kind: TerminalPwdKind): string {
  const text = raw.trim();
  if (!text) return "";
  // `~` / `$HOME` 是 SSH 的相对家目录约定，不能被当成 URI 或做分隔符转换。
  if (text === "~" || text === "$HOME" || text === "${HOME}") return text;
  if (text.startsWith("~/") || text.startsWith("$HOME/") || text.startsWith("${HOME}/")) {
    return text;
  }
  const fromUri = uriPath(text);
  const path = decodePath(fromUri ?? text);
  if (!path) return "";
  if (kind === "local") return windowsPath(path);
  // SSH 远端是 POSIX 路径：只去掉多余的反斜杠，保留 `/`。
  return path.replace(/\\/g, "/");
}
