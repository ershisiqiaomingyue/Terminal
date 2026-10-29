use super::*;

pub fn test_ssh_connection(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    // v3 §21.4 S5a：显式测试 = 手动重连语义——先复位 russh 回退记忆（§21.1），
    // 再按引擎链验证：russh（新连接不吃池缓存，校验的就是当前凭据）→
    // libssh2 → ssh.exe。Transport（已记忆）/Auth 落入既有链继续；
    // 网络类错误直接报错。
    super::russh_runtime::reset_russh_transport_failure(config);
    if super::russh_runtime::russh_engine_active(config) {
        match super::russh_runtime::russh_test_connection(config, password) {
            Ok(()) => return Ok(()),
            Err(error) => match error.kind {
                super::russh_runtime::RusshErrorKind::Transport
                | super::russh_runtime::RusshErrorKind::Auth => {}
                super::russh_runtime::RusshErrorKind::Cancelled
                | super::russh_runtime::RusshErrorKind::Network
                | super::russh_runtime::RusshErrorKind::Other => return Err(error.to_string()),
            },
        }
    }
    if uses_openssh_fallback(config) {
        return verify_openssh_connection(config, password);
    }
    // A connection test is an explicit authentication check. Do not let an
    // already cached session make a changed password look valid.
    let session = match connect_fresh(config, password) {
        Ok(session) => session,
        Err(error)
            if can_retry_with_openssh(config, password) && should_fallback_to_openssh(&error) =>
        {
            remember_openssh_transport(config);
            return verify_openssh_connection(config, password);
        }
        Err(error) => return Err(error),
    };
    session.set_blocking(false);
    let channel = retry_nonblocking("打开 SSH 测试通道", Duration::from_secs(10), || {
        session.channel_session()
    })?;
    let context = SshToolContext::new(
        config.clone(),
        "/",
        password.map(str::to_string),
        Arc::new(AtomicBool::new(false)),
        None,
    );
    let result = execute_remote_command(
        channel,
        &session,
        "printf android-tool-ssh-ok",
        &context,
        None,
        Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
        None,
    )?;
    if result.get("success").and_then(Value::as_bool) != Some(true)
        || result
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            != "android-tool-ssh-ok"
    {
        return Err("SSH 已连接，但远程命令验证失败".to_string());
    }
    Ok(())
}

