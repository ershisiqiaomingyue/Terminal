// ==========================================================================
// 主机状态 - 右侧抽屉「主机状态」面板的数据源
//
// 数据来自后端 `host_status::get_host_status`（Windows 原生 API + 系统命令），
// 只有 Tauri 桌面环境才有真实数据；浏览器预览时返回 null，面板显示占位。
// ==========================================================================

import { invoke, isTauri } from "./runtime";

/** 监听端口条目 */
export interface HostPortEntry {
  port: number;
  protocol: string;
  processName: string;
  pid?: number | null;
}

/** 活跃进程条目 */
export interface HostProcessEntry {
  name: string;
  pid: number;
  memoryBytes: number;
  /** 该进程在 Top-N 线程里占的条数（不是进程全部线程数） */
  threadCount?: number | null;
}

/** 线程占用明细条目（按 CPU 时间降序） */
export interface HostThreadEntry {
  processName: string;
  pid: number;
  threadId: number;
  /** 该线程累计占用的 CPU 时间（毫秒） */
  cpuTimeMs: number;
  priority: number;
}

/** 主机状态快照 */
export interface HostStatusSnapshot {
  cpuPercent?: number | null;
  memoryTotalBytes: number;
  memoryUsedBytes: number;
  memoryPercent: number;
  /** 磁盘总量（字节）。统计的是工作目录所在卷，取不到时为空。 */
  diskTotalBytes?: number | null;
  /** 磁盘已用（字节） */
  diskUsedBytes?: number | null;
  /** 磁盘可用（字节） */
  diskAvailableBytes?: number | null;
  /** 磁盘占用百分比（0-100） */
  diskUsedPercent?: number | null;
  /** 磁盘统计对应的卷根，如 `D:\` */
  diskPath?: string | null;
  cpuCount: number;
  /** 物理核心数（Windows 按 RelationProcessorCore 统计；拿不到时为空） */
  cpuPhysicalCores?: number | null;
  /** 网卡累计接收字节（两次快照求差算下行速率） */
  networkRxBytes?: number | null;
  /** 网卡累计发送字节（两次快照求差算上行速率） */
  networkTxBytes?: number | null;
  listeningPorts: HostPortEntry[];
  topProcesses: HostProcessEntry[];
  /** CPU 时间占用最高的线程（需求：线程占用明细） */
  topThreads?: HostThreadEntry[];
  hostName: string;
  homeDir: string;
  workingDir: string;
  sampledAt: string;
}

/**
 * 纯计算部分（字节格式化、网速求差）放在 `hostStatusMetrics.ts`：
 * 本文件依赖 `./runtime`，Node 单测进程解析不了无扩展名导入，
 * 只有拆出去纯函数才能进单测。这里重导出保持既有调用点不变。
 */
export { formatBytes, formatCpuTime, formatRate, networkRates } from "./hostStatusMetrics";

/**
 * 采集主机状态。
 * @param topProcessLimit 返回的进程条数，默认 5。
 */
export async function readHostStatus(topProcessLimit = 5): Promise<HostStatusSnapshot | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<HostStatusSnapshot>("get_host_status", { topProcessLimit });
  } catch {
    // 采集失败（老版本后端、非 Windows）时静默返回 null，面板走占位分支。
    return null;
  }
}
