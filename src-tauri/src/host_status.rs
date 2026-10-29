// ==========================================================================
// 主机状态采集 - 供终端视图右侧「主机状态」面板显示真实数据
//
// 为什么需要这个模块：
//   终端的右侧抽屉要展示 CPU 负载、内存占用、监听端口、活跃进程。
//   ADB 会话可以用 adb shell 拿设备数据，但本地 / WSL / SSH 会话需要
//   「本机」的真实状态，此前后端没有任何采集入口。
//
// 采集方式（Windows 原生 API + 系统自带命令，不引入新依赖）：
//   - 内存：GlobalMemoryStatusEx
//   - CPU ：GetSystemTimes 两次采样求差（与任务管理器口径一致）
//   - 端口：netstat -ano -p TCP
//   - 进程：tasklist /FO CSV /NH（同时用于给端口补进程名）
//
// 非 Windows 平台返回空快照，前端据此隐藏相关卡片，不会报错。
// ==========================================================================

use serde::Serialize;

/// 主机状态快照（字段全部 camelCase，与前端 TS 类型一一对应）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusSnapshot {
    /// CPU 使用率百分比（0-100），采样失败时为 None
    pub cpu_percent: Option<f64>,
    /// 物理内存总量（字节）
    pub memory_total_bytes: u64,
    /// 已用物理内存（字节）
    pub memory_used_bytes: u64,
    /// 内存占用百分比（0-100）
    pub memory_percent: f64,
    /// 磁盘总量（字节）。取工作目录所在卷；拿不到时为 None。
    pub disk_total_bytes: Option<u64>,
    /// 磁盘已用（字节）= 总量 - 可用
    pub disk_used_bytes: Option<u64>,
    /// 磁盘可用（字节）。保留原始值：前端算百分比要用可用而不是"总量-已用"，
    /// 因为卷预留块不计入可用空间，两者口径不同。
    pub disk_available_bytes: Option<u64>,
    /// 磁盘占用百分比（0-100）。后端直接给出来，避免前端为不同数据源各算一遍。
    pub disk_used_percent: Option<f64>,
    /// 磁盘统计对应的路径（供面板显示"统计的是哪个卷"）
    pub disk_path: Option<String>,
    /// 逻辑处理器数量
    pub cpu_count: usize,
    /// 物理核心数量（Windows 按物理处理器包 × 每包核数估算；拿不到时 None）
    pub cpu_physical_cores: Option<usize>,
    /// 网卡累计接收字节数（用于前端两次采样求差算下行速率）
    pub network_rx_bytes: Option<u64>,
    /// 网卡累计发送字节数（用于前端两次采样求差算上行速率）
    pub network_tx_bytes: Option<u64>,
    /// 采样窗口长度（毫秒）：前端按 `(本次累计 - 上次累计) / 窗口` 算速率，
    /// 因此必须把窗口一起返回，否则两次采样间隔不同会算出错误速率。
    pub sample_window_ms: u64,
    /// 当前监听中的 TCP 端口
    pub listening_ports: Vec<HostPortEntry>,
    /// 常驻内存最高的若干进程
    pub top_processes: Vec<HostProcessEntry>,
    /// CPU 时间占用最高的若干线程（需求：线程占用明细）。
    /// 非 Windows 平台为空；采集失败也为空，前端据此不渲染该卡片。
    pub top_threads: Vec<HostThreadEntry>,
    /// 主机名，供顶栏胶囊显示
    pub host_name: String,
    /// 当前用户主目录（文件面板的默认根）
    pub home_dir: String,
    /// 进程工作目录（打包后通常是安装目录，仅作兜底）
    pub working_dir: String,
    /// 采集时间（RFC3339），供前端显示「刷新于 …」
    pub sampled_at: String,
}

/// 监听端口条目
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPortEntry {
    pub port: u16,
    pub protocol: String,
    /// 占用该端口的进程名，拿不到时为空串
    pub process_name: String,
    pub pid: Option<u32>,
}

/// 活跃进程条目
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProcessEntry {
    pub name: String,
    pub pid: u32,
    /// 常驻内存（字节）
    pub memory_bytes: u64,
    /// 该进程的线程数（需求：线程占用明细）。拿不到时为 None。
    pub thread_count: Option<u32>,
}