/// Execute a bounded shell command in a configured SSH workspace. This is
/// intentionally not a general desktop terminal API: callers are expected to
/// provide commands from a typed workflow and to retain the cancellation flag.
pub fn run_ssh_workspace_command(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    command: &str,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<SshCommandResult, String> {
    run_ssh_workspace_command_streaming(
        config,
        password,
        root,
        command,
        cancellation,
        timeout,
        None,
    )
}

pub fn run_ssh_workspace_command_streaming(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    command: &str,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<SshCommandResult, String> {
    run_ssh_workspace_command_with_input(
        config,
        password,
        root,
        command,
        None,
        cancellation,
        timeout,
        sink,
    )
}

pub(super) fn run_ssh_workspace_command_with_input(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    command: &str,
    input: Option<&[u8]>,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<SshCommandResult, String> {
    // 注意：root 可能是哨兵值，**不能**把空串换成 "~"，否则哨兵判定失效。
    let root = root.trim();
    // `shell_escape` 会把路径整体包成字面量，所以 `~`/`$HOME` 这类语法在这里
    // **永远展开不了**（实机 `sh` 报 `can't cd to ~`，10.179.85.15 可复现）。
    // 调用方要的是登录目录时用哨兵值 `LOGIN_DIRECTORY_MARKER` 表达，这里换成
    // 不带参数的 `cd`（sh/bash 无参 `cd` 即回到登录目录）；空 root 同样按登录目录
    // 处理，保持「空 = 家目录」这一既有语义不变。
    let login_dir = root.is_empty() || super::workspace_git::is_login_directory_marker(root);
    let cd_prefix = if login_dir {
        "cd".to_string()
    } else {
        format!("cd {}", shell_escape(root))
    };
    let context = SshToolContext::new(
        config.clone(),
        root,
        password.map(str::to_string),
        cancellation.clone(),
        None,
    );
    let shell_command = format!("{cd_prefix} && {command}");
    // v3 §21.2 S2 + §21.4 S3：russh 主传输（引擎决策唯一出口 russh_engine_active，
    // 泳道键归一）。协商/协议级失败（Transport）已写入记忆，本调用立即降级；
    // 认证失败（Auth）不记忆，本调用降级走 libssh2（还有 agent 认证）；
    // 网络/取消/其余直接报错——换引擎重试只会重复超时或重复认证。
    if super::russh_runtime::russh_engine_active(config) {
        match super::russh_runtime::russh_run_command(
            config,
            password,
            &shell_command,
            input,
            cancellation.clone(),
            timeout,
            sink,
        ) {
            Ok(result) => return Ok(result),
            Err(error) => match error.kind {
                super::russh_runtime::RusshErrorKind::Transport
                | super::russh_runtime::RusshErrorKind::Auth => {}
                super::russh_runtime::RusshErrorKind::Cancelled
                | super::russh_runtime::RusshErrorKind::Network
                | super::russh_runtime::RusshErrorKind::Other => return Err(error.to_string()),
            },
        }
    }
    if uses_openssh_fallback(config) {
        let output = run_openssh_command_with_cancellation(
            config,
            &shell_command,
            input,
            timeout,
            Some(&cancellation),
            password,
            sink,
        )?;
        return Ok(SshCommandResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output_truncated: false,
        });
    }
    let session = connect(config, password)?;
    session.set_blocking(false);
    let channel = retry_nonblocking("打开 SSH 命令通道", timeout, || {
        session.channel_session()
    })?;
    let value = super::command_transport::execute_remote_command_with_input(
        channel,
        &session,
        &shell_command,
        input,
        &context,
        sink,
        timeout,
    )?;
    Ok(SshCommandResult {
        exit_code: value.get("exitCode").and_then(Value::as_i64).unwrap_or(-1) as i32,
        stdout: value
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stderr: value
            .get("stderr")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        output_truncated: value
            .get("outputTruncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Downloads one already-validated workspace-relative file for a typed desktop
/// workflow. This is deliberately narrower than the Agent download tool: it
/// doesn't accept absolute paths and always writes to a caller-owned staging
/// destination.
pub fn download_ssh_workspace_file(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    relative_path: &str,
    local_path: &Path,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<u64, String> {
    let relative_path = relative_path.trim().trim_start_matches('/');
    if relative_path.is_empty()
        || relative_path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("SSH 下载路径必须是工作区内的相对文件路径".to_string());
    }
    // 真标记判定：russh 引擎可用时下载仍走 libssh2 SFTP 通道（传输级流式，
    // 不吃 russh exec 的 8MB 输出上限），仅显式标记的服务器报无 SFTP。
    if is_openssh_transport_marked(config) {
        return Err("当前 SSH 连接未提供可用于 AOSP 产物下载的 SFTP 服务".to_string());
    }
    let session = connect(config, password)?;
    session.set_blocking(false);
    let sftp = retry_nonblocking("初始化 SSH 文件服务", timeout, || session.sftp())?;
    let remote_path = format!("{}/{}", root.trim_end_matches('/'), relative_path);
    let metadata = remote_sftp_lstat(&sftp, &remote_path, &cancellation, "读取 AOSP 产物")?;
    if !matches!(metadata.file_type(), ssh2::FileType::RegularFile) {
        return Err("AOSP 产物不是普通文件".to_string());
    }
    let parent = local_path
        .parent()
        .ok_or_else(|| "本地产物暂存路径无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建本地产物暂存目录：{error}"))?;
    let mut completed = 0;
    let total = metadata.size.unwrap_or(0);
    download_remote_file_to_local(
        &sftp,
        &remote_path,
        local_path,
        false,
        &cancellation,
        None,
        &mut completed,
        total,
    )
}

/// Select a transport before an Agent turn starts. Some servers permit shell
/// commands but deliberately disable SFTP, while some modern SSH endpoints do
/// not negotiate reliably with libssh2. Probe those capabilities once so the
/// first model tool call does not become a sacrificial failed task.
pub fn prepare_ssh_transport(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    super::session::with_connection_operation(config, || {
        prepare_ssh_transport_inner(config, password)
    })
}

fn prepare_ssh_transport_inner(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    // v3 §21.4 S5a：russh 引擎上不存在「SFTP 可用性」问题（文件工具走 shell
    // 通道），探针只验证连接 + exec。走池（russh_run_command 内部 get_or_connect），
    // 首次连接后缓存复用，不付重复握手；失败分类同 test_ssh_connection。
    if super::russh_runtime::russh_engine_active(config) {
        match super::russh_runtime::russh_run_command(
            config,
            password,
            "printf android-tool-ssh-ok",
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            None,
        ) {
            Ok(result) if result.exit_code == 0 => return Ok(()),
            Ok(_) => return Err("SSH 已连接，但远程命令验证失败".to_string()),
            Err(error) => match error.kind {
                super::russh_runtime::RusshErrorKind::Transport
                | super::russh_runtime::RusshErrorKind::Auth => {}
                super::russh_runtime::RusshErrorKind::Cancelled
                | super::russh_runtime::RusshErrorKind::Network
                | super::russh_runtime::RusshErrorKind::Other => return Err(error.to_string()),
            },
        }
    }
    if uses_openssh_fallback(config) {
        return verify_openssh_connection(config, password);
    }
    let session = match connect(config, password) {
        Ok(session) => session,
        Err(error)
            if can_retry_with_openssh(config, password) && should_fallback_to_openssh(&error) =>
        {
            remember_openssh_transport(config);
            return verify_openssh_connection(config, password);
        }
        Err(error) => return Err(error),
    };
    session.set_blocking(false);
    match retry_nonblocking(
        "初始化 SSH 文件服务",
        Duration::from_secs(10),
        || session.sftp(),
    ) {
        Ok(_) => Ok(()),
        Err(error)
            if can_retry_with_openssh(config, password) && should_fallback_to_openssh(&error) =>
        {
            remember_openssh_transport(config);
            verify_openssh_connection(config, password)
        }
        // A non-SFTP-related probe failure should not block command-only
        // servers. The relevant tool will still return its exact error.
        Err(_) => Ok(()),
    }
}

pub(super) fn read_ssh_status_with_openssh(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
) -> Result<String, String> {
    let result = run_openssh_command_with_secret(
        config,
        command,
        None,
        Duration::from_secs(config.operation_timeout_secs.clamp(10, 120)),
        password,
    )?;
    if result.status.success() {
        return Ok(String::from_utf8_lossy(&result.stdout).into_owned());
    }
    let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
    Err(if error.is_empty() {
        "远程状态读取失败，请检查远程工作目录和 SSH 权限".to_string()
    } else {
        format!("远程状态读取失败：{error}")
    })
}

/// Read host status without racing other operations that share the cached
/// libssh2 session. libssh2 sessions are not safe for concurrent channel
/// operations, so status polling must use the same per-connection lane as
/// normal SSH tools.
pub fn query_ssh_status(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
) -> Result<SshRemoteStatus, String> {
    let operation_lock = connection_operation_lock(config);
    let _operation_guard = operation_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = query_ssh_status_inner(config, password, root);
    if result.is_err() {
        // A channel/session transport failure leaves the cached session
        // unusable. Force the next operation to perform a fresh handshake.
        invalidate_cached_connection(config);
    }
    result
}

fn query_ssh_status_inner(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
) -> Result<SshRemoteStatus, String> {
    let started = Instant::now();
    let requested_root = if root.trim().is_empty() {
        "~"
    } else {
        root.trim()
    };
    let checked_path = if is_symbolic_remote_path(requested_root) {
        // query_ssh_status already owns the connection lane. The public
        // resolver would acquire the same non-reentrant mutex a second time.
        resolve_ssh_root_inner(config, password, Some(requested_root))?
    } else {
        normalize_remote_path(requested_root)
    };
    let command = format!(
        "cd {} && printf '__AT_HOST__%s\\n' \"$(hostname 2>/dev/null || true)\"; \
printf '__AT_PLATFORM__%s\\n' \"$(uname -srm 2>/dev/null || true)\"; \
printf '__AT_CPUS__%s\\n' \"$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || true)\"; \
printf '__AT_LOAD__%s\\n' \"$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null | tr -d '{{}}' | awk '{{print $1}}' || true)\"; \
if [ -r /proc/meminfo ]; then awk '/^MemTotal:/ {{print \"__AT_MEM_TOTAL__\" $2}} /^MemAvailable:/ {{print \"__AT_MEM_AVAILABLE__\" $2}}' /proc/meminfo; elif command -v sysctl >/dev/null 2>&1; then printf '__AT_MEM_TOTAL__%s\\n' \"$(sysctl -n hw.memsize 2>/dev/null || true)\"; pagesize=\"$(sysctl -n hw.pagesize 2>/dev/null || echo 4096)\"; vm_stat 2>/dev/null | awk -v page=\"$pagesize\" '/Pages free/ {{gsub(\"\\\\.\", \"\", $3); free=$3}} /Pages inactive/ {{gsub(\"\\\\.\", \"\", $3); inactive=$3}} /Pages speculative/ {{gsub(\"\\\\.\", \"\", $3); speculative=$3}} /Pages purgeable/ {{gsub(\"\\\\.\", \"\", $3); purgeable=$3}} END {{if (free + inactive + speculative + purgeable > 0) print \"__AT_MEM_AVAILABLE__\" ((free + inactive + speculative + purgeable) * page)}}'; fi; \
printf '__AT_DISK__%s\\n' \"$(df -P -k {} 2>/dev/null | awk 'NR==2 {{print $2, $4, $5, $3}}')\"; {}",
        shell_escape(&checked_path),
        shell_escape(&checked_path),
        LINUX_RESOURCE_PROBE,
    );
    // v3 §21.2 S2 + §21.4 S3：状态探针 russh 优先（引擎决策唯一出口）。
    let mut russh_probe: Option<Result<String, String>> = None;
    if super::russh_runtime::russh_engine_active(config) {
        match super::russh_runtime::russh_run_command(
            config,
            password,
            &command,
            None,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(config.operation_timeout_secs.clamp(10, 1800)),
            None,
        ) {
            Ok(result) if result.exit_code == 0 => russh_probe = Some(Ok(result.stdout)),
            Ok(result) => {
                russh_probe = Some(Err(if result.stderr.trim().is_empty() {
                    "远程状态读取失败，请检查远程工作目录和 SSH 权限".to_string()
                } else {
                    format!("远程状态读取失败：{}", result.stderr.trim())
                }))
            }
            Err(error) => match error.kind {
                super::russh_runtime::RusshErrorKind::Transport
                | super::russh_runtime::RusshErrorKind::Auth => {}
                _ => russh_probe = Some(Err(error.to_string())),
            },
        }
    }
    let output = if let Some(probe) = russh_probe {
        probe?
    } else if uses_openssh_fallback(config) {
        read_ssh_status_with_openssh(config, password, &command)?
    } else {
        let session = match connect(config, password) {
            Ok(session) => session,
            Err(error)
                if can_retry_with_openssh(config, password)
                    && should_fallback_to_openssh(&error) =>
            {
                remember_openssh_transport(config);
                return query_ssh_status_inner(config, password, &checked_path);
            }
            Err(error) => return Err(error),
        };
        session.set_blocking(false);
        let channel =
            retry_nonblocking("打开 SSH 状态通道", Duration::from_secs(10), || {
                session.channel_session()
            })?;
        let context = SshToolContext::new(
            config.clone(),
            checked_path.clone(),
            password.map(str::to_string),
            Arc::new(AtomicBool::new(false)),
            None,
        );
        let command_result = execute_remote_command(
            channel,
            &session,
            &command,
            &context,
            None,
            Duration::from_secs(config.operation_timeout_secs.clamp(10, 1800)),
            None,
        )?;
        if command_result.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(command_result
                .get("stderr")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("远程状态读取失败：{}", value.trim()))
                .unwrap_or_else(|| "远程状态读取失败，请检查远程工作目录和 SSH 权限".to_string()));
        }
        command_result
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

    let mut status = SshRemoteStatus {
        latency_ms,
        engine: super::russh_runtime::russh_active_engine_label(config).to_string(),
        hostname: None,
        platform: None,
        cpu_count: None,
        load_average: None,
        memory_total_bytes: None,
        memory_available_bytes: None,
        disk_total_bytes: None,
        disk_available_bytes: None,
        disk_used_percent: None,
        checked_path,
        ..Default::default()
    };
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("__AT_HOST__") {
            status.hostname = (!value.trim().is_empty()).then(|| value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("__AT_PLATFORM__") {
            status.platform = (!value.trim().is_empty()).then(|| value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("__AT_CPUS__") {
            status.cpu_count = value.trim().parse::<u32>().ok().filter(|value| *value > 0);
        } else if let Some(value) = line.strip_prefix("__AT_LOAD__") {
            status.load_average = value.trim().parse::<f64>().ok();
        } else if let Some(value) = line.strip_prefix("__AT_MEM_TOTAL__") {
            status.memory_total_bytes = value.trim().parse::<u64>().ok().map(|value| {
                if status
                    .platform
                    .as_deref()
                    .is_some_and(|platform| platform.contains("Darwin"))
                {
                    value
                } else {
                    value.saturating_mul(1024)
                }
            });
        } else if let Some(value) = line.strip_prefix("__AT_MEM_AVAILABLE__") {
            status.memory_available_bytes = value.trim().parse::<u64>().ok().map(|value| {
                if status
                    .platform
                    .as_deref()
                    .is_some_and(|platform| platform.contains("Darwin"))
                {
                    value
                } else {
                    value.saturating_mul(1024)
                }
            });
        } else if let Some(value) = line.strip_prefix("__AT_MEM_TOTAL_BYTES__") {
            status.memory_total_bytes = value.trim().parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("__AT_MEM_AVAILABLE_BYTES__") {
            status.memory_available_bytes = value.trim().parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("__AT_DISK__") {
            let fields = value.split_whitespace().collect::<Vec<_>>();
            if fields.len() >= 3 {
                status.disk_total_bytes = fields[0]
                    .parse::<u64>()
                    .ok()
                    .map(|kib| kib.saturating_mul(1024));
                status.disk_available_bytes = fields[1]
                    .parse::<u64>()
                    .ok()
                    .map(|kib| kib.saturating_mul(1024));
                status.disk_used_percent = fields[2].trim_end_matches('%').parse::<f64>().ok();
                status.disk_used_bytes = fields
                    .get(3)
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|kib| kib.saturating_mul(1024));
            }
        }
    }
    enrich_linux_resource_status(
        &mut status,
        &output,
        &format!(
            "ssh:{}:{}:{}:{}",
            config.id, config.host, config.port, config.username
        ),
    );
    Ok(status)
}

pub(super) fn default_known_hosts_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.map(PathBuf::from)
        .map(|path| path.join(".ssh").join("known_hosts"))
}

pub(super) fn expand_local_config_path(value: &str) -> PathBuf {
    let value = value.trim();
    let home = {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME");
        home.map(PathBuf::from)
    };
    if let Some(home) = home {
        let home_text = home.to_string_lossy();
        let home_prefixes = ["~", "$HOME", r"${HOME}", "%USERPROFILE%"];
        for prefix in home_prefixes {
            if value == prefix {
                return home;
            }
            if let Some(suffix) = value.strip_prefix(prefix) {
                if suffix.starts_with('/') || suffix.starts_with('\\') {
                    return home.join(suffix.trim_start_matches(['/', '\\']));
                }
            }
        }
        if value.starts_with(&format!("{home_text}/"))
            || value.starts_with(&format!("{home_text}\\"))
        {
            return PathBuf::from(value);
        }
    }
    PathBuf::from(value)
}

pub(super) fn openssh_private_key_path(config: &SshConnectionConfig) -> Option<PathBuf> {
    let path = config
        .private_key_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(expand_local_config_path)?;
    path.is_file().then_some(path)
}

pub(super) fn openssh_target(config: &SshConnectionConfig) -> String {
    let host = config
        .host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.contains(':') {
        format!("{}@[{}]", config.username.trim(), host)
    } else {
        format!("{}@{}", config.username.trim(), host)
    }
}

pub(super) fn connect_ssh_tcp(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, String> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err("SSH 主机不能为空".to_string());
    }
    let address = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let addresses = address
        .to_socket_addrs()
        .map_err(|error| format!("SSH 地址无效：{error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("无法解析 SSH 主机：{host}"));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(format!(
        "无法连接 SSH 主机 {host}:{port}：{}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "没有可用地址".to_string())
    ))
}

/// Owns the temporary helper OpenSSH uses to read a stored password without
/// writing that password to an interactive terminal.
pub struct SshAskPass {
    pub(super) path: PathBuf,
}

impl SshAskPass {
    pub fn new(secret: &str) -> Result<Self, String> {
        if secret.is_empty() {
            return Err("SSH 密码不能为空".to_string());
        }
        Ok(Self {
            path: write_temporary_askpass(secret)?,
        })
    }

    pub fn program_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SshAskPass {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn write_temporary_askpass(secret: &str) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("无法生成 SSH 凭据临时路径: {error}"))?
        .as_nanos();
    let encoded = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
    let path = std::env::temp_dir().join(format!(
        "android-tool-ssh-askpass-{}-{stamp}{}",
        std::process::id(),
        if cfg!(windows) { ".cmd" } else { ".sh" }
    ));
    let contents = if cfg!(windows) {
        format!(
            "@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -Command \"$bytes=[Convert]::FromBase64String('{encoded}'); [Console]::Out.Write([Text.Encoding]::UTF8.GetString($bytes))\"\r\n"
        )
    } else {
        format!("#!/bin/sh\nprintf '%s' '{encoded}' | base64 -d\n")
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("无法创建 SSH 凭据临时文件: {error}"))?;
    if let Err(error) = file.write_all(contents.as_bytes()) {
        let _ = fs::remove_file(&path);
        return Err(format!("无法写入 SSH 凭据临时文件: {error}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
    }
    Ok(path)
}

pub(super) fn remove_temporary_askpass(askpass: &Option<SshAskPass>) {
    if let Some(askpass) = askpass {
        let _ = fs::remove_file(askpass.program_path());
    }
}

pub(super) fn spawn_openssh_command(
    config: &SshConnectionConfig,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    auth_secret: Option<&str>,
    pty: bool,
) -> Result<(std::process::Child, Option<SshAskPass>), String> {
    let key_path = openssh_private_key_path(config);
    let port = if config.port == 0 { 22 } else { config.port };
    let timeout_secs = timeout.as_secs().clamp(3, 1800);
    let askpass = auth_secret
        .filter(|value| !value.is_empty())
        .map(SshAskPass::new)
        .transpose()?;
    let mut process = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x08000000);
    }
    process
        .arg("-o")
        .arg(if askpass.is_some() {
            "BatchMode=no"
        } else {
            "BatchMode=yes"
        })
        .arg("-o")
        .arg(format!("ConnectTimeout={timeout_secs}"))
        .arg("-o")
        .arg(format!(
            "ServerAliveInterval={}",
            config.keepalive_interval_secs.clamp(5, 300)
        ))
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg(if config.strict_host_key {
            "StrictHostKeyChecking=yes"
        } else {
            "StrictHostKeyChecking=no"
        });
    if config.strict_host_key {
        if let Some(path) = config
            .known_hosts_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            process.arg("-o").arg(format!(
                "UserKnownHostsFile={}",
                expand_local_config_path(path).to_string_lossy()
            ));
        }
    } else {
        process.arg("-o").arg(if cfg!(windows) {
            "UserKnownHostsFile=NUL"
        } else {
            "UserKnownHostsFile=/dev/null"
        });
    }
    if let Some(askpass) = &askpass {
        process
            .env("SSH_ASKPASS", askpass.program_path())
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "android-tool");
    }
    process.arg("-p").arg(port.to_string());
    if pty {
        process.arg("-tt").arg("-o").arg("LogLevel=ERROR");
    }
    if let Some(key_path) = key_path {
        process
            .arg("-i")
            .arg(key_path)
            .arg("-o")
            .arg("IdentitiesOnly=yes");
    }
    let child = process
        .arg(openssh_target(config))
        .arg(command)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    match child {
        Ok(child) => Ok((child, askpass)),
        Err(error) => {
            remove_temporary_askpass(&askpass);
            Err(format!("无法启动系统 SSH：{error}"))
        }
    }
}

/// 把 russh 执行结果适配成 `std::process::Output`（run_openssh 家族的统一
/// 返回形状；调用方只读 success/code/stdout/stderr）。
fn openssh_output_from_russh(result: SshCommandResult) -> Output {
    let code = if result.exit_code <= 0 {
        result.exit_code.max(0) as u32
    } else {
        result.exit_code as u32
    };
    #[cfg(windows)]
    let status = std::os::windows::process::ExitStatusExt::from_raw(code);
    #[cfg(unix)]
    let status = std::os::unix::process::ExitStatusExt::from_raw(code as i32);
    Output {
        status,
        stdout: result.stdout.into_bytes(),
        stderr: result.stderr.into_bytes(),
    }
}

pub(super) fn run_openssh_command_with_secret(
    config: &SshConnectionConfig,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    auth_secret: Option<&str>,
) -> Result<Output, String> {
    run_openssh_command_with_cancellation(config, command, input, timeout, None, auth_secret, None)
}

pub(super) enum OpenSshOutputEvent {
    Chunk(&'static str, Vec<u8>),
    Finished(&'static str),
    Error(&'static str, String),
}

pub(super) struct OpenSshOwnedProcess<'a> {
    pub(super) root: &'a str,
    pub(super) pty: bool,
    pub(super) drained: std::cell::Cell<bool>,
    pub(super) output_truncated: std::cell::Cell<bool>,
}

impl OpenSshOwnedProcess<'_> {
    pub(super) fn drain_script(&self) -> &'static str {
        if self.pty {
            crate::owned_process::DRAIN_PTY
        } else {
            crate::owned_process::DRAIN
        }
    }
}

pub(super) fn spawn_openssh_output_reader<R>(
    mut reader: R,
    stream: &'static str,
    sender: mpsc::SyncSender<OpenSshOutputEvent>,
) where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => {
                    if sender
                        .send(OpenSshOutputEvent::Chunk(stream, buffer[..length].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(OpenSshOutputEvent::Error(stream, error.to_string()));
                    break;
                }
            }
        }
        let _ = sender.send(OpenSshOutputEvent::Finished(stream));
    });
}

pub(super) fn run_openssh_command_with_cancellation(
    config: &SshConnectionConfig,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    cancellation: Option<&Arc<AtomicBool>>,
    auth_secret: Option<&str>,
    sink: Option<&CommandOutputSink>,
) -> Result<Output, String> {
    if let Some(cancellation) = cancellation {
        cancelled(cancellation)?;
    }
    let root = if cancellation.is_some() {
        let prepared = run_openssh_raw(
            config,
            crate::owned_process::PREPARE,
            None,
            Duration::from_secs(10),
            // PREPARE 自己也要能被取消：它和真正的命令各占一次 exec 名额，
            // 传 None 会让「暂停」在排队阶段完全失效。
            cancellation,
            auth_secret,
            None,
            None,
        )?;
        let root = String::from_utf8_lossy(&prepared.stdout).trim().to_string();
        if !prepared.status.success() || !crate::owned_process::valid_control_root(&root) {
            return Err("SSH 无法建立命令进程归属，命令未执行".into());
        }
        Some(root)
    } else {
        None
    };
    let remote_command = root
        .as_ref()
        .map(|root| crate::owned_process::shell_command(root, "sh", &["-c".into(), command.into()]))
        .unwrap_or_else(|| command.into());
    let owned = root.as_deref().map(|root| OpenSshOwnedProcess {
        root,
        pty: false,
        drained: false.into(),
        output_truncated: false.into(),
    });
    let result = run_openssh_raw(
        config,
        &remote_command,
        input,
        timeout,
        cancellation,
        auth_secret,
        sink,
        owned.as_ref(),
    );
    if let Some(owned) = owned.as_ref() {
        finalize_openssh_owned(config, owned, auth_secret)?;
    }
    result
}

pub(super) fn run_openssh_owned_control(
    config: &SshConnectionConfig,
    root: &str,
    auth_secret: Option<&str>,
    script: &str,
) -> Result<String, String> {
    let command = format!(
        "sh -c {} android-tool-control {}",
        shell_escape(script),
        shell_escape(root)
    );
    let result = run_openssh_raw(
        config,
        &command,
        None,
        Duration::from_secs(10),
        None,
        auth_secret,
        None,
        None,
    );
    match result {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().into())
        }
        Ok(output) => Err(format!(
            "{}SSH 子进程停止未确认（{}）",
            crate::process::PROCESS_CLEANUP_ERROR_PREFIX,
            output.status
        )),
        Err(error) => Err(format!(
            "{}SSH 子进程停止未确认: {error}",
            crate::process::PROCESS_CLEANUP_ERROR_PREFIX
        )),
    }
}

