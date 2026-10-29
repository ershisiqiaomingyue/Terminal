import type { ReactNode } from "react";
import { Activity, Cpu, HardDrive, MemoryStick } from "lucide-react";
import type { SystemStatusMetrics } from "./systemStatusMetrics";

function Metric({ icon, label, value, secondary, annotation, percent, title, compact = false }: {
  icon: ReactNode; label: string; value: string; secondary?: string; annotation?: string;
  percent?: number | null; title: string; compact?: boolean;
}) {
  const tone = percent == null ? "muted" : percent >= 90 ? "danger" : percent >= 75 ? "warning" : "healthy";
  return <div className={`system-environment-metric is-${tone}${compact ? " is-compact" : ""}`} title={title}>
    <div className="system-environment-metric-heading">
      <span className="system-environment-metric-label">{icon}{label}</span>
      {annotation && <small>{annotation}</small>}
    </div>
    <b>{value}</b>
    {secondary && <span className="system-environment-metric-secondary">{secondary}</span>}
    {!compact && <em aria-hidden="true">{percent != null && <i style={{ width: `${percent}%` }} />}</em>}
  </div>;
}

export function SystemStatusResources({ metrics: { memory, disk, cpu, load } }: { metrics: SystemStatusMetrics }) {
  return <div className="sidebar-system-status-grid">
    <Metric icon={<MemoryStick size={13} />} label="内存" annotation={memory.usage} value={`已用 ${memory.used}`} secondary={`共 ${memory.total}`} percent={memory.percent}
      title={`内存：已用 ${memory.used}，共 ${memory.total}，占用 ${memory.usage}（按总量减可用量计算）`} />
    <Metric icon={<HardDrive size={13} />} label="磁盘" annotation={disk.usage} value={`已用 ${disk.used}`} secondary={`共 ${disk.total}`} percent={disk.percent}
      title={`工作目录所在磁盘${disk.path ? `（${disk.path}）` : ""}：已用 ${disk.used}，共 ${disk.total}，占用 ${disk.usage}`} />
    <Metric compact icon={<Cpu size={13} />} label="CPU" annotation={cpu.usage} value={cpu.topology} percent={cpu.percent}
      title={`CPU：${cpu.topology}；使用率 ${cpu.usage}。使用率按两次采样之间的计数差计算，首次采样显示 -。`} />
    <Metric compact icon={<Activity size={13} />} label="负载" annotation="1 / 5 / 15分" value={load.value}
      title={`平均负载（运行中及等待资源的任务数）：${load.description}。此值不是 CPU 使用率。`} />
  </div>;
}
