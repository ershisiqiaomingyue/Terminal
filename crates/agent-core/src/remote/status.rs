use super::{RemoteDiskMount, RemoteProcessStatus, SshRemoteStatus};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Read counters once per existing status request; no extra connection or sampling sleep.
// /proc/stat guest times are already included in user/nice, so sum only columns 2..9.
pub const LINUX_RESOURCE_PROBE: &str = r#"
if [ -r /proc/stat ]; then
  awk '/^cpu / {total=0; for(i=2;i<=9;i++) total+=$i; printf "__AT_CPU_TICKS__ %.0f %.0f\n", total, $5+$6; exit}' /proc/stat
fi
if [ -r /proc/cpuinfo ]; then
  awk -F: '/^physical id/ {socket=$2; have_socket=1} /^core id/ {core=$2; have_core=1} /^$/ {if(have_socket && have_core) cores[socket ":" core]=1; have_socket=0; have_core=0} END {if(have_socket && have_core) cores[socket ":" core]=1; for(c in cores) n++; if(n>0) print "__AT_PHYSICAL_CORES__", n}' /proc/cpuinfo
fi
if [ -r /proc/loadavg ]; then
  awk '{print "__AT_LOAD_AVERAGES__", $1, $2, $3}' /proc/loadavg
fi
# 每核 tick（cpu0..cpuN），与总量同样的差值算法。
if [ -r /proc/stat ]; then
  awk '/^cpu[0-9]/ {total=0; for(i=2;i<=9;i++) total+=$i; print "__AT_CPU_CORE__", substr($1, 4), total, $5+$6}' /proc/stat
fi
# 内存明细：真正的空闲与可回收缓存。
if [ -r /proc/meminfo ]; then
  awk '/^MemFree:/ {print "__AT_MEM_FREE__", $2} /^Buffers:/ {print "__AT_MEM_BUFFERS__", $2} /^Cached:/ {print "__AT_MEM_CACHED__", $2} /^SReclaimable:/ {print "__AT_MEM_SRECLAIM__", $2}' /proc/meminfo
fi
# 网卡累计收发（排除回环）；速率由后端相邻两次采样求差。
if [ -r /proc/net/dev ]; then
  awk 'NR>2 && $1!="lo:" {sub(/:/, "", $1); rx+=$2; tx+=$10} END {print "__AT_NET__", rx+0, tx+0}' /proc/net/dev
fi
# 关注的挂载点：/ 与 /data（不存在时 df 静默失败，不输出行）。
df -P -k / 2>/dev/null | awk 'NR==2 {gsub(/%/, "", $5); print "__AT_DF__", "/", $2*1024, $3*1024, $4*1024, $5}'
df -P -k /data 2>/dev/null | awk 'NR==2 {gsub(/%/, "", $5); print "__AT_DF__", "/data", $2*1024, $3*1024, $4*1024, $5}'
# CPU TOP 进程（GNU ps；busybox/受限环境失败则整段静默跳过）。
ps -eo pid,comm,pcpu,pmem --sort=-pcpu 2>/dev/null | awk 'NR>1 && NR<=6 {print "__AT_PS__", $1, $2, $3, $4}'
"#;

#[derive(Clone, Copy)]
struct CpuSample {
    total: u64,
    idle: u64,
}

impl CpuSample {
    fn usage_since(self, previous: Self) -> Option<f64> {
        let total = self.total.checked_sub(previous.total)?;
        let idle = self.idle.checked_sub(previous.idle)?;
        (total > 0 && idle <= total).then(|| (total - idle) as f64 / total as f64 * 100.0)
    }
}

struct CpuReading {
    sample: CpuSample,
    at: Instant,
    usage: Option<f64>,
}

fn sample_usage(key: &str, sample: CpuSample) -> Option<f64> {
    static READINGS: OnceLock<Mutex<HashMap<String, CpuReading>>> = OnceLock::new();
    let mut readings = READINGS.get_or_init(Default::default).lock().ok()?;
    if readings.len() >= 512 && !readings.contains_key(key) {
        let oldest = readings
            .iter()
            .min_by_key(|(_, value)| value.at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            readings.remove(&oldest);
        }
    }
    let usage = readings.get(key).and_then(|previous| {
        if previous.at.elapsed() > Duration::from_secs(60) {
            return None;
        }
        if sample.total == previous.sample.total {
            return previous.usage;
        }
        sample.usage_since(previous.sample)
    });
    readings.insert(
        key.to_owned(),
        CpuReading {
            sample,
            at: Instant::now(),
            usage,
        },
    );
    usage
}