pub(super) fn drain_openssh_owned(
    config: &SshConnectionConfig,
    owned: &OpenSshOwnedProcess<'_>,
    auth_secret: Option<&str>,
) -> Result<(), String> {
    if !owned.drained.get() {
        run_openssh_owned_control(config, owned.root, auth_secret, owned.drain_script())?;
        owned.drained.set(true);
    }
    Ok(())
}

pub(super) fn finalize_openssh_owned(
    config: &SshConnectionConfig,
    owned: &OpenSshOwnedProcess<'_>,
    auth_secret: Option<&str>,
) -> Result<(), String> {
    let script = if owned.drained.get() {
        crate::owned_process::REMOVE.to_string()
    } else {
        format!("{}\n{}", owned.drain_script(), crate::owned_process::REMOVE)
    };
    run_openssh_owned_control(config, owned.root, auth_secret, &script)?;
    owned.drained.set(true);
    Ok(())
}

pub(super) fn run_openssh_pty_command(
    context: &SshToolContext,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<Value, String> {
    cancelled(&context.cancellation)?;
    let prepared = run_openssh_command_with_secret(
        &context.config,
        crate::owned_process::PREPARE,
        None,
        Duration::from_secs(10),
        context.password.as_deref(),
    )?;
    let root = String::from_utf8_lossy(&prepared.stdout).trim().to_string();
    if !prepared.status.success() || !crate::owned_process::valid_control_root(&root) {
        return Err("SSH 无法建立终端进程归属，命令未执行".into());
    }
    let owned = OpenSshOwnedProcess {
        root: &root,
        pty: true,
        drained: false.into(),
        output_truncated: false.into(),
    };
    let result = run_openssh_raw(
        &context.config,
        &crate::owned_process::pty_command(&root, command),
        input,
        timeout,
        Some(&context.cancellation),
        context.password.as_deref(),
        sink,
        Some(&owned),
    );
    finalize_openssh_owned(&context.config, &owned, context.password.as_deref())?;
    result.map(|output| json!({"command":command,"exitCode":output.status.code(),"success":output.status.success(),
        "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),
        "pty":true,"backend":"openssh", "outputTruncated":owned.output_truncated.get()}))
}

pub(super) fn run_openssh_raw(
    config: &SshConnectionConfig,
    remote_command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    cancellation: Option<&Arc<AtomicBool>>,
    auth_secret: Option<&str>,
    sink: Option<&CommandOutputSink>,
    owned: Option<&OpenSshOwnedProcess<'_>>,
) -> Result<Output, String> {
    if let Some(cancellation) = cancellation {
        cancelled(cancellation)?;
    }
    // v3 §21.4 S3：russh 引擎可用时，原本要 spawn ssh.exe 的 shell 命令改走
    // russh 连接——约 30 处「SFTP / shell」双路径工具点经 uses_openssh_fallback
    // 的有效判定汇入这里，一处拦截全部收敛。PTY 型命令（agent 交互工具）保持
    // ssh.exe；仅 Transport/Auth 失败才真正落到 ssh.exe 兜底。
    if owned.map_or(true, |process| !process.pty)
        && super::russh_runtime::russh_engine_active(config)
    {
        if let Some(cancellation) = cancellation {
            cancelled(cancellation)?;
        }
        match super::russh_runtime::russh_run_command(
            config,
            auth_secret,
            remote_command,
            input,
            cancellation.cloned().unwrap_or_default(),
            timeout,
            sink,
        ) {
            Ok(result) => {
                if result.output_truncated {
                    return Err("远程命令输出过大，已中止".to_string());
                }
                return Ok(openssh_output_from_russh(result));
            }
            Err(error) => match error.kind {
                super::russh_runtime::RusshErrorKind::Transport
                | super::russh_runtime::RusshErrorKind::Auth => {}
                super::russh_runtime::RusshErrorKind::Cancelled
                | super::russh_runtime::RusshErrorKind::Network
                | super::russh_runtime::RusshErrorKind::Other => return Err(error.to_string()),
            },
        }
    }
    // 每条 ssh.exe 子进程都要完整跑一遍 TCP+握手+认证，与库连接的新建握手
    // 共用同一份同服务器并发名额（≤2），避免回退路径单独撞上 MaxStartups。
    let _handshake_slot = acquire_ssh_handshake_slot(config);
    let (mut child, askpass) = spawn_openssh_command(
        config,
        remote_command,
        input,
        timeout,
        auth_secret,
        owned.is_some_and(|owned| owned.pty),
    )?;
    if let Some(input) = input {
        if let Some(mut stdin) = child.stdin.take() {
            let input = input.to_vec();
            // Do not block the output readers while a large base64 patch or
            // transfer payload is being written to the remote command.
            std::thread::spawn(move || {
                let _ = stdin.write_all(&input);
            });
        }
    }
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            remove_temporary_askpass(&askpass);
            return Err("系统 SSH 未提供 stdout 管道".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            remove_temporary_askpass(&askpass);
            return Err("系统 SSH 未提供 stderr 管道".to_string());
        }
    };
    let (sender, receiver) = mpsc::sync_channel(64);
    spawn_openssh_output_reader(stdout, "stdout", sender.clone());
    spawn_openssh_output_reader(stderr, "stderr", sender);

    let started = Instant::now();
    let mut status = None;
    let mut last_wait_progress = started;
    let mut streams_finished = [false, false];
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let mut output_sequence = 0u64;
    let mut reader_error = None;
    let mut pty_exit = None;
    let mut pty_drained_at = None;
    let mut last_pty_status = started - Duration::from_millis(100);

    let append_output = |stream: &'static str,
                         bytes: &[u8],
                         target: &mut Vec<u8>,
                         truncated: &mut bool,
                         sink: Option<&CommandOutputSink>,
                         output_sequence: &mut u64| {
        if let Some(sink) = sink.filter(|sink| sink.is_continuous()) {
            sink.emit_remote_bytes(*output_sequence, stream, bytes);
            *output_sequence = (*output_sequence).saturating_add(1);
        }
        let sink = sink.filter(|sink| !sink.is_continuous());
        let remaining = MAX_REMOTE_COMMAND_OUTPUT_BYTES.saturating_sub(target.len());
        let visible_length = bytes.len().min(remaining);
        if visible_length > 0 {
            target.extend_from_slice(&bytes[..visible_length]);
            if let Some(sink) = sink {
                sink.emit_remote_chunk(
                    *output_sequence,
                    stream,
                    &String::from_utf8_lossy(&bytes[..visible_length]),
                );
                *output_sequence = (*output_sequence).saturating_add(1);
            }
        }
        if visible_length < bytes.len() && !*truncated {
            *truncated = true;
            if let Some(sink) = sink {
                sink.emit_remote_chunk(
                    *output_sequence,
                    stream,
                    &format!("[远程 {stream} 已达到输出上限，后续内容已省略]\\n"),
                );
                *output_sequence = (*output_sequence).saturating_add(1);
            }
        }
    };

    loop {
        if cancellation.is_some_and(|value| value.load(Ordering::Relaxed)) {
            let cleanup = owned
                .map(|owned| drain_openssh_owned(config, owned, auth_secret))
                .unwrap_or(Ok(()));
            let _ = child.kill();
            let _ = child.wait();
            remove_temporary_askpass(&askpass);
            cleanup?;
            return Err("请求已取消".to_string());
        }
        if (pty_exit.is_none() && started.elapsed() >= timeout)
            || pty_drained_at
                .is_some_and(|started: Instant| started.elapsed() >= Duration::from_secs(10))
        {
            let cleanup = owned
                .map(|owned| drain_openssh_owned(config, owned, auth_secret))
                .unwrap_or(Ok(()));
            let _ = child.kill();
            let _ = child.wait();
            remove_temporary_askpass(&askpass);
            cleanup?;
            return Err(format!("系统 SSH 执行超时（{} 秒）", timeout.as_secs()));
        }
        if let Some(owned) = owned.filter(|owned| {
            owned.pty
                && pty_exit.is_none()
                && last_pty_status.elapsed() >= Duration::from_millis(100)
        }) {
            let polled = (|| -> Result<Option<i32>, String> {
                let code = run_openssh_owned_control(
                    config,
                    owned.root,
                    auth_secret,
                    r#"if test -r "$1/exit"; then cat "$1/exit"; fi"#,
                )?;
                if code.is_empty() {
                    return Ok(None);
                }
                let code = code
                    .parse::<i32>()
                    .ok()
                    .filter(|code| (0..=255).contains(code))
                    .ok_or("SSH 终端返回了无效的退出码")?;
                drain_openssh_owned(config, owned, auth_secret)?;
                Ok(Some(code))
            })();
            match polled {
                Ok(Some(code)) => {
                    pty_exit = Some(code);
                    pty_drained_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    remove_temporary_askpass(&askpass);
                    return Err(error);
                }
            }
            last_pty_status = Instant::now();
        }
        if last_wait_progress.elapsed() >= Duration::from_secs(5) {
            if let Some(sink) = sink {
                sink.emit_remote_progress(json!({"stage": "waiting", "elapsedSeconds": started.elapsed().as_secs(), "message": format!("远程命令仍在执行，已等待 {} 秒", started.elapsed().as_secs())}));
            }
            last_wait_progress = Instant::now();
        }
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    remove_temporary_askpass(&askpass);
                    return Err(format!("等待系统 SSH 失败：{error}"));
                }
            };
        }
        // Yield to cancellation and timeout checks even when the producer never goes idle.
        for _ in 0..64 {
            let Ok(event) = receiver.try_recv() else {
                break;
            };
            match event {
                OpenSshOutputEvent::Chunk(stream, bytes) if stream == "stdout" => {
                    append_output(
                        stream,
                        &bytes,
                        &mut stdout_bytes,
                        &mut stdout_truncated,
                        sink,
                        &mut output_sequence,
                    );
                }
                OpenSshOutputEvent::Chunk(stream, bytes) => {
                    append_output(
                        stream,
                        &bytes,
                        &mut stderr_bytes,
                        &mut stderr_truncated,
                        sink,
                        &mut output_sequence,
                    );
                }
                OpenSshOutputEvent::Finished(stream) if stream == "stdout" => {
                    streams_finished[0] = true;
                }
                OpenSshOutputEvent::Finished(_) => {
                    streams_finished[1] = true;
                }
                OpenSshOutputEvent::Error(stream, error) => {
                    reader_error = Some(format!("远程 {stream} 输出读取失败：{error}"));
                }
            }
        }
        if status.is_some() && streams_finished.iter().all(|finished| *finished) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(error) = reader_error {
        remove_temporary_askpass(&askpass);
        return Err(error);
    }
    let status = status.ok_or_else(|| "系统 SSH 没有返回退出状态".to_string())?;
    remove_temporary_askpass(&askpass);
    if let Some(owned) = owned {
        owned
            .output_truncated
            .set(stdout_truncated || stderr_truncated);
    }
    let status = if owned.is_some_and(|owned| owned.pty) {
        let code = pty_exit.ok_or("SSH 终端在发布退出状态前关闭")?;
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        }
    } else {
        status
    };
    Ok(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

pub(super) fn run_openssh_context_command(
    context: &SshToolContext,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<Output, String> {
    run_openssh_command_with_cancellation(
        &context.config,
        command,
        input,
        timeout,
        Some(&context.cancellation),
        context.password.as_deref(),
        sink,
    )
}

pub(super) fn run_openssh_context_command_checked(
    context: &SshToolContext,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
    paths: &[&str],
    operation: &str,
) -> Result<Output, String> {
    run_openssh_context_command_checked_with_options(
        context, command, input, timeout, sink, paths, operation,
    )
}

pub(super) fn run_openssh_context_command_checked_with_options(
    context: &SshToolContext,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
    paths: &[&str],
    operation: &str,
) -> Result<Output, String> {
    let guard = if context.allow_external_paths || paths.is_empty() {
        String::new()
    } else {
        let root = shell_escape(&context.root);
        let mut script = format!(
            "root_candidate={root}; if [ -d \"$root_candidate\" ]; then root_real=$(cd \"$root_candidate\" 2>/dev/null && pwd -P); else root_ancestor=\"$root_candidate\"; while [ \"$root_ancestor\" != \"/\" ] && [ ! -e \"$root_ancestor\" ] && [ ! -L \"$root_ancestor\" ]; do root_ancestor=$(dirname \"$root_ancestor\"); done; if [ -L \"$root_ancestor\" ] || [ ! -d \"$root_ancestor\" ]; then printf '%s\\n' 'SSH 工作目录不可解析' >&2; exit 97; fi; root_parent_real=$(cd \"$root_ancestor\" 2>/dev/null && pwd -P) || exit 97; root_real=\"$root_parent_real/$(basename \"$root_candidate\")\"; fi; guard_path() {{ candidate=\"$1\"; if [ -L \"$candidate\" ]; then return 2; else if [ -d \"$candidate\" ]; then resolved=$(cd \"$candidate\" 2>/dev/null && pwd -P) || return 3; else if [ -e \"$candidate\" ]; then ancestor=$(dirname \"$candidate\"); else ancestor=\"$candidate\"; while [ \"$ancestor\" != \"/\" ] && [ ! -e \"$ancestor\" ] && [ ! -L \"$ancestor\" ]; do ancestor=$(dirname \"$ancestor\"); done; fi; if [ -L \"$ancestor\" ] || [ ! -d \"$ancestor\" ]; then return 3; fi; parent_real=$(cd \"$ancestor\" 2>/dev/null && pwd -P) || return 3; resolved=\"$parent_real/$(basename \"$candidate\")\"; fi; fi; case \"$resolved\" in \"$root_real\"|\"$root_real\"/*) return 0 ;; *) return 4 ;; esac; }}; ",
            root = root,
        );
        for path in paths {
            script.push_str(&format!(
                "guard_path {path} || {{ printf '%s\\n' {operation} >&2; exit 97; }}; ",
                path = shell_escape(path),
                operation = shell_escape(&format!("{operation} 被 SSH 工作目录安全边界拒绝")),
            ));
        }
        script
    };
    let guarded_command = if guard.is_empty() {
        command.to_string()
    } else {
        format!("{guard}{command}")
    };
    let output = run_openssh_context_command(context, &guarded_command, input, timeout, sink)?;
    if output.status.code() == Some(97)
        && String::from_utf8_lossy(&output.stderr).contains("SSH 工作目录安全边界")
    {
        return Err(format!("{operation} 被 SSH 工作目录安全边界拒绝"));
    }
    Ok(output)
}

/// libssh2 协商失败的记忆（真标记，不含派生语义）。引擎选择与记忆管理用它；
/// 「选哪条传输路径」用 [`uses_openssh_fallback`]（有效判定）。
pub(super) fn is_openssh_transport_marked(config: &SshConnectionConfig) -> bool {
    openssh_transports()
        .lock()
        .map(|transports| transports.contains(&connection_pool_key(config)))
        .unwrap_or(false)
}

/// Streams a local file into an openssh/russh command's stdin without ever
/// holding the whole payload (or a base64 copy of it) in memory.
///
/// The base64 pipeline this replaces encoded the entire file and wrote it as
/// one `input` buffer: 33% more bytes on the wire plus a full-size encode for
/// every upload. Streaming raw bytes measured ~2.5x faster on a 20 MB file.
pub(super) fn stream_file_to_openssh(
    context: &SshToolContext,
    command: &str,
    local_path: &Path,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
    total: u64,
) -> Result<u64, String> {
    // PREPARE 也要接上取消：四个并行上传会各自先跑一次 PREPARE，通道名额
    // 紧张时这一步能排上十几秒，而原来走的是 `run_openssh_command_with_secret`
    // （内部把 cancellation 固定传 None），用户点「暂停」后它照样跑完才轮到
    // 后面的取消检查——这就是「点了停止还在转」的一部分。这里直接调
    // `run_openssh_raw` 传真实取消标志，避免多套一层 owned 包装。
    let root = run_openssh_raw(
        &context.config,
        crate::owned_process::PREPARE,
        None,
        Duration::from_secs(10),
        Some(&context.cancellation),
        context.password.as_deref(),
        None,
        None,
    )?;
    let owned_root = String::from_utf8_lossy(&root.stdout).trim().to_string();
    if !root.status.success() || !crate::owned_process::valid_control_root(&owned_root) {
        return Err("SSH 无法建立命令进程归属，命令未执行".into());
    }
    let owned = OpenSshOwnedProcess {
        root: &owned_root,
        pty: false,
        drained: false.into(),
        output_truncated: false.into(),
    };
    let remote_command =
        crate::owned_process::shell_command(&owned_root, "sh", &["-c".into(), command.into()]);
    let result = write_stream_through_openssh(
        &context.config,
        &remote_command,
        local_path,
        timeout,
        context.password.as_deref(),
        sink,
        total,
        &context.cancellation,
    );
    let finalize = finalize_openssh_owned(&context.config, &owned, context.password.as_deref());
    let written = result?;
    finalize?;
    Ok(written)
}

#[allow(clippy::too_many_arguments)]
fn write_stream_through_openssh(
    config: &SshConnectionConfig,
    remote_command: &str,
    local_path: &Path,
    timeout: Duration,
    auth_secret: Option<&str>,
    sink: Option<&CommandOutputSink>,
    total: u64,
    cancellation: &Arc<AtomicBool>,
) -> Result<u64, String> {
    cancelled(cancellation)?;
    if super::russh_runtime::russh_engine_active(config) {
        // russh already writes stdin asynchronously from a tokio task; feed it
        // the file contents in one buffer only when it is small enough to be
        // cheap, otherwise fall through to the ssh.exe streaming path.
        if total <= STREAM_VIA_RUSSH_LIMIT {
            let bytes =
                fs::read(local_path).map_err(|error| format!("读取待上传文件失败：{error}"))?;
            match super::russh_runtime::russh_run_command(
                config,
                auth_secret,
                remote_command,
                Some(&bytes),
                cancellation.clone(),
                timeout,
                sink,
            ) {
                Ok(result) if result.exit_code == 0 => return Ok(total),
                Ok(result) => return Err(upload_status_error(result.exit_code, &result.stderr)),
                Err(error) => match error.kind {
                    super::russh_runtime::RusshErrorKind::Transport
                    | super::russh_runtime::RusshErrorKind::Auth => {}
                    _ => return Err(error.to_string()),
                },
            }
        }
    }
    let _handshake_slot = acquire_ssh_handshake_slot(config);
    let (mut child, askpass) =
        spawn_openssh_command(config, remote_command, None, timeout, auth_secret, false)?;
    let mut source =
        fs::File::open(local_path).map_err(|error| format!("无法打开待上传文件：{error}"))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Drain both streams so a chatty remote command can never block on a full
    // pipe while we are still feeding it the body.
    let stdout_reader = stdout.map(|stream| std::thread::spawn(move || drain_stream(stream)));
    let stderr_reader = stderr.map(|stream| std::thread::spawn(move || drain_stream(stream)));
    let write_result = match stdin {
        Some(mut stdin) => copy_upload_stream(&mut source, &mut stdin, cancellation, sink, total),
        None => Err("系统 SSH 未提供 stdin 管道".to_string()),
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if let Err(error) = cancelled(cancellation) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("等待 SSH 上传进程失败：{error}"));
            }
        }
    };
    let stderr_text = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    }
    remove_temporary_askpass(&askpass);
    let written = write_result?;
    let status = status?;
    if !status.success() {
        return Err(upload_status_error(
            status.code().unwrap_or(-1),
            &stderr_text,
        ));
    }
    Ok(written)
}

