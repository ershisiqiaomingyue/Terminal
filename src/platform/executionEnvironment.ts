import type { SshConnection, SshConnectionProgress, WslProfile } from "./api";
import type { GlobalExecutionTarget } from "./globalSsh";

export type EnvironmentSelection = "auto" | "local" | "ssh" | "wsl";
export type WslConnectionState = "idle" | "connecting" | "connected" | "failed";

/** 一个已保存的 SSH 主机对应的执行环境（供全局选择与会话绑定共用）。 */
export function environmentTargetForSsh(connection: SshConnection): GlobalExecutionTarget {
  return { kind: "ssh", id: connection.id, label: `SSH · ${connection.name}`, root: connection.defaultRoot?.trim() || "~" };
}

/** 一个 WSL 配置对应的执行环境（供全局选择与会话绑定共用）。 */
export function environmentTargetForWsl(profile: WslProfile): GlobalExecutionTarget {
  return { kind: "wsl", id: profile.id, label: `WSL · ${profile.name}`, root: profile.defaultRoot?.trim() || "/" };
}

export function resolveGlobalEnvironment(
  selection: EnvironmentSelection,
  connection: SshConnection | null,
  progress: SshConnectionProgress | null,
  profile: WslProfile | null,
  wslStatus: WslConnectionState,
  wslError: string,
): { target: GlobalExecutionTarget; error: string } {
  const local: GlobalExecutionTarget = { kind: "local", label: "本机工作区" };
  if (selection === "local" || (selection === "auto" && progress?.status !== "connected")) return { target: local, error: "" };
  if (selection === "wsl") {
    return {
      target: profile ? environmentTargetForWsl(profile) : local,
      error: !profile ? "所选 WSL 配置不存在，请在菜单栏重新选择执行环境。"
        : wslStatus === "connected" ? "" : wslStatus === "connecting" ? "WSL 正在连接，请稍后发送。"
          : wslError || "WSL 尚未连接，请在菜单栏连接后重试。",
    };
  }
  return {
    target: connection ? environmentTargetForSsh(connection) : local,
    error: !connection ? "请在菜单栏选择并连接 SSH 主机。"
      : progress?.status === "connected" ? "" : progress?.status === "connecting" ? "SSH 正在连接，请稍后发送。"
        : progress?.error || "SSH 已断开，请在菜单栏重新连接。",
  };
}
