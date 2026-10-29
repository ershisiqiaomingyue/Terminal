import { getSshConnectionProgress, type SshConnectionProgress } from "./api";
import { invoke } from "./runtime";

const POLL_INTERVAL_MS = 400;

/**
 * 在按需读取远程文件（文件预览）前，确保该 SSH 主机已连上。
 *
 * 刻意不用 `startSshConnection`：那个命令会把菜单栏的「全局执行环境」改成这台
 * 主机，而预览只是临时读一个文件，不该动用户全局选择。这里调 `connect_ssh_host_command`
 * （只建立传输通道），已连接时直接返回。
 */
export async function ensureSshHostConnected(connectionId: string, timeoutMs = 30_000): Promise<SshConnectionProgress> {
  const id = connectionId.trim();
  if (!id) throw new Error("预览目标缺少 SSH 连接 ID。");
  const current = await getSshConnectionProgress(id);
  if (current.status === "connected") return current;
  const started = await invoke<SshConnectionProgress>("connect_ssh_host_command", { id, path: null });
  if (started.status === "connected") return started;
  const deadline = Date.now() + timeoutMs;
  let latest = started;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
    latest = await getSshConnectionProgress(id);
    if (latest.status === "connected") return latest;
    if (latest.status === "failed") {
      throw new Error(latest.error || latest.message || "SSH 连接失败。");
    }
  }
  throw new Error(`SSH 连接超时（已等待 ${Math.round(timeoutMs / 1000)} 秒）。`);
}