fn copy_upload_stream(
    source: &mut fs::File,
    destination: &mut impl Write,
    cancellation: &Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
    total: u64,
) -> Result<u64, String> {
    let mut buffer = vec![0u8; REMOTE_UPLOAD_STREAM_BYTES];
    let mut completed = 0u64;
    let mut reported = Instant::now();
    loop {
        cancelled(cancellation)?;
        let length = source
            .read(&mut buffer)
            .map_err(|error| format!("读取本地上传文件失败：{error}"))?;
        if length == 0 {
            break;
        }
        destination
            .write_all(&buffer[..length])
            .map_err(|error| format!("写入 SSH 上传管道失败：{error}"))?;
        completed = completed.saturating_add(length as u64);
        if let Some(sink) = sink.filter(|_| reported.elapsed() >= REMOTE_PROGRESS_INTERVAL) {
            sink.emit_remote_progress(json!({
                "kind": "remote_transfer",
                "direction": "upload",
                "target": "ssh",
                "message": format!("已上传 {completed} / {total} 字节"),
            }));
            reported = Instant::now();
        }
    }
    Ok(completed)
}

fn drain_stream(mut stream: impl Read) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) => {
                let room = 8192usize.saturating_sub(kept.len());
                if room > 0 {
                    kept.extend_from_slice(&buffer[..length.min(room)]);
                }
            }
        }
    }
    String::from_utf8_lossy(&kept).trim().to_string()
}

