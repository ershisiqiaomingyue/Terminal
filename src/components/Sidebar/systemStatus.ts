export type RemoteStatus = Record<string, unknown>;
export const RESOURCE_REFRESH_OPEN_MS = 5_000;
export const RESOURCE_REFRESH_CLOSED_MS = 15_000;

function numberValue(status: RemoteStatus | null, ...fields: string[]) {
  for (const field of fields) {
    const value = status?.[field];
    const parsed = typeof value === 'number' ? value : typeof value === 'string' && value.trim() ? Number(value) : NaN;
    if (Number.isFinite(parsed) && parsed >= 0) return parsed;
  }
  return null;
}

function formatBytes(value: number) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let current = Math.max(0, value), unit = 0;
  while (current >= 1024 && unit < units.length - 1) { current /= 1024; unit++; }
  return `${current.toLocaleString('zh-CN', { maximumFractionDigits: current >= 10 ? 0 : 1 })} ${units[unit]}`;
}

export function formatPercent(value: number | null) {
  return value == null ? '未上报' : `${value.toLocaleString('zh-CN', { maximumFractionDigits: 1 })}%`;
}

export function sshDisplayIdentity(connection: { name: string; username: string; host: string; port?: number }) {
  const address = `${connection.username}@${connection.host}${connection.port && connection.port !== 22 ? `:${connection.port}` : ''}`;
  // Preserve custom names while avoiding an identical second line for default names.
  return { name: connection.name.trim() || address, address,
    subtitle: connection.name.trim() && connection.name.trim() !== address ? address : null };
}

export function statusDetails(status: RemoteStatus | null) {
  const memoryTotal = numberValue(status, 'memoryTotalBytes', 'memory_total_bytes');
  const memoryAvailable = numberValue(status, 'memoryAvailableBytes', 'memory_available_bytes');
  const disk = numberValue(status, 'diskUsedPercent', 'disk_used_percent');
  const cpuCount = numberValue(status, 'cpuCount', 'cpu_count');
  const load = numberValue(status, 'loadAverage', 'load_average');
  const latency = numberValue(status, 'latencyMs', 'latency_ms');
  const usedMemory = memoryTotal != null && memoryAvailable != null ? Math.max(0, memoryTotal - memoryAvailable) : null;
  const memoryPercent = usedMemory != null && memoryTotal && memoryTotal > 0 ? usedMemory / memoryTotal * 100 : null;
  const loadPercent = load != null && cpuCount && cpuCount > 0 ? load / cpuCount * 100 : null;
  return {
    memory: usedMemory != null && memoryTotal != null ? `${formatBytes(usedMemory)} / ${formatBytes(memoryTotal)}` : '未上报',
    memoryPercent,
    diskPercent: disk == null ? null : Math.min(100, disk),
    disk: formatPercent(disk == null ? null : Math.min(100, disk)),
    cpu: cpuCount && cpuCount > 0 ? `${cpuCount} 核` : '未上报',
    load: load == null ? '未上报' : load.toLocaleString('zh-CN', { maximumFractionDigits: 2 }),
    loadPercent,
    latency: latency == null ? null : `${Math.round(latency)} ms`,
  };
}