/// 网卡计数采样：相邻两次求差得速率（字节/秒）。计数器回绕/清零（新值更小）时
/// 放弃本次速率，等下一轮重新建立基线。
fn sample_net_rate(key: &str, rx: u64, tx: u64) -> Option<(f64, f64)> {
    static NET: OnceLock<Mutex<HashMap<String, (u64, u64, Instant)>>> = OnceLock::new();
    let mut map = NET.get_or_init(Default::default).lock().ok()?;
    let previous = map.insert(key.to_owned(), (rx, tx, Instant::now()));
    let (previous_rx, previous_tx, at) = previous?;
    let elapsed = at.elapsed().as_secs_f64();
    if elapsed <= 0.0 || rx < previous_rx || tx < previous_tx {
        return None;
    }
    Some((
        (rx - previous_rx) as f64 / elapsed,
        (tx - previous_tx) as f64 / elapsed,
    ))
}

pub fn enrich_linux_resource_status(status: &mut SshRemoteStatus, output: &str, sample_key: &str) {
    // 缓存三件（Buffers/Cached/SReclaimable）先累加，循环结束后一次写入。
    let mut cached_kb: u64 = 0;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("__AT_CPU_TICKS__") => {
                let total = parts.next().and_then(|value| value.parse().ok());
                let idle = parts.next().and_then(|value| value.parse().ok());
                if let (Some(total), Some(idle)) = (total, idle) {
                    status.cpu_usage_percent = sample_usage(sample_key, CpuSample { total, idle });
                }
            }
            Some("__AT_CPU_CORE__") => {
                let index = parts.next().and_then(|value| value.parse::<usize>().ok());
                let total = parts.next().and_then(|value| value.parse::<u64>().ok());
                let idle = parts.next().and_then(|value| value.parse::<u64>().ok());
                if let (Some(index), Some(total), Some(idle)) = (index, total, idle) {
                    // 首轮没有差值也要登记核心槽位（NaN 占位）：前端第一拍就知道
                    // 有几个核，第二拍开始填使用率。
                    let cores = status.cpu_cores.get_or_insert_with(Vec::new);
                    if cores.len() <= index {
                        cores.resize(index + 1, f64::NAN);
                    }
                    let key = format!("{sample_key}#c{index}");
                    if let Some(usage) = sample_usage(&key, CpuSample { total, idle }) {
                        cores[index] = usage;
                    }
                }
            }
            Some("__AT_PHYSICAL_CORES__") => {
                status.cpu_physical_cores = parts
                    .next()
                    .and_then(|value| value.parse().ok())
                    .filter(|value| *value > 0);
            }
            Some("__AT_LOAD_AVERAGES__") => {
                let mut next_load = || {
                    parts
                        .next()
                        .and_then(|value| value.parse::<f64>().ok())
                        .filter(|value| value.is_finite() && *value >= 0.0)
                };
                status.load_average = next_load();
                status.load_average_5 = next_load();
                status.load_average_15 = next_load();
            }
            Some("__AT_MEM_FREE__") => {
                if let Some(kb) = parts.next().and_then(|value| value.parse::<u64>().ok()) {
                    status.memory_free_bytes = Some(kb.saturating_mul(1024));
                }
            }
            Some("__AT_MEM_BUFFERS__")
            | Some("__AT_MEM_CACHED__")
            | Some("__AT_MEM_SRECLAIM__") => {
                if let Some(kb) = parts.next().and_then(|value| value.parse::<u64>().ok()) {
                    cached_kb = cached_kb.saturating_add(kb);
                }
            }
            Some("__AT_NET__") => {
                let rx = parts.next().and_then(|value| value.parse::<u64>().ok());
                let tx = parts.next().and_then(|value| value.parse::<u64>().ok());
                if let (Some(rx), Some(tx)) = (rx, tx) {
                    status.net_rx_bytes = Some(rx);
                    status.net_tx_bytes = Some(tx);
                    if let Some((rx_rate, tx_rate)) = sample_net_rate(sample_key, rx, tx) {
                        status.net_rx_rate_bps = Some(rx_rate);
                        status.net_tx_rate_bps = Some(tx_rate);
                    }
                }
            }
            Some("__AT_DF__") => {
                let mount = parts.next().map(str::to_string);
                let total = parts.next().and_then(|value| value.parse::<u64>().ok());
                let used = parts.next().and_then(|value| value.parse::<u64>().ok());
                let available = parts.next().and_then(|value| value.parse::<u64>().ok());
                let percent = parts.next().and_then(|value| value.parse::<f64>().ok());
                if let (Some(mount), Some(total), Some(used), Some(available)) =
                    (mount, total, used, available)
                {
                    let used_percent = percent.filter(|value| (0.0..=100.0).contains(value));
                    if !status.disks.iter().any(|disk| disk.mount == mount) {
                        status.disks.push(RemoteDiskMount {
                            mount,
                            total_bytes: total,
                            used_bytes: used,
                            available_bytes: available,
                            used_percent: used_percent
                                .unwrap_or((used as f64 / total.max(1) as f64) * 100.0),
                        });
                    }
                }
            }
            Some("__AT_PS__") => {
                let pid = parts.next().and_then(|value| value.parse::<u64>().ok());
                let name = parts.next().map(str::to_string);
                let cpu = parts.next().and_then(|value| value.parse::<f64>().ok());
                let memory = parts.next().and_then(|value| value.parse::<f64>().ok());
                if let (Some(pid), Some(name), Some(cpu), Some(memory)) = (pid, name, cpu, memory) {
                    if status.top_processes.len() < 6 {
                        status.top_processes.push(RemoteProcessStatus {
                            pid,
                            name,
                            cpu_percent: cpu,
                            memory_percent: memory,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    if cached_kb > 0 {
        status.memory_cached_bytes = Some(cached_kb.saturating_mul(1024));
    }
}

pub fn parse_wsl_resource_status(
    text: &str,
    latency_ms: u64,
    checked_path: String,
    sample_key: &str,
) -> SshRemoteStatus {
    let mut hostname = None;
    let mut platform = None;
    let mut cpu_count = None;
    let mut memory_total_bytes = None;
    let mut memory_available_bytes = None;
    let mut disk_total_bytes = None;
    let mut disk_available_bytes = None;
    let mut disk_used_percent = None;
    let mut disk_used_bytes = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("__AT_HOST__") => hostname = parts.next().map(str::to_string),
            Some("__AT_PLATFORM__") => platform = parts.next().map(str::to_string),
            Some("__AT_CPU__") => cpu_count = parts.next().and_then(|value| value.parse().ok()),
            Some("__AT_MEM__") => {
                memory_total_bytes = parts.next().and_then(|value| value.parse().ok());
                memory_available_bytes = parts.next().and_then(|value| value.parse().ok());
            }
            Some("__AT_DISK__") => {
                disk_total_bytes = parts.next().and_then(|value| value.parse().ok());
                disk_available_bytes = parts.next().and_then(|value| value.parse().ok());
                disk_used_percent = parts.next().and_then(|value| value.parse().ok());
                disk_used_bytes = parts.next().and_then(|value| value.parse().ok());
            }
            _ => {}
        }
    }
    let mut status = SshRemoteStatus {
        latency_ms,
        hostname,
        platform,
        cpu_count,
        load_average: None,
        memory_total_bytes,
        memory_available_bytes,
        disk_total_bytes,
        disk_available_bytes,
        disk_used_percent,
        disk_used_bytes,
        checked_path,
        ..Default::default()
    };
    enrich_linux_resource_status(&mut status, text, sample_key);
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wsl_status_fields_without_inventing_missing_values() {
        let status = parse_wsl_resource_status(
            "__AT_HOST__ ubuntu\n__AT_PLATFORM__ Linux\n__AT_CPU__ 8\n__AT_MEM__ 1000 400\n__AT_DISK__ 3298534883328 1099511627776 67 2199023255552\n__AT_PHYSICAL_CORES__ 4\n__AT_LOAD_AVERAGES__ 0.18 0.24 0.30",
            12,
            "/workspace/aosp".to_string(),
            "wsl-status-test",
        );
        assert_eq!(status.hostname.as_deref(), Some("ubuntu"));
        assert_eq!(status.platform.as_deref(), Some("Linux"));
        assert_eq!(status.cpu_count, Some(8));
        assert_eq!(status.memory_total_bytes, Some(1000));
        assert_eq!(status.memory_available_bytes, Some(400));
        assert_eq!(status.disk_used_percent, Some(67.0));
        assert_eq!(status.disk_total_bytes, Some(3298534883328));
        assert_eq!(status.disk_used_bytes, Some(2199023255552));
        assert_eq!(status.cpu_physical_cores, Some(4));
        assert_eq!(status.load_average_5, Some(0.24));
        assert_eq!(status.cpu_usage_percent, None);
        assert_eq!(status.checked_path, "/workspace/aosp");
    }

    #[test]
    fn cpu_usage_uses_counter_deltas_and_rejects_resets() {
        let before = CpuSample {
            total: 1000,
            idle: 600,
        };
        assert_eq!(
            CpuSample {
                total: 1200,
                idle: 750
            }
            .usage_since(before),
            Some(25.0)
        );
        assert_eq!(before.usage_since(before), None);
        assert_eq!(CpuSample { total: 5, idle: 2 }.usage_since(before), None);
        assert_eq!(
            CpuSample {
                total: 1100,
                idle: 850
            }
            .usage_since(before),
            None
        );
    }

    #[test]
    fn linux_metrics_keep_missing_values_and_hosts_separate() {
        let mut first = SshRemoteStatus::default();
        enrich_linux_resource_status(&mut first, "__AT_CPU_TICKS__ 1000 600\n__AT_PHYSICAL_CORES__ 32\n__AT_LOAD_AVERAGES__ 0.18 0.24 0.30", "status-test-a");
        assert_eq!(first.cpu_usage_percent, None);
        assert_eq!(first.cpu_physical_cores, Some(32));
        assert_eq!(first.load_average_15, Some(0.30));
        enrich_linux_resource_status(&mut first, "__AT_CPU_TICKS__ 1200 750", "status-test-a");
        assert_eq!(first.cpu_usage_percent, Some(25.0));
        let mut second = SshRemoteStatus::default();
        enrich_linux_resource_status(
            &mut second,
            "__AT_CPU_TICKS__ 1500 800\n__AT_LOAD_AVERAGES__ NaN ?",
            "status-test-b",
        );
        assert_eq!(second.cpu_usage_percent, None);
        assert_eq!(second.cpu_physical_cores, None);
        assert_eq!(second.load_average, None);
    }
}

#[cfg(test)]
mod ssh_rich_tests {
    use super::*;

    #[test]
    fn enrich_parses_rich_markers() {
        let mut status = SshRemoteStatus::default();
        let output = "\
__AT_CPU_CORE__ 0 100 50
__AT_CPU_CORE__ 1 200 50
__AT_MEM_FREE__ 1000
__AT_MEM_BUFFERS__ 200
__AT_MEM_CACHED__ 300
__AT_MEM_SRECLAIM__ 100
__AT_DF__ / 1000 400 600 40
__AT_DF__ /data 2000 1500 500 75
__AT_PS__ 42 sshd 12.5 1.2
__AT_PS__ 7 build 8.3 0.4
";
        enrich_linux_resource_status(&mut status, output, "test-host");
        // 每核：无历史采样时首轮不出使用率，但核心数量应被登记
        let cores = status.cpu_cores.expect("cores registered");
        assert_eq!(cores.len(), 2);
        // 内存：空闲与缓存（Buffers+Cached+SReclaimable = 600 KB）
        assert_eq!(status.memory_free_bytes, Some(1000 * 1024));
        assert_eq!(status.memory_cached_bytes, Some(600 * 1024));
        // 挂载点：/ 与 /data 各一条
        assert_eq!(status.disks.len(), 2);
        assert_eq!(status.disks[0].mount, "/");
        assert_eq!(status.disks[0].used_percent, 40.0);
        assert_eq!(status.disks[1].mount, "/data");
        // 进程排名
        assert_eq!(status.top_processes.len(), 2);
        assert_eq!(status.top_processes[0].name, "sshd");
        assert_eq!(status.top_processes[0].pid, 42);
    }

    #[test]
    fn net_rate_rejects_counter_reset() {
        assert!(sample_net_rate("net-test", 1000, 500).is_none());
        // 计数回绕（新值更小）不应产生负速率
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(sample_net_rate("net-test", 900, 400).is_none());
    }
}