pub(super) fn upload_status_error(code: i32, stderr: &str) -> String {
    match code {
        2 => "远程目标文件已存在；如需覆盖请明确传入 overwrite=true".to_string(),
        3 => "远程目录不存在，请先创建目标目录".to_string(),
        _ if stderr.trim().is_empty() => format!("上传文件失败（退出码 {code}）"),
        _ => format!("上传文件失败：{}", stderr.trim()),
    }
}

/// 私钥是 libssh2 无法加载的加密 OpenSSH 格式（russh 可以加载，故仅在 russh
/// 引擎不可用时才构成走系统 ssh.exe 的理由）。
fn openssh_key_format_required(config: &SshConnectionConfig) -> bool {
    let Some(path) = openssh_private_key_path(config) else {
        return false;
    };
    // Unencrypted Ed25519/RSA OpenSSH keys are converted to a temporary PEM
    // inside connect(), so they can use the pooled libssh2 session. Keep the
    // system OpenSSH fallback only for encrypted or otherwise unsupported key
    // formats that libssh2 cannot authenticate directly.
    fs::read_to_string(path)
        .map(|contents| {
            contents
                .trim_start()
                .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
                && openssh_private_key_to_pem(&contents).is_err()
        })
        .unwrap_or(true)
}

/// 有效传输判定（v3 §21.4 S3）：走 shell 传输而非 libssh2 SFTP 的条件——
/// ① 已标记 openssh 兼容（libssh2 协商失败）→ 系统 ssh.exe；
/// ② russh 引擎可用 → shell 命令由 russh 通道执行（run_openssh 家族统一
///    拦截，见 `run_openssh_command_with_cancellation`）；
/// ③ 私钥格式 libssh2 不支持且 russh 不可用 → 系统 ssh.exe。
pub(super) fn uses_openssh_fallback(config: &SshConnectionConfig) -> bool {
    is_openssh_transport_marked(config)
        || super::russh_runtime::russh_engine_active(config)
        || openssh_key_format_required(config)
}