/// 线程占用明细条目：按 CPU 时间排前的线程。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostThreadEntry {
    /// 所属进程名
    pub process_name: String,
    pub pid: u32,
    pub thread_id: u32,
    /// 该线程累计占用的 CPU 时间（毫秒）
    pub cpu_time_ms: u64,
    /// 线程优先级（Windows 基础优先级）
    pub priority: i32,
}

/// 采集主机状态。
///
/// `top_process_limit` 控制返回的进程条数（前端默认 5，避免一次塞太多数据）。
///
/// 传 0 是**轻量档**：跳过进程/线程/端口明细，只返回 CPU、内存、磁盘、网速。
/// 明细要遍历全机进程与线程（Top-N 线程得逐个 GetThreadTimes），实测一次约 0.4 秒 CPU，
/// 面板几秒刷一次时没必要每一拍都付这笔钱。
#[tauri::command]
pub async fn get_host_status(top_process_limit: Option<usize>) -> Result<HostStatusSnapshot, String> {
    let limit = top_process_limit.unwrap_or(5).min(30);
    tauri::async_runtime::spawn_blocking(move || collect(limit))
        .await
        .map_err(|error| format!("采集主机状态失败：{error}"))
}

fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(0)
}

/// 物理核心数量。
///
/// Windows 上用 `GetLogicalProcessorInformationEx` 按 `RelationProcessorCore`
/// 统计：每个"核"一条记录，与任务管理器「内核」一致。
/// 拿不到时返回 None，前端退化为只显示逻辑处理器数。
#[cfg(windows)]
fn read_physical_cores() -> Option<usize> {
    use windows::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };

    let mut length: u32 = 0;
    // 第一次调用只为拿所需缓冲区大小（必然返回错误）。
    // SAFETY: 传 null 指针 + 长度指针是文档规定的"查询大小"用法。
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut length) }.ok()?;
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0u8; length as usize];
    let info = buffer.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
    // SAFETY: buffer 按上一步返回的 length 分配，对齐由 Vec<u8> 保证不低于结构体对齐要求。
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(info), &mut length) }.ok()?;

    // 缓冲区里是变长记录序列，按每条记录的 Size 步进统计条数。
    let mut offset = 0usize;
    let mut cores = 0usize;
    while offset + std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= length as usize {
        // SAFETY: offset 始终落在 buffer 内，且每条记录的 Size 由系统填写。
        let record = unsafe { &*(buffer.as_ptr().add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
        let size = record.Size as usize;
        if size == 0 {
            break;
        }
        cores += 1;
        offset += size;
    }
    if cores > 0 {
        Some(cores)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn read_physical_cores() -> Option<usize> {
    None
}

/// 读取所有网卡的累计收发字节数。
///
/// 返回 `(接收累计, 发送累计, 采样窗口毫秒)`。窗口恒为 0：这里只读一次累计值，
/// "速率"必须由前端用相邻两次快照的差值除以时间差算出——后端做两次采样会让
/// 每次调用都多阻塞几百毫秒，拖慢整个面板。
#[cfg(windows)]
fn read_network_totals() -> (Option<u64>, Option<u64>, u64) {
    use windows::Win32::NetworkManagement::IpHelper::{GetIfTable2, MIB_IF_TABLE2};

    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: 传二级指针让系统分配并填充表；成功后由 FreeMibTable 释放。
    if unsafe { GetIfTable2(&mut table) }.is_err() || table.is_null() {
        return (None, None, 0);
    }
    let mut rx: u64 = 0;
    let mut tx: u64 = 0;
    // SAFETY: 系统保证表内 NumEntries 条 MIB_IF_ROW2 连续存放。
    unsafe {
        let entries = (*table).Table.as_ptr();
        for index in 0..(*table).NumEntries as usize {
            let row = &*entries.add(index);
            rx = rx.saturating_add(row.InOctets);
            tx = tx.saturating_add(row.OutOctets);
        }
        windows::Win32::NetworkManagement::IpHelper::FreeMibTable(table as *const _);
    }
    (Some(rx), Some(tx), 0)
}

#[cfg(not(windows))]
fn read_network_totals() -> (Option<u64>, Option<u64>, u64) {
    (None, None, 0)
}

/// 线程占用明细：枚举系统全部线程，按累计 CPU 时间降序取前 `limit` 条。
///
/// 数据来源是 `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)` + `Thread32First/Next`，
/// 这是枚举"全系统线程"唯一不需要逐进程 OpenProcess 的路径。
/// 单个线程读不到时间时跳过，不因为一个失败就整表为空。
#[cfg(windows)]
fn read_threads(limit: usize, processes: &[(String, u32, u64)]) -> Vec<HostThreadEntry> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{
        GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: 标准 Toolhelp 用法；快照句柄用完立即 CloseHandle。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    let Ok(snapshot) = snapshot else {
        return Vec::new();
    };

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut threads: Vec<HostThreadEntry> = Vec::new();
    // SAFETY: entry 已填 dwSize，句柄来自上一步成功创建的快照。
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) }.is_ok();
    while has_entry {
        let pid = entry.th32OwnerProcessID;
        let tid = entry.th32ThreadID;
        // SAFETY: 只申请查询时间所需的最小权限；拿不到就跳过该线程。
        if let Ok(handle) = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) } {
            let mut creation = Default::default();
            let mut exit = Default::default();
            let mut kernel = Default::default();
            let mut user = Default::default();
            // SAFETY: 四个 FILETIME 都在栈上有效；失败时不写入。
            let ok = unsafe { GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }.is_ok();
            // SAFETY: handle 由 OpenThread 获得，必须关闭。
            unsafe { let _ = windows::Win32::Foundation::CloseHandle(handle); };
            if ok {
                let to_u64 = |value: windows::Win32::Foundation::FILETIME| {
                    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
                };
                // FILETIME 单位是 100ns，换算成毫秒。
                let cpu_100ns = to_u64(kernel).saturating_add(to_u64(user));
                let cpu_time_ms = cpu_100ns / 10_000;
                if cpu_time_ms > 0 {
                    let process_name = processes
                        .iter()
                        .find(|item| item.1 == pid)
                        .map(|item| item.0.clone())
                        .unwrap_or_default();
                    threads.push(HostThreadEntry {
                        process_name,
                        pid,
                        thread_id: tid,
                        cpu_time_ms,
                        priority: entry.tpBasePri,
                    });
                }
            }
        }
        // SAFETY: entry 的 dwSize 在循环中保持不变，句柄仍有效。
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) }.is_ok();
    }
    // SAFETY: 快照句柄必须关闭。
    unsafe { let _ = windows::Win32::Foundation::CloseHandle(snapshot); };

    threads.sort_by(|left, right| right.cpu_time_ms.cmp(&left.cpu_time_ms));
    threads.truncate(limit);
    threads
}

