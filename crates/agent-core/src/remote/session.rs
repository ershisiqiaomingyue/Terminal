use super::*;

const SSH_CONNECT_ATTEMPTS: usize = 3;

/// EAGAIN 退避：先在 0 与 1ms 之间让出，让同一条通知驱动上的读/写/window
/// adjust 先推进；超过 1ms 才做一次真正的 sleep。libssh2 非阻塞模式下的
/// 「写满窗口」是常态，固定 10ms 会让 8 KB 分片的上传降到几十 KB/s。
fn eagain_backoff(spins: u32) {
    if spins == 0 {
        std::thread::yield_now();
    } else if spins < 8 {
        std::thread::sleep(Duration::from_millis(1));
    } else {
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn retry_nonblocking<T, F>(
    operation: &str,
    timeout: Duration,
    mut action: F,
) -> Result<T, String>
where
    F: FnMut() -> Result<T, ssh2::Error>,
{
    let started = Instant::now();
    let mut spins = 0;
    loop {
        match action() {
            Ok(value) => return Ok(value),
            Err(error) if error.code() == ssh2::ErrorCode::Session(SSH2_ERROR_EAGAIN) => {
                if started.elapsed() >= timeout {
                    return Err(format!("{operation}超时，请检查 SSH 连接是否仍然可用"));
                }
                eagain_backoff(spins);
                spins = spins.saturating_add(1);
            }
            Err(error) => return Err(ssh_error(error)),
        }
    }
}

pub(super) fn retry_nonblocking_with_cancel<T, F>(
    operation: &str,
    timeout: Duration,
    cancellation: &Arc<AtomicBool>,
    mut action: F,
) -> Result<T, String>
where
    F: FnMut() -> Result<T, ssh2::Error>,
{
    let started = Instant::now();
    let mut spins = 0;
    loop {
        cancelled(cancellation)?;
        match action() {
            Ok(value) => return Ok(value),
            Err(error) if error.code() == ssh2::ErrorCode::Session(SSH2_ERROR_EAGAIN) => {
                if started.elapsed() >= timeout {
                    return Err(format!("{operation}超时，请检查 SSH 连接是否仍然可用"));
                }
                eagain_backoff(spins);
                spins = spins.saturating_add(1);
            }
            Err(error) => return Err(ssh_error(error)),
        }
    }
}

pub(super) fn retry_io_with_cancel<T, F>(
    operation: &str,
    timeout: Duration,
    cancellation: &Arc<AtomicBool>,
    mut action: F,
) -> Result<T, String>
where
    F: FnMut() -> std::io::Result<T>,
{
    let started = Instant::now();
    let mut spins = 0;
    loop {
        cancelled(cancellation)?;
        match action() {
            Ok(value) => return Ok(value),
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                if started.elapsed() >= timeout {
                    return Err(format!("{operation}超时，请检查 SSH 连接是否仍然可用"));
                }
                eagain_backoff(spins);
                spins = spins.saturating_add(1);
            }
            Err(error) => return Err(format!("{operation}失败: {error}")),
        }
    }
}

pub(super) fn read_remote_bytes<R: Read>(
    reader: &mut R,
    max_bytes: usize,
    cancellation: &Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0u8; 8192];
    while bytes.len() < max_bytes {
        let length = retry_io_with_cancel(
            "读取远程文件",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            cancellation,
            || reader.read(&mut buffer),
        )?;
        if length == 0 {
            break;
        }
        let remaining = max_bytes - bytes.len();
        bytes.extend_from_slice(&buffer[..length.min(remaining)]);
        if length > remaining {
            break;
        }
    }
    Ok(bytes)
}

pub(super) fn write_remote_bytes<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    cancellation: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let written = retry_io_with_cancel(
            "写入远程文件",
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            cancellation,
            || writer.write(&bytes[offset..]),
        )?;
        if written == 0 {
            return Err("写入远程文件失败: 远程文件通道已关闭".to_string());
        }
        offset += written;
    }
    Ok(())
}

/// v3 §21.4 S3：功能泳道后缀（libssh2 会话串行化时代的产物——浏览/工作区
/// Git/cwd 探测各占一条独立连接）。russh 引擎上这些泳道共享同一条连接
/// （通道级并发），引擎的池键与回退记忆键都必须剥离后缀归一。
pub(super) const SSH_LANE_SUFFIXES: [&str; 4] = [
    ":file-browser",
    ":git-browser",
    ":workspace-git",
    ":cwd-probe",
];