pub(super) fn can_retry_with_openssh(config: &SshConnectionConfig, password: Option<&str>) -> bool {
    openssh_private_key_path(config).is_some()
        || password.is_some_and(|value| !value.trim().is_empty())
}

pub(super) fn remember_openssh_transport(config: &SshConnectionConfig) {
    invalidate_cached_connection(config);
    if let Ok(mut transports) = openssh_transports().lock() {
        transports.insert(connection_pool_key(config));
    }
}

pub(super) fn should_fallback_to_openssh(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("sftp")
        || error.contains("subsystem")
        || error.contains("exchange encryption keys")
        || error.contains("key exchange")
        || error.contains("handshake")
}

pub(super) fn verify_openssh_connection(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    let result = run_openssh_command_with_secret(
        config,
        "printf android-tool-ssh-ok",
        None,
        Duration::from_secs(config.operation_timeout_secs.clamp(10, 120)),
        password,
    )?;
    if result.status.success()
        && String::from_utf8_lossy(&result.stdout).trim() == "android-tool-ssh-ok"
    {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
    Err(if error.is_empty() {
        "SSH 已连接，但远程命令验证失败".to_string()
    } else {
        format!("SSH 连接验证失败：{error}")
    })
}

/// Read the server host key without authenticating. This lets the settings
/// UI show a fingerprint and explain whether the configured known_hosts entry
/// matches before the user starts an agent turn.
pub fn query_ssh_host_key(config: &SshConnectionConfig) -> Result<SshHostKeyInfo, String> {
    let port = if config.port == 0 { 22 } else { config.port };
    let host = config
        .host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let tcp = connect_ssh_tcp(
        host,
        port,
        Duration::from_secs(config.connect_timeout_secs.clamp(3, 60)),
    )?;
    let mut session = Session::new().map_err(ssh_error)?;
    session.set_tcp_stream(tcp);
    session.set_timeout((config.connect_timeout_secs.clamp(3, 60) * 1000) as u32);
    session.handshake().map_err(ssh_error)?;
    let (_, key_type) = session
        .host_key()
        .ok_or_else(|| "SSH 主机没有提供主机密钥".to_string())?;
    let fingerprint = session
        .host_key_hash(ssh2::HashType::Sha256)
        .map(|hash| {
            format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash)
            )
        })
        .ok_or_else(|| "无法计算 SSH 主机密钥指纹".to_string())?;
    let algorithm = format!("{key_type:?}").to_ascii_lowercase();
    let known_host_status = config
        .known_hosts_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(expand_local_config_path)
        .or_else(default_known_hosts_path)
        .and_then(|path| {
            let mut known_hosts = session.known_hosts().ok()?;
            known_hosts
                .read_file(&path, ssh2::KnownHostFileKind::OpenSSH)
                .ok()?;
            let (key, _) = session.host_key()?;
            Some(match known_hosts.check_port(host, port, key) {
                ssh2::CheckResult::Match => "match",
                ssh2::CheckResult::Mismatch => "mismatch",
                ssh2::CheckResult::NotFound => "not_found",
                ssh2::CheckResult::Failure => "error",
            })
        })
        .unwrap_or("not_checked")
        .to_string();
    Ok(SshHostKeyInfo {
        host: host.to_string(),
        port,
        algorithm,
        fingerprint,
        known_host_status,
    })
}

