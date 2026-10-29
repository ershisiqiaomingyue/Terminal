type RemoteStatus = Record<string, unknown> | null;

function metric(status: RemoteStatus, ...fields: string[]) {
  for (const field of fields) {
    const value = status?.[field];
    const parsed = typeof value === "number" ? value : typeof value === "string" && value.trim() ? Number(value) : Number.NaN;
    if (Number.isFinite(parsed) && parsed >= 0) return parsed;
  }
  return null;
}

function bytes(value: number | null) {
  if (value == null) return "-";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value.toLocaleString("zh-CN", { maximumFractionDigits: 1 })} ${units[unit]}`;
}

function percentage(value: number | null) {
  return value == null ? "-" : `${value.toLocaleString("zh-CN", { maximumFractionDigits: 1 })}%`;
}

function validPercent(value: number | null) {
  return value != null && value <= 100 ? value : null;
}

export function systemStatusMetrics(status: RemoteStatus) {
  const memoryTotal = metric(status, "memoryTotalBytes", "memory_total_bytes");
  const memoryAvailable = metric(status, "memoryAvailableBytes", "memory_available_bytes");
  const memoryUsed = memoryTotal != null && memoryTotal > 0 && memoryAvailable != null && memoryAvailable <= memoryTotal ? memoryTotal - memoryAvailable : null;
  const memoryPercent = memoryUsed != null && memoryTotal ? memoryUsed / memoryTotal * 100 : null;
  const diskTotal = metric(status, "diskTotalBytes", "disk_total_bytes");
  const used = metric(status, "diskUsedBytes", "disk_used_bytes");
  const diskUsed = used != null && (diskTotal == null || used <= diskTotal) ? used : null;
  // df's available space excludes reserved blocks. total - available is not actual used space.
  const diskPercent = validPercent(metric(status, "diskUsedPercent", "disk_used_percent"));
  const threads = metric(status, "cpuCount", "cpu_count");
  const cores = metric(status, "cpuPhysicalCores", "cpu_physical_cores");
  const cpuPercent = validPercent(metric(status, "cpuUsagePercent", "cpu_usage_percent"));
  const loads = [metric(status, "loadAverage", "load_average"), metric(status, "loadAverage5", "load_average_5"), metric(status, "loadAverage15", "load_average_15")];
  const latency = metric(status, "latencyMs", "latency_ms");
  return {
    memory: { used: bytes(memoryUsed), total: bytes(memoryTotal), percent: memoryPercent, usage: percentage(memoryPercent) },
    disk: { used: bytes(diskUsed), total: bytes(diskTotal), percent: diskPercent, usage: percentage(diskPercent), path: typeof status?.checkedPath === "string" ? status.checkedPath : "" },
    cpu: { topology: [cores != null && cores > 0 ? `${cores} 核` : "", threads != null && threads > 0 ? `${threads} 线程` : ""].filter(Boolean).join(" · ") || "-", percent: cpuPercent, usage: percentage(cpuPercent) },
    // Windows 没有 load average 概念（三项全空）：value 归一成 "-"，消费方按 "-" 整行隐藏。
    // 有值时 value 是 "0.5 / 0.3 / 0.2"，description 只说轴含义，不再把三个数重复一遍。
    load: {
      value: loads.every((value) => value == null) ? "-" : loads.map(value => value == null ? "-" : value.toLocaleString("zh-CN", { maximumFractionDigits: 2 })).join(" / "),
      description: loads.every((value) => value == null) ? "" : "1 / 5 / 15 分钟平均",
    },
    latency: latency == null ? null : `${Math.round(latency)} ms`,
  };
}

export type SystemStatusMetrics = ReturnType<typeof systemStatusMetrics>;