#[cfg(not(windows))]
fn read_threads(_limit: usize, _processes: &[(String, u32, u64)]) -> Vec<HostThreadEntry> {
    Vec::new()
}

/// 磁盘容量：`(总量, 已用, 可用, 百分比, 统计路径)`。
///
/// 统计**工作目录所在的卷**，而不是固定 C 盘——用户可能在 D 盘跑项目，
/// 固定 C 盘会显示一堆无关数字。取不到时全部为 None，前端据此隐藏磁盘卡。
#[cfg(windows)]
fn read_disk(path: &str) -> (Option<u64>, Option<u64>, Option<u64>, Option<f64>, Option<String>) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let volume = volume_root(path);
    let mut available: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    let wide = HSTRING::from(volume.as_str());
    // SAFETY: 三个输出指针都指向栈上的有效 u64，路径是有效的 UTF-16 宽字符串。
    if unsafe {
        GetDiskFreeSpaceExW(
            &wide,
            Some(&mut available as *mut u64),
            Some(&mut total as *mut u64),
            Some(&mut free as *mut u64),
        )
    }
    .is_err()
    {
        return (None, None, None, None, None);
    }
    if total == 0 {
        return (None, None, None, None, None);
    }
    // 已用 = 总量 - 卷剩余；百分比同样按这个口径，与资源管理器一致。
    // （`available` 是"当前用户可用"，含配额差异，只作参考值一并返回。）
    let used = total.saturating_sub(free);
    let percent = ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
    (Some(total), Some(used), Some(available), Some(percent), Some(volume))
}