pub(super) fn list_ssh_directories_with_openssh(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
) -> Result<Vec<SshDirectoryEntry>, String> {
    let command = format!(
        "setopt NULL_GLOB 2>/dev/null || true; for entry in {path}/* {path}/.[!.]* {path}/..?*; do if [ -d \"$entry\" ] && [ ! -L \"$entry\" ]; then printf '%s\\n' \"$entry\"; fi; done",
        path = shell_escape(path),
    );
    let result = run_openssh_command_with_secret(
        config,
        &command,
        None,
        Duration::from_secs(config.operation_timeout_secs.clamp(10, 120)),
        password,
    )?;
    if !result.status.success() {
        return Err(format!(
            "读取远程目录失败：{}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&result.stdout);
    let directories = stdout
        .lines()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if directories.len() > MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "读取远程目录超过 {} 项限制",
            MAX_REMOTE_DIRECTORY_ENTRIES
        ));
    }
    Ok(directories
        .into_iter()
        .filter_map(|value| {
            let path = normalize_remote_path(value.trim());
            let name = Path::new(&path).file_name()?.to_str()?.to_string();
            Some(SshDirectoryEntry { name, path })
        })
        .collect())
}

pub fn list_ssh_directories(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
) -> Result<Vec<SshDirectoryEntry>, String> {
    let requested_path = if path.trim().is_empty() {
        "~"
    } else {
        path.trim()
    };
    let path = if is_symbolic_remote_path(requested_path) {
        resolve_ssh_root(config, password, Some(requested_path))?
    } else {
        normalize_remote_path(requested_path)
    };
    if uses_openssh_fallback(config) {
        return list_ssh_directories_with_openssh(config, password, &path);
    }
    let session = match connect(config, password) {
        Ok(session) => session,
        Err(error)
            if can_retry_with_openssh(config, password) && should_fallback_to_openssh(&error) =>
        {
            remember_openssh_transport(config);
            return list_ssh_directories_with_openssh(config, password, &path);
        }
        Err(error) => return Err(error),
    };
    session.set_blocking(false);
    let sftp = match retry_nonblocking(
        "初始化 SSH 目录服务",
        Duration::from_secs(10),
        || session.sftp(),
    ) {
        Ok(sftp) => sftp,
        Err(error)
            if can_retry_with_openssh(config, password) && should_fallback_to_openssh(&error) =>
        {
            remember_openssh_transport(config);
            return list_ssh_directories_with_openssh(config, password, &path);
        }
        Err(error) => return Err(error),
    };
    let mut entries =
        match retry_nonblocking("读取 SSH 远程目录", Duration::from_secs(10), || {
            sftp.readdir(Path::new(&path))
        }) {
            Ok(entries) => entries,
            Err(error)
                if can_retry_with_openssh(config, password)
                    && should_fallback_to_openssh(&error) =>
            {
                remember_openssh_transport(config);
                return list_ssh_directories_with_openssh(config, password, &path);
            }
            Err(error) => return Err(error),
        };
    entries.retain(|(_, stat)| matches!(stat.file_type(), ssh2::FileType::Directory));
    if entries.len() > MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "读取远程目录超过 {} 项限制",
            MAX_REMOTE_DIRECTORY_ENTRIES
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries
        .into_iter()
        .filter_map(|(entry, _)| {
            let name = entry.file_name()?.to_str()?.to_string();
            Some(SshDirectoryEntry {
                name,
                path: remote_directory_entry_path(&path, &entry),
            })
        })
        .collect())
}