pub(super) fn strip_lane_suffix(id: &str) -> &str {
    SSH_LANE_SUFFIXES
        .iter()
        .find(|suffix| id.ends_with(*suffix))
        .map(|suffix| &id[..id.len() - suffix.len()])
        .unwrap_or(id)
}

/// russh 引擎键：泳道归一后的池/记忆键（字段口径与 libssh2 池键一致）。
pub(super) fn russh_engine_pool_key(config: &SshConnectionConfig) -> String {
    if !config.id.contains(':') {
        return connection_pool_key(config);
    }
    let normalized = SshConnectionConfig {
        id: strip_lane_suffix(&config.id).to_string(),
        ..config.clone()
    };
    connection_pool_key(&normalized)
}

pub(super) fn connection_pool_key(config: &SshConnectionConfig) -> String {
    [
        config.id.clone(),
        config.host.trim().to_string(),
        config.port.to_string(),
        config.username.trim().to_string(),
        config
            .private_key_path
            .as_deref()
            .unwrap_or_default()
            .to_string(),
        config
            .known_hosts_path
            .as_deref()
            .unwrap_or_default()
            .to_string(),
        config.strict_host_key.to_string(),
        config.connect_timeout_secs.to_string(),
        config.keepalive_interval_secs.to_string(),
        config.operation_timeout_secs.to_string(),
    ]
    .join("\u{1f}")
}

/// v3 §21.4 S0：连接门控的快路径——池里已有通过 keepalive 探活的已认证会话时
/// 直接视为「已连接」，免掉一次全新握手。显式「测试连接」（设置页的测试按钮）
/// 仍走 `test_ssh_connection` 的强制新握手——它校验的是用户刚输入的凭据，
/// 不能让池里的旧会话把改错的密码判成通过。
pub fn ensure_ssh_connected(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    if cached_alive_session(config).is_some() || super::russh_runtime::russh_pool_alive(config) {
        return Ok(());
    }
    test_ssh_connection(config, password)
}

fn cached_alive_session(config: &SshConnectionConfig) -> Option<Session> {
    if !config.reuse_connection {
        return None;
    }
    let pool_key = connection_pool_key(config);
    let session = {
        let pool = ssh_connection_pool().lock().ok()?;
        pool.get(&pool_key).cloned()?
    };
    if !session.authenticated() {
        return None;
    }
    configure_session_keepalive(&session, config.keepalive_interval_secs);
    match send_session_keepalive(&session) {
        Ok(()) => Some(session),
        Err(error) if error.contains(&SSH2_ERROR_EAGAIN.to_string()) => Some(session),
        Err(_) => {
            invalidate_cached_connection(config);
            None
        }
    }
}

// v3 §21.4 S0：同一服务器并发「新建握手」上限。OpenSSH 默认 MaxStartups
/// `10:30:100`，多个终端页签 + 对话工具 + 文件操作同时冷启动时会概率性被
/// 服务器随机丢弃握手；这里把每台服务器的新建握手并发压到 2，其余排队。
const MAX_CONCURRENT_SSH_HANDSHAKES: usize = 2;

pub(super) struct SshHandshakeSlot {
    active: Mutex<usize>,
    released: Condvar,
}

impl Default for SshHandshakeSlot {
    fn default() -> Self {
        SshHandshakeSlot {
            active: Mutex::new(0),
            released: Condvar::new(),
        }
    }
}

impl SshHandshakeSlot {
    fn acquire(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *active >= MAX_CONCURRENT_SSH_HANDSHAKES {
            active = self
                .released
                .wait(active)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *active += 1;
    }

    fn release(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        self.released.notify_one();
    }
}

pub(super) struct SshHandshakeGuard(Arc<SshHandshakeSlot>);

impl Drop for SshHandshakeGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// 占一个同服务器新建握手名额，守卫存活期间（即整个握手+认证期间）占用。
/// 不依赖 `connection_operation_lock`，持锁方调用不会形成等待环。
pub(super) fn acquire_ssh_handshake_slot(config: &SshConnectionConfig) -> SshHandshakeGuard {
    let key = connection_pool_key(config);
    let mut slots = ssh_handshake_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    slots.retain(|_, slot| slot.strong_count() > 0);
    let slot = slots.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
        let slot = Arc::new(SshHandshakeSlot::default());
        slots.insert(key, Arc::downgrade(&slot));
        slot
    });
    drop(slots);
    slot.acquire();
    SshHandshakeGuard(slot)
}