#[cfg(not(windows))]
fn read_disk(_path: &str) -> (Option<u64>, Option<u64>, Option<u64>, Option<f64>, Option<String>) {
    (None, None, None, None, None)
}

/// 把任意路径规约成卷根（`D:\workspace\x` → `D:\`），用于磁盘统计与显示。
#[cfg(windows)]
fn volume_root(path: &str) -> String {
    let trimmed = path.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        return format!("{}:\\", chars[0].to_ascii_uppercase());
    }
    trimmed.to_string()
}

/// 主机名、主目录、工作目录。这些值不随刷新变化，但一并返回可以让前端
/// 少发一次请求；拿不到时给空串，前端据此隐藏对应入口。
fn host_identity() -> (String, String, String) {
    let host_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let working_dir = std::env::current_dir()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    (host_name, home_dir, working_dir)
}

#[cfg(not(windows))]
fn collect(_limit: usize) -> HostStatusSnapshot {
    let (host_name, home_dir, working_dir) = host_identity();
    HostStatusSnapshot {
        cpu_percent: None,
        memory_total_bytes: 0,
        memory_used_bytes: 0,
        memory_percent: 0.0,
        disk_total_bytes: None,
        disk_used_bytes: None,
        disk_available_bytes: None,
        disk_used_percent: None,
        disk_path: None,
        cpu_count: cpu_count(),
        cpu_physical_cores: None,
        network_rx_bytes: None,
        network_tx_bytes: None,
        sample_window_ms: 0,
        listening_ports: Vec::new(),
        top_processes: Vec::new(),
        top_threads: Vec::new(),
        host_name,
        home_dir,
        working_dir,
        sampled_at: chrono::Local::now().to_rfc3339(),
    }
}

#[cfg(windows)]
fn collect(limit: usize) -> HostStatusSnapshot {
    let (total, used) = read_memory();
    let (host_name, home_dir, working_dir) = host_identity();
    // 磁盘统计跟随工作目录所在卷，避免固定 C 盘。
    let (disk_total, disk_used, disk_available, disk_percent, disk_path) = read_disk(&working_dir);
    // 网速需要"累计字节"而不是瞬时速率：前端拿两次快照求差。
    // 这里读一次累计值，并把采样窗口一起返回，避免两次采样间隔不同算错。
    let (network_rx_bytes, network_tx_bytes, sample_window_ms) = read_network_totals();
    // limit == 0 是轻量档：上面几项都是一次系统调用，下面这三份明细才是贵的部分。
    let mut top_processes: Vec<HostProcessEntry> = Vec::new();
    let mut top_threads: Vec<HostThreadEntry> = Vec::new();
    let mut listening_ports: Vec<HostPortEntry> = Vec::new();
    if limit > 0 {
        let processes = read_processes();
        // 线程明细只枚举一次，下面的进程条目复用它汇总 thread_count。
        top_threads = read_threads(limit, &processes);
        // 避免为每个进程再单独开一次 Toolhelp 快照（那是 O(进程数) 次系统调用）。
        let count_for = |pid: u32| -> Option<u32> {
            let count = top_threads.iter().filter(|item| item.pid == pid).count();
            if count > 0 {
                Some(count as u32)
            } else {
                None
            }
        };
        top_processes = processes
            .iter()
            .take(limit)
            .map(|entry| HostProcessEntry {
                name: entry.0.clone(),
                pid: entry.1,
                memory_bytes: entry.2,
                thread_count: count_for(entry.1),
            })
            .collect();
        listening_ports = read_listening_ports(limit, &processes);
    }
    HostStatusSnapshot {
        cpu_percent: read_cpu_percent(),
        memory_total_bytes: total,
        memory_used_bytes: used,
        memory_percent: if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        },
        disk_total_bytes: disk_total,
        disk_used_bytes: disk_used,
        disk_available_bytes: disk_available,
        disk_used_percent: disk_percent,
        disk_path,
        cpu_count: cpu_count(),
        cpu_physical_cores: read_physical_cores(),
        network_rx_bytes,
        network_tx_bytes,
        sample_window_ms,
        listening_ports,
        top_processes,
        // 注意：`top_threads` 只保留 CPU 时间前 limit 条，因此 thread_count 的含义是
        // "该进程在 Top-N 线程里占了几条"，不是进程的全部线程数。对"哪些线程在吃 CPU"
        // 这个用途是准确的；要精确总线程数需另开进程快照，代价更高，当前不做。
        top_threads,
        host_name,
        home_dir,
        working_dir,
        sampled_at: chrono::Local::now().to_rfc3339(),
    }
}