/// Resolve the configured remote root on the SSH host before registering tools.
///
/// The desktop may persist a symbolic root such as `~`, or no root at all when
/// the user has only tested the connection. SFTP does not expand shell
/// expressions, so resolving here prevents every later file tool from trying
/// to open a literal `/~` directory.
pub fn resolve_ssh_root(
    config: &SshConnectionConfig,
    password: Option<&str>,
    requested_root: Option<&str>,
) -> Result<String, String> {
    super::session::with_connection_operation(config, || {
        resolve_ssh_root_inner(config, password, requested_root)
    })
}

fn resolve_ssh_root_inner(
    config: &SshConnectionConfig,
    password: Option<&str>,
    requested_root: Option<&str>,
) -> Result<String, String> {
    let cancellation = Arc::new(AtomicBool::new(false));
    let configured_root = requested_root
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let expression = configured_root
        .map(remote_shell_path_expression)
        .transpose()?
        .unwrap_or_else(|| "$HOME".to_string());
    let command = if configured_root.is_some() {
        format!("cd {expression} && pwd")
    } else {
        // Some restricted accounts do not export HOME. In that case the SSH
        // login directory is still a valid, user-scoped remote workspace.
        format!("if [ -n \"${{HOME:-}}\" ]; then cd \"$HOME\"; fi; pwd")
    };
    if uses_openssh_fallback(config) {
        let result = run_openssh_command_with_secret(
            config,
            &command,
            None,
            Duration::from_secs(config.operation_timeout_secs.clamp(10, 120)),
            password,
        )?;
        if !result.status.success() {
            return Err(format!(
                "远程工作目录不可用：{}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let resolved = String::from_utf8_lossy(&result.stdout);
        return resolved
            .lines()
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(normalize_remote_path)
            .ok_or_else(|| "远程主机没有返回工作目录".to_string());
    }
    let session = connect(config, password)?;
    session.set_blocking(false);
    let context = SshToolContext::new(
        config.clone(),
        configured_root.unwrap_or("/"),
        password.map(str::to_string),
        cancellation.clone(),
        None,
    );
    let channel = retry_nonblocking_with_cancel(
        "打开 SSH 远程工作目录通道",
        Duration::from_secs(10),
        &cancellation,
        || session.channel_session(),
    )?;
    let result = execute_remote_command(
        channel,
        &session,
        &command,
        &context,
        None,
        Duration::from_secs(10),
        None,
    )?;
    if result.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(result
            .get("stderr")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("远程工作目录不可用：{}", value.trim()))
            .unwrap_or_else(|| "远程工作目录不可用".to_string()));
    }
    let resolved = result
        .get("stdout")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "远程主机没有返回工作目录".to_string())?;
    Ok(normalize_remote_path(resolved))
}