pub(super) fn connection_operation_lock(config: &SshConnectionConfig) -> Arc<Mutex<()>> {
    let key = connection_pool_key(config);
    let mut locks = ssh_operation_locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

fn cache_session(key: String, session: &Session) {
    if let Ok(mut pool) = ssh_connection_pool().lock() {
        if pool.len() >= MAX_CACHED_SSH_CONNECTIONS && !pool.contains_key(&key) {
            if let Some(oldest) = pool.keys().next().cloned() {
                pool.remove(&oldest);
            }
        }
        pool.insert(key, session.clone());
    }
}

pub(super) fn invalidate_cached_connection(config: &SshConnectionConfig) {
    if let Ok(mut pool) = ssh_connection_pool().lock() {
        pool.remove(&connection_pool_key(config));
    }
}

pub(super) fn with_connection_operation<T>(
    config: &SshConnectionConfig,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = connection_operation_lock(config);
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = operation();
    if result.is_err() {
        invalidate_cached_connection(config);
    }
    result
}

/// 取同一泳道上那把「一次只允许一个操作」的锁，但**带等待上限**。
///
/// 为什么不能直接 `lock()`：文件浏览器在保存/读取时都是阻塞式地等这把锁，
/// 一旦同泳道上有慢操作（大目录列举、网络抖动），保存就会无限期卡住 ——
/// 界面上表现为「保存中…」永远转圈、既没成功也没报错。调用方拿到超时错误
/// 会立刻把状态复位并提示，不会把用户锁死。超时本身不代表连接坏了，
/// 所以这里**不**清空缓存会话（真正的操作失败才清）。
pub(super) fn try_connection_operation<T>(
    config: &SshConnectionConfig,
    wait: Duration,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = connection_operation_lock(config);
    let deadline = Instant::now() + wait;
    let guard = loop {
        match lock.try_lock() {
            Ok(guard) => break Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let Some(_guard) = guard else {
        return Err(format!(
            "SSH 连接正忙：等待同一连接上的前一个操作超过 {} 秒仍未完成，请稍后重试",
            wait.as_secs()
        ));
    };
    // 这里刻意**不**在失败时清空缓存会话：浏览器这条通路上的错误大多是业务
    // 错误（版本冲突、路径不在根目录内、权限不足），连接本身是好的；清掉只会
    // 让下一次操作重新握手。真正断链由 connect 侧的存活检查负责。
    operation()
}

/// Drop a cached SSH session before a saved connection is edited or deleted.
/// The next operation performs a fresh handshake and authentication.
pub fn disconnect_ssh_connection(config: &SshConnectionConfig) {
    invalidate_cached_connection(config);
    if let Ok(mut transports) = openssh_transports().lock() {
        transports.remove(&connection_pool_key(config));
    }
}

pub(super) fn configure_session_keepalive(session: &Session, interval_secs: u64) {
    if interval_secs == 0 {
        return;
    }
    let interval_secs = interval_secs.clamp(5, 300);
    // ssh2 0.9 does not wrap libssh2's keepalive functions. The raw session
    // pointer is exposed by ssh2 for this purpose and uses the same
    // libssh2-sys version as the direct dependency.
    let mut raw = session.raw();
    unsafe {
        libssh2_sys::libssh2_keepalive_config(
            &mut *raw as *mut _,
            1,
            interval_secs as std::os::raw::c_uint,
        );
    }
}

pub(super) fn send_session_keepalive(session: &Session) -> Result<(), String> {
    let mut seconds_to_next = 0;
    let mut raw = session.raw();
    let result =
        unsafe { libssh2_sys::libssh2_keepalive_send(&mut *raw as *mut _, &mut seconds_to_next) };
    if result < 0 {
        return Err(format!("SSH 保活发送失败（错误码 {result}）"));
    }
    Ok(())
}

pub(super) fn connect(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<Session, String> {
    connect_with_cache(config, password, true)
}

pub(super) fn connect_fresh(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<Session, String> {
    connect_with_cache(config, password, false)
}

pub(super) fn connect_with_cache(
    config: &SshConnectionConfig,
    password: Option<&str>,
    allow_cached: bool,
) -> Result<Session, String> {
    let mut last_error = None;
    for attempt in 0..SSH_CONNECT_ATTEMPTS {
        match connect_with_cache_once(config, password, allow_cached) {
            Ok(session) => return Ok(session),
            Err(error)
                if attempt + 1 < SSH_CONNECT_ATTEMPTS && retryable_connection_error(&error) =>
            {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(200 + attempt as u64 * 300));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| "SSH 连接失败".to_string()))
}

pub(super) fn retryable_connection_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    ![
        "认证",
        "密码",
        "私钥",
        "known_hosts",
        "主机密钥",
        "用户名",
        "不能为空",
        "地址无效",
        "找不到",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn connect_with_cache_once(
    config: &SshConnectionConfig,
    password: Option<&str>,
    allow_cached: bool,
) -> Result<Session, String> {
    if config.host.trim().is_empty() || config.username.trim().is_empty() {
        return Err("SSH 主机和用户名不能为空".to_string());
    }
    let port = if config.port == 0 { 22 } else { config.port };
    let pool_key = connection_pool_key(config);
    // A saved password is loaded for normal Agent operations too. Reuse the
    // authenticated session for those operations; explicit connection tests
    // call connect_fresh so they still validate the supplied credential.
    if allow_cached {
        if let Some(session) = cached_alive_session(config) {
            return Ok(session);
        }
    }
    // 缓存未命中才会走到这里：真实新建握手前占同服务器并发名额（≤2），
    // 避开服务器 MaxStartups 的随机丢弃（v3 §21.4 S0）。
    let _handshake_slot = acquire_ssh_handshake_slot(config);
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
    tcp.set_read_timeout(Some(Duration::from_secs(
        config.connect_timeout_secs.clamp(3, 60),
    )))
    .map_err(ssh_error)?;
    tcp.set_write_timeout(Some(Duration::from_secs(
        config.connect_timeout_secs.clamp(3, 60),
    )))
    .map_err(ssh_error)?;
    let mut session = Session::new().map_err(ssh_error)?;
    session.set_tcp_stream(tcp);
    session.set_timeout((config.connect_timeout_secs.clamp(3, 60) * 1000) as u32);
    session.handshake().map_err(ssh_error)?;
    configure_session_keepalive(&session, config.keepalive_interval_secs);

    if config.strict_host_key {
        let known_hosts_path = config
            .known_hosts_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(expand_local_config_path)
            .or_else(default_known_hosts_path)
            .ok_or_else(|| "找不到 known_hosts 文件，请填写路径或关闭主机密钥校验".to_string())?;
        if !known_hosts_path.is_file() {
            return Err(format!(
                "找不到 known_hosts 文件：{}。可先用系统 SSH 连接一次服务器，或手动填写正确路径。",
                known_hosts_path.display()
            ));
        }
        let (host_key, _) = session
            .host_key()
            .ok_or_else(|| "SSH 主机没有提供可校验的主机密钥".to_string())?;
        let mut known_hosts = session.known_hosts().map_err(ssh_error)?;
        known_hosts
            .read_file(&known_hosts_path, ssh2::KnownHostFileKind::OpenSSH)
            .map_err(|error| format!("无法读取 known_hosts: {error}"))?;
        match known_hosts.check_port(host, port, host_key) {
            ssh2::CheckResult::Match => {}
            ssh2::CheckResult::Mismatch => {
                return Err(format!(
                    "SSH 主机密钥与 known_hosts 不匹配：{}:{}。密码变化不会改变主机密钥，请确认服务器是否重装或更换了 SSH 主机密钥。",
                    host, port
                ));
            }
            ssh2::CheckResult::NotFound => {
                return Err(format!(
                    "known_hosts 中没有 {}:{} 的主机记录，请先用系统 SSH 连接一次服务器，或关闭主机密钥校验。",
                    host, port
                ));
            }
            ssh2::CheckResult::Failure => {
                return Err("无法完成 SSH 主机密钥校验，请检查 known_hosts 格式".to_string());
            }
        }
    }

    let mut authentication_errors = Vec::new();
    if let Some(key_path) = config
        .private_key_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let key_path = expand_local_config_path(key_path);
        let passphrase = password.filter(|value| !value.is_empty());
        let private_key_result = match fs::read_to_string(&key_path) {
            Ok(contents)
                if contents
                    .trim_start()
                    .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") =>
            {
                match openssh_private_key_to_pem(&contents) {
                    Ok(pem) => match write_temporary_private_key(&pem) {
                        Ok(temporary_path) => {
                            let result = session.userauth_pubkey_file(
                                &config.username,
                                None,
                                &temporary_path,
                                passphrase,
                            );
                            let _ = fs::remove_file(&temporary_path);
                            Some(result)
                        }
                        Err(error) => {
                            authentication_errors.push(error);
                            None
                        }
                    },
                    Err(error) => {
                        authentication_errors.push(format!("SSH 私钥格式不支持: {error}"));
                        None
                    }
                }
            }
            _ => Some(session.userauth_pubkey_file(&config.username, None, &key_path, passphrase)),
        };
        if let Some(private_key_result) = private_key_result {
            match private_key_result {
                Ok(()) if session.authenticated() => {
                    if config.reuse_connection {
                        cache_session(pool_key.clone(), &session);
                    }
                    return Ok(session);
                }
                Ok(()) => authentication_errors.push("SSH 私钥认证未完成".to_string()),
                Err(error) => authentication_errors.push(format!("SSH 私钥认证失败: {error}")),
            }
        }
    }
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        match session.userauth_password(&config.username, password) {
            Ok(()) if session.authenticated() => {
                if config.reuse_connection {
                    cache_session(pool_key, &session);
                }
                return Ok(session);
            }
            Ok(()) => authentication_errors.push("SSH 密码认证未完成".to_string()),
            Err(error) => authentication_errors.push(format!("SSH 密码认证失败: {error}")),
        }
    }
    match session.userauth_agent(&config.username) {
        Ok(()) if session.authenticated() => {
            if config.reuse_connection {
                cache_session(pool_key, &session);
            }
            return Ok(session);
        }
        Ok(()) => authentication_errors.push("SSH Agent 认证未完成".to_string()),
        Err(error) => {
            authentication_errors.push(format!("SSH Agent 认证失败: {error}"));
        }
    }
    if !session.authenticated() {
        return Err(if authentication_errors.is_empty() {
            "SSH 认证未通过".to_string()
        } else {
            authentication_errors.join("；")
        });
    }
    Ok(session)
}

pub(super) fn run_blocking<T, F>(context: &SshToolContext, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Session, &SshToolContext) -> Result<T, String> + Send + 'static,
{
    cancelled(&context.cancellation)?;
    // A detached build can last hours. Never hold the shared operation lock
    // or reuse libssh2's mutable session while normal tools access the host.
    if context.command_timeout_limit > MAX_REMOTE_COMMAND_SECONDS * 1000 {
        let session = connect_fresh(&context.config, context.password.as_deref())?;
        session.set_blocking(false);
        cancelled(&context.cancellation)?;
        return operation(session, context);
    }
    let operation_lock = connection_operation_lock(&context.config);
    let mut reported_wait = false;
    let _operation_guard = loop {
        cancelled(&context.cancellation)?;
        match operation_lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Some(sink) = context.output_sink.as_ref().filter(|_| !reported_wait) {
                    sink.emit_remote_progress(json!({
                        "stage": "waiting",
                        "message": "正在等待同一 SSH 连接上的前一个操作完成",
                    }));
                }
                reported_wait = true;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
        }
    };
    cancelled(&context.cancellation)?;
    let session = connect(&context.config, context.password.as_deref())?;
    session.set_blocking(false);
    cancelled(&context.cancellation)?;
    let result = operation(session, context);
    if result.is_err() {
        invalidate_cached_connection(&context.config);
    }
    result
}

pub(super) fn spawn_remote<T, F>(
    context: SshToolContext,
    operation: F,
) -> tokio::task::JoinHandle<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(Session, &SshToolContext) -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || run_blocking(&context, operation))
}

pub(super) async fn await_remote<T: Send + 'static>(
    task: tokio::task::JoinHandle<Result<T, String>>,
) -> Result<T, String> {
    task.await
        .map_err(|error| format!("SSH 后台任务失败: {error}"))?
}
