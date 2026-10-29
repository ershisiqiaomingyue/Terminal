/**
 * 主机状态的纯计算部分：网速与格式化。
 *
 * 为什么单独成文件：`hostStatus.ts` 依赖 `./runtime`（Tauri invoke），
 * Node 单测进程解析不了无扩展名导入，所以纯函数必须隔离出来才能进单测。
 * 这与 `terminalScroll.ts` / `terminalReset.ts` 的拆分理由一致。
 */
import type { HostStatusSnapshot } from "./hostStatus";

/** 把字节数格式化成人类可读文本，例如 `9.2 GB`。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // 小于 10 的保留一位小数，其余取整，避免 `9.2431 GB` 这种噪声。
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** 把字节/秒格式化成 `1.2 MB/s`。 */
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

/** 把 CPU 累计时间格式化成 `12 m` / `2 h 05 m`（只到分钟，数字与单位间细空格）。 */
export function formatCpuTime(milliseconds: number): string {
  // 只显示分钟/小时（秒级跳动造成视觉抖动）；不满 1 小时不出现 h；
  // 数字与单位之间用一个细空格（U+2009）做轻微分割。
  const thin = String.fromCharCode(0x2009);
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) return `0${thin}m`;
  const minutes = Math.max(1, Math.round(milliseconds / 1000 / 60));
  if (minutes < 60) return `${minutes}${thin}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}${thin}h${thin}${String(minutes % 60).padStart(2, "0")}${thin}m`;
}

/**
 * 由相邻两次快照算出网速（字节/秒）。
 *
 * 为什么要在前端算：后端只返回**累计字节**，速率必须除以两次采样的真实间隔。
 * 用固定间隔假设会算错——刷新周期会因面板开关、页面可见性而变化。
 * 任一快照缺字段、或时间差非正、或计数回绕（负值）时返回 null。
 */
export function networkRates(
  previous: HostStatusSnapshot | null,
  current: HostStatusSnapshot | null,
  elapsedMs: number,
): { down: number; up: number } | null {
  if (!previous || !current || elapsedMs <= 0) return null;
  const prevRx = previous.networkRxBytes;
  const prevTx = previous.networkTxBytes;
  const curRx = current.networkRxBytes;
  const curTx = current.networkTxBytes;
  if (prevRx == null || prevTx == null || curRx == null || curTx == null) return null;
  // 计数器回绕或网卡重置时差值为负，此时本轮不给速率，等下一轮恢复。
  const rxDelta = curRx - prevRx;
  const txDelta = curTx - prevTx;
  if (rxDelta < 0 || txDelta < 0) return null;
  const seconds = elapsedMs / 1000;
  return { down: rxDelta / seconds, up: txDelta / seconds };
}