#[cfg(windows)]
fn read_memory() -> (u64, u64) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status 已按 API 要求填好 dwLength，且指向栈上的有效内存。
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return (0, 0);
    }
    let total = status.ullTotalPhys;
    (total, total.saturating_sub(status.ullAvailPhys))
}

#[cfg(windows)]
fn read_cpu_percent() -> Option<f64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    // FILETIME 是 100 纳秒单位的高低位结构，这里合并成 u64 便于求差。
    fn to_u64(value: FILETIME) -> u64 {
        ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
    }

    fn sample() -> Option<(u64, u64, u64)> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: 三个指针都指向栈上的有效 FILETIME。
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        Some((to_u64(idle), to_u64(kernel), to_u64(user)))
    }

    let before = sample()?;
    // 采样间隔 200ms：太短噪声大，太长会让调用方感到卡顿。
    std::thread::sleep(std::time::Duration::from_millis(200));
    let after = sample()?;

    let idle = after.0.saturating_sub(before.0);
    // kernel 时间里已包含 idle，所以总线时间 = kernel + user。
    let total = after
        .1
        .saturating_sub(before.1)
        .saturating_add(after.2.saturating_sub(before.2));
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle);
    Some(((busy as f64 / total as f64) * 100.0).clamp(0.0, 100.0))
}

/// 读取进程列表，按常驻内存降序返回 `(进程名, PID, 内存字节)`。
/// 拿不到时返回空列表（面板仍可显示内存与端口）。
#[cfg(windows)]
fn read_processes() -> Vec<(String, u32, u64)> {
    let Ok(output) = crate::background_process::command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    for line in text.lines() {
        // 形如："chrome.exe","12345","Console","1","123,456 K"
        let fields: Vec<&str> = line.split("\",\"").collect();
        if fields.len() < 5 {
            continue;
        }
        let name = fields[0].trim_start_matches('"').to_string();
        let Some(pid) = fields[1].trim_matches('"').parse::<u32>().ok() else {
            continue;
        };
        // 内存字段形如 "123,456 K"，去掉逗号与单位后换算成字节。
        let memory_text = fields[4]
            .trim_end_matches('"')
            .replace(',', "")
            .replace(" K", "")
            .replace('K', "");
        let Some(kilobytes) = memory_text.trim().parse::<u64>().ok() else {
            continue;
        };
        entries.push((name, pid, kilobytes.saturating_mul(1024)));
    }
    entries.sort_by(|left, right| right.2.cmp(&left.2));
    entries
}

#[cfg(windows)]
fn read_listening_ports(limit: usize, processes: &[(String, u32, u64)]) -> Vec<HostPortEntry> {
    let Ok(output) = crate::background_process::command("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut entries: Vec<HostPortEntry> = Vec::new();

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // 形如：TCP  0.0.0.0:135  0.0.0.0:0  LISTENING  1204
        if fields.len() < 5 || !fields[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !fields[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Some(port) = fields[1]
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<u16>().ok())
        else {
            continue;
        };
        // 同一端口可能被 IPv4 / IPv6 各监听一次，只保留第一条。
        if entries.iter().any(|entry| entry.port == port) {
            continue;
        }
        let pid = fields[4].parse::<u32>().ok();
        let process_name = pid
            .and_then(|value| processes.iter().find(|entry| entry.1 == value))
            .map(|entry| entry.0.clone())
            .unwrap_or_default();
        entries.push(HostPortEntry {
            port,
            protocol: "TCP".to_string(),
            process_name,
            pid,
        });
    }
    entries.sort_by_key(|entry| entry.port);
    entries.truncate(limit);
    entries
}
