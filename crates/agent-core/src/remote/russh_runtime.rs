//! v3 §21：russh 统一连接运行时（S1）。
//!
//! russh 是主传输；libssh2（`remote::session` 现有路径）与 ssh.exe 仅在引擎
//! 决策器标记「russh 传输级失败」后兜底（内存级记忆，随进程重启复位）。
//! 本模块与 libssh2 池**互不共享连接**：同一台服务器同一时刻只有一个引擎，
//! 选择权集中在引擎决策器，各功能不得自行探测。

use super::connection::{default_known_hosts_path, expand_local_config_path};
use super::paths::cancelled;
use super::session::russh_engine_pool_key;
use super::*;

use russh::client::{self, Handle};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

// ---------------------------------------------------------------------------
// 静态运行时（连接任务的生命周期宿主）
// ---------------------------------------------------------------------------

static RUSSH_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn russh_runtime() -> &'static tokio::runtime::Runtime {
    RUSSH_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("russh-runtime")
            .enable_all()
            .build()
            .expect("russh 运行时创建失败")
    })
}

/// 预热全局 russh 运行时（幂等）。首次初始化现在允许发生在任何线程上：
/// `russh_block_on` 自己会把异步上下文让进阻塞区，不再依赖「调用方先预热」
/// 来规避嵌套运行时。保留这个入口是因为终端桥仍按「先预热、再打开 PTY」的
/// 顺序写注释与日志。
pub fn russh_runtime_ensure_started() {
    let _ = russh_runtime();
}

/// 在静态运行时上执行异步工作。
///
/// `Runtime::block_on` 在 tokio 工作线程上会 panic「Cannot start a runtime from
/// within a runtime」，而这个 panic 会被 agent 任务的 JoinError 吞掉，最终只剩
/// 一句「Agent 任务异常终止」，栈里没有任何业务帧（线上事故：`ssh__list_local_files`
/// → `run_openssh_raw` → `russh_run_command`）。
///
/// 工具层是 async 的，而 SFTP / ssh.exe / russh 的入口全是同步的，所以这里必须
/// 自己兜住重入：先 `block_in_place` 把本线程切进阻塞区（tokio 会把该线程上的
/// 其它任务迁到别的 worker），再进入 russh 运行时。实测三种调用面——
/// 多线程 worker、`spawn_blocking` 线程、普通同步线程——都安全；唯一仍然报错的
/// 是 current_thread 运行时，那里 `block_in_place` 本身不成立，直接给出可读诊断。
fn russh_block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
                panic!(
                    "SSH 同步入口被 current_thread 运行时调用，无法切进阻塞区：\
                     请用 tokio::task::spawn_blocking 包住这次 SSH 调用。"
                );
            }
            tokio::task::block_in_place(|| russh_runtime().block_on(future))
        }
        Err(_) => russh_runtime().block_on(future),
    }
}

// ---------------------------------------------------------------------------
// 引擎决策（§21.1）：russh 传输级失败的内存级记忆
// ---------------------------------------------------------------------------

static RUSSH_TRANSPORT_FAILURES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn russh_transport_failures() -> &'static Mutex<HashSet<String>> {
    RUSSH_TRANSPORT_FAILURES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// russh 对该服务器出现传输级失败（协商/协议错误），本进程内回退 libssh2。
/// 键按引擎口径归一（剥离功能泳道后缀）——同一台服务器所有功能共用一个决策。
pub fn remember_russh_transport_failure(config: &SshConnectionConfig) {
    let key = russh_engine_pool_key(config);
    russh_transport_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key);
}

/// 手动重连/显式测试时复位：重新尝试 russh（§21.1）。
pub fn reset_russh_transport_failure(config: &SshConnectionConfig) {
    let key = russh_engine_pool_key(config);
    russh_transport_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&key);
}

pub fn russh_transport_failing(config: &SshConnectionConfig) -> bool {
    russh_transport_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&russh_engine_pool_key(config))
}

/// §21 引擎决策器的唯一出口：russh 是否负责该配置的一切传输（exec/PTY/文件
/// shell 路径）。归一化键上：①未记忆 russh 传输级失败；②未被标记 openssh
/// 兼容（libssh2 协商失败的服务器整链保持既有 ssh.exe 路径，避免二次认证）。
/// 功能泳道后缀在键上归一，浏览/对话/终端/状态天然共享同一决策与同一条连接。
pub fn russh_engine_active(config: &SshConnectionConfig) -> bool {
    !russh_transport_failing(config) && !super::connection::is_openssh_transport_marked(config)
}

/// §21.3：终端 PTY 是否走 russh 桥。
pub fn russh_pty_preferred(config: &SshConnectionConfig) -> bool {
    russh_engine_active(config)
}

/// 池内是否已有该配置的活连接（连接门控快路径用，零网络动作）。
pub fn russh_pool_alive(config: &SshConnectionConfig) -> bool {
    russh_pool()
        .lock()
        .map(|pool| {
            pool.get(&russh_engine_pool_key(config))
                .is_some_and(|connection| !connection.handle.is_closed())
        })
        .unwrap_or(false)
}

/// §21.4 S5b：该服务器当前生效的传输引擎（状态面板展示用）。
pub fn russh_active_engine_label(config: &SshConnectionConfig) -> &'static str {
    if russh_engine_active(config) {
        "russh"
    } else if super::connection::is_openssh_transport_marked(config) {
        "ssh.exe（兼容）"
    } else {
        "libssh2"
    }
}

/// §21.1 错误分类：只有协商/协议级失败才值得换引擎重试。
/// 认证拒绝（`AuthResult::Failure`）与 host key 拒绝在此之外——它们与引擎
/// 无关，换库重试只会造成二次认证（fail2ban 风险）或绕过安全判定；TCP 层
/// 失败（IO 错误）两个库同样会失败，回退无意义。
pub(crate) fn is_russh_transport_failure(error: &russh::Error) -> bool {
    matches!(
        error,
        russh::Error::UnknownAlgo
            | russh::Error::NoCommonAlgo { .. }
            | russh::Error::Kex
            | russh::Error::KexInit
    )
}

/// russh 调用失败的处置分类（§21.1）：决定调用方「降级 libssh2」还是「直接报错」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RusshErrorKind {
    /// 协商/协议级失败：已写入引擎回退记忆，本次调用立即降级。
    Transport,
    /// 认证未通过：不记忆（引擎无关），本次降级走 libssh2（还有 agent 认证路径）。
    Auth,
    /// TCP 连不上/超时：换引擎同样失败，直接报错。
    Network,
    /// 用户取消。
    Cancelled,
    /// 其余（host key 拒绝、通道异常、命令超时）：直接报错。
    Other,
}

#[derive(Debug, Clone)]
pub struct RusshCallError {
    pub kind: RusshErrorKind,
    pub message: String,
}

impl RusshCallError {
    fn new(kind: RusshErrorKind, message: impl Into<String>) -> Self {
        RusshCallError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RusshCallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

// ---------------------------------------------------------------------------
// 连接池（§21.2）：per-server 一条连接 + 通道预算
// ---------------------------------------------------------------------------

/// exec 通道并发上限（短命命令排队拿名额，替代 libssh2 路径的全串行锁）。
const RUSSH_EXEC_CHANNEL_BUDGET: usize = 4;
/// 终端 PTY 通道并发上限；超出预算的终端由调用方溢出到专用连接。
const RUSSH_PTY_CHANNEL_BUDGET: usize = 4;

struct PooledRusshConnection {
    handle: Handle<RusshClientHandler>,
    exec_permits: Arc<Semaphore>,
    pty_permits: Arc<Semaphore>,
}

static RUSSH_POOL: OnceLock<Mutex<HashMap<String, Arc<PooledRusshConnection>>>> = OnceLock::new();

fn russh_pool() -> &'static Mutex<HashMap<String, Arc<PooledRusshConnection>>> {
    RUSSH_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn invalidate_russh_connection(config: &SshConnectionConfig) {
    russh_pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&russh_engine_pool_key(config));
}

/// 取该配置的池连接；缺失或已断（`is_closed`）时新建（TCP+握手+认证）。
/// 键按引擎口径归一（剥离泳道后缀）：浏览/对话/终端共享同一条连接。
/// 必须在 russh 运行时的异步上下文内调用。
async fn get_or_connect(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<Arc<PooledRusshConnection>, RusshCallError> {
    let key = russh_engine_pool_key(config);
    {
        let pool = russh_pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(connection) = pool.get(&key).cloned() {
            if !connection.handle.is_closed() {
                return Ok(connection);
            }
        }
    }
    let connection = connect_and_authenticate(config, password).await?;
    let mut pool = russh_pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if pool.len() >= MAX_CACHED_SSH_CONNECTIONS && !pool.contains_key(&key) {
        if let Some(oldest) = pool.keys().next().cloned() {
            pool.remove(&oldest);
        }
    }
    pool.insert(key, connection.clone());
    Ok(connection)
}

// ---------------------------------------------------------------------------
// 客户端 Handler：known_hosts 校验（§21.2）
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RusshClientHandler {
    host: String,
    port: u16,
    strict_host_key: bool,
    known_hosts_path: Option<PathBuf>,
    /// host key 校验失败的原因（用于把 russh 的笼统错误还原成可读信息）。
    host_key_error: Arc<Mutex<Option<String>>>,
}

fn record_host_key_error(handler: &RusshClientHandler, message: String) {
    *handler
        .host_key_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
}

impl client::Handler for RusshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(_) => {
                record_host_key_error(
                    self,
                    "服务器使用 SSH 证书认证主机，russh 引擎暂不支持校验".to_string(),
                );
                return Ok(false);
            }
        };
        if !self.strict_host_key {
            return Ok(true);
        }
        let Some(path) = self
            .known_hosts_path
            .clone()
            .or_else(default_known_hosts_path)
        else {
            record_host_key_error(
                self,
                "找不到 known_hosts 文件，请填写路径或关闭主机密钥校验".to_string(),
            );
            return Ok(false);
        };
        if !path.is_file() {
            record_host_key_error(
                self,
                format!(
                    "找不到 known_hosts 文件：{}。可先用系统 SSH 连接一次服务器，或手动填写正确路径。",
                    path.display()
                ),
            );
            return Ok(false);
        }
        match keys::check_known_hosts_path(&self.host, self.port, key, &path) {
            Ok(true) => Ok(true),
            Ok(false) => {
                record_host_key_error(
                    self,
                    format!(
                        "known_hosts 中没有 {}:{} 的主机记录，请先用系统 SSH 连接一次服务器，或关闭主机密钥校验。",
                        self.host, self.port
                    ),
                );
                Ok(false)
            }
            Err(_) => {
                record_host_key_error(
                    self,
                    format!(
                        "SSH 主机密钥与 known_hosts 不匹配：{}:{}。请确认服务器是否重装或更换了 SSH 主机密钥。",
                        self.host, self.port
                    ),
                );
                Ok(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 连接 + 认证（§21.2）：私钥 → 密码 → keyboard-interactive
// ---------------------------------------------------------------------------

fn normalized_host(config: &SshConnectionConfig) -> String {
    config
        .host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

async fn connect_and_authenticate(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<Arc<PooledRusshConnection>, RusshCallError> {
    let host = normalized_host(config);
    let port = if config.port == 0 { 22 } else { config.port };
    if host.is_empty() || config.username.trim().is_empty() {
        return Err(RusshCallError::new(
            RusshErrorKind::Other,
            "SSH 主机和用户名不能为空",
        ));
    }
    let connect_timeout = Duration::from_secs(config.connect_timeout_secs.clamp(3, 60));
    let russh_config = client::Config {
        keepalive_interval: Some(Duration::from_secs(
            config.keepalive_interval_secs.clamp(5, 300),
        )),
        keepalive_max: 3,
        nodelay: true,
        ..client::Config::default()
    };
    let handler = RusshClientHandler {
        host: host.clone(),
        port,
        strict_host_key: config.strict_host_key,
        known_hosts_path: config
            .known_hosts_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(expand_local_config_path),
        host_key_error: Arc::new(Mutex::new(None)),
    };
    let connect = client::connect(
        Arc::new(russh_config),
        (host.as_str(), port),
        handler.clone(),
    );
    let mut handle = match tokio::time::timeout(connect_timeout, connect).await {
        Err(_) => {
            return Err(RusshCallError::new(
                RusshErrorKind::Network,
                format!("SSH 连接超时（{} 秒）", connect_timeout.as_secs()),
            ))
        }
        Ok(Err(error)) => {
            let host_key_detail = handler
                .host_key_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(detail) = host_key_detail {
                return Err(RusshCallError::new(RusshErrorKind::Other, detail));
            }
            if is_russh_transport_failure(&error) {
                remember_russh_transport_failure(config);
                return Err(RusshCallError::new(
                    RusshErrorKind::Transport,
                    format!("SSH 协商失败（已切换兼容引擎）：{error}"),
                ));
            }
            return Err(RusshCallError::new(
                RusshErrorKind::Network,
                format!("SSH 连接失败：{error}"),
            ));
        }
        Ok(Ok(handle)) => handle,
    };
    authenticate(&mut handle, config, password)
        .await
        .map_err(|error| RusshCallError::new(RusshErrorKind::Auth, error))?;
    Ok(Arc::new(PooledRusshConnection {
        handle,
        exec_permits: Arc::new(Semaphore::new(RUSSH_EXEC_CHANNEL_BUDGET)),
        pty_permits: Arc::new(Semaphore::new(RUSSH_PTY_CHANNEL_BUDGET)),
    }))
}

/// 认证顺序与 libssh2 路径一致：私钥 → 密码 → keyboard-interactive（同一密码
/// 应答全部提示，覆盖 PAM 场景）。认证拒绝**不**标记引擎回退（§21.1）。
async fn authenticate(
    handle: &mut Handle<RusshClientHandler>,
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), String> {
    let user = config.username.trim();
    let mut errors = Vec::new();
    if let Some(key_path) = config
        .private_key_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let key_path = expand_local_config_path(key_path);
        let passphrase = password.filter(|value| !value.is_empty());
        match keys::load_secret_key(&key_path, passphrase) {
            Ok(key) => {
                // RSA 私钥按 SHA2-512 → SHA2-256 顺序尝试；非 RSA 忽略哈希参数。
                let attempts: &[Option<HashAlg>] = if key.algorithm().is_rsa() {
                    &[Some(HashAlg::Sha512), Some(HashAlg::Sha256)]
                } else {
                    &[None]
                };
                for hash in attempts {
                    let with_hash = PrivateKeyWithHashAlg::new(Arc::new(key.clone()), *hash);
                    match handle.authenticate_publickey(user, with_hash).await {
                        Ok(result) if result.success() => return Ok(()),
                        Ok(_) => errors.push("SSH 公钥认证未通过".to_string()),
                        Err(error) => {
                            errors.push(format!("SSH 公钥认证失败：{error}"));
                        }
                    }
                }
            }
            Err(error) => errors.push(format!("SSH 私钥读取失败：{error}")),
        }
    }
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        match handle.authenticate_password(user, password).await {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => errors.push("SSH 密码认证未通过".to_string()),
            Err(error) => errors.push(format!("SSH 密码认证失败：{error}")),
        }
        // PAM 场景：密码方法被拒但服务器只接受 keyboard-interactive。
        let mut response = match handle
            .authenticate_keyboard_interactive_start(user, None::<String>)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                errors.push(format!("SSH 键盘交互认证失败：{error}"));
                return Err(errors.join("；"));
            }
        };
        for _ in 0..5 {
            match response {
                russh::client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
                russh::client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                    errors.push("SSH 键盘交互认证未通过".to_string());
                    break;
                }
                russh::client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = vec![password.to_string(); prompts.len()];
                    match handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                    {
                        Ok(next) => response = next,
                        Err(error) => {
                            errors.push(format!("SSH 键盘交互认证失败：{error}"));
                            break;
                        }
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        errors.push("SSH 认证未通过：未配置私钥，也未提供密码".to_string());
    }
    Err(errors.join("；"))
}

// ---------------------------------------------------------------------------
// exec 通道（S2 的核心原语）
// ---------------------------------------------------------------------------

async fn poll_cancelled(cancellation: Arc<AtomicBool>) {
    loop {
        if cancellation.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

fn append_capped(target: &mut Vec<u8>, truncated: &mut bool, data: &[u8]) {
    if target.len() >= MAX_REMOTE_COMMAND_OUTPUT_BYTES {
        *truncated = true;
        return;
    }
    let room = MAX_REMOTE_COMMAND_OUTPUT_BYTES - target.len();
    if data.len() > room {
        target.extend_from_slice(&data[..room]);
        *truncated = true;
    } else {
        target.extend_from_slice(data);
    }
}

/// 在 russh 引擎上执行一条远端命令。同步入口（供现有同步工具层调用），
/// 语义对齐 `run_ssh_workspace_command_with_input` 的 libssh2 路径：
/// stdout/stderr 各 8MB 截断、可取消、可流式输出、带退出码。
#[allow(clippy::too_many_arguments)]
pub fn russh_run_command(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
    input: Option<&[u8]>,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<SshCommandResult, RusshCallError> {
    cancelled(&cancellation)
        .map_err(|message| RusshCallError::new(RusshErrorKind::Cancelled, message))?;
    let config = config.clone();
    let password = password.map(str::to_string);
    let command = command.to_string();
    let input = input.map(<[u8]>::to_vec);
    let sink = sink.cloned();
    russh_block_on(async move {
        run_command_async(
            &config,
            password.as_deref(),
            &command,
            input.as_deref(),
            cancellation,
            timeout,
            sink.as_ref(),
        )
        .await
    })
}

async fn run_command_async(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
    input: Option<&[u8]>,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<SshCommandResult, RusshCallError> {
    // 连接与通道名额的等待都必须可取消：四个并行上传各要先跑一次 PREPARE 再拿
    // 名额，预算只有 4，排在后面的调用会长时间卡在 `acquire_owned()` 上。原来
    // 这两步没有任何取消检查，用户点「暂停」后只能等它自己排到队——表现就是
    // 「点了停止还在跑」。这里改成和收流循环同一个 150ms 取消轮询。
    let connection = {
        let cancelled = poll_cancelled(cancellation.clone());
        tokio::pin!(cancelled);
        tokio::select! {
            _ = &mut cancelled => {
                return Err(RusshCallError::new(RusshErrorKind::Cancelled, "SSH 命令已取消"));
            }
            result = get_or_connect(config, password) => result?,
        }
    };
    let _permit = {
        let cancelled = poll_cancelled(cancellation.clone());
        tokio::pin!(cancelled);
        tokio::select! {
            _ = &mut cancelled => {
                return Err(RusshCallError::new(RusshErrorKind::Cancelled, "SSH 命令已取消"));
            }
            permit = connection.exec_permits.clone().acquire_owned() => permit
                .map_err(|_| RusshCallError::new(RusshErrorKind::Other, "SSH 连接已关闭"))?,
        }
    };
    run_command_on_connection(
        config,
        &connection,
        command,
        input,
        cancellation,
        timeout,
        sink,
    )
    .await
}

/// 在既有连接上执行命令（通道打开 → exec → 收流）。测试连接（专用新连接）
/// 与池化执行共用；`connection` 由调用方持有（同时持有 exec 预算名额）。
#[allow(clippy::too_many_arguments)]
async fn run_command_on_connection(
    config: &SshConnectionConfig,
    connection: &Arc<PooledRusshConnection>,
    command: &str,
    input: Option<&[u8]>,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<&CommandOutputSink>,
) -> Result<SshCommandResult, RusshCallError> {
    let connect_timeout = Duration::from_secs(config.connect_timeout_secs.clamp(3, 60));
    let channel_open =
        tokio::time::timeout(connect_timeout, connection.handle.channel_open_session())
            .await
            .map_err(|_| {
                invalidate_russh_connection(config);
                RusshCallError::new(RusshErrorKind::Other, "打开 SSH 命令通道超时")
            })?
            .map_err(|error| {
                invalidate_russh_connection(config);
                RusshCallError::new(
                    RusshErrorKind::Other,
                    format!("打开 SSH 命令通道失败：{error}"),
                )
            })?;
    let mut channel = channel_open;
    channel.exec(true, command).await.map_err(|error| {
        invalidate_russh_connection(config);
        RusshCallError::new(RusshErrorKind::Other, format!("发送 SSH 命令失败：{error}"))
    })?;
    if let Some(input) = input {
        let mut writer = channel.make_writer();
        let bytes = input.to_vec();
        // 写完必须 `shutdown()` 发 ChannelMsg::Eof，否则远端的 `cat > file`
        // （以及任何读 stdin 到 EOF 的命令）永远收不到结束信号，通道一直挂着
        // 直到超时——这就是「不到 1KB 的文件上传要等很久」的根因。
        tokio::spawn(async move {
            let _ = writer.write_all(&bytes).await;
            let _ = writer.shutdown().await;
        });
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let mut exit_status: Option<i32> = None;
    let mut sequence = 0u64;
    let mut eof_seen = false;
    let deadline = tokio::time::Instant::now() + timeout;
    let cancellation_poll = poll_cancelled(cancellation);
    tokio::pin!(cancellation_poll);
    loop {
        tokio::select! {
            _ = &mut cancellation_poll => {
                let _ = channel.close().await;
                return Err(RusshCallError::new(RusshErrorKind::Cancelled, "SSH 命令已取消"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = channel.close().await;
                return Err(RusshCallError::new(
                    RusshErrorKind::Other,
                    format!("SSH 命令超时（{} 秒），请检查命令是否产生持续输出", timeout.as_secs()),
                ));
            }
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => {
                    if let Some(sink) = sink.filter(|sink| sink.is_continuous()) {
                        sink.emit_remote_bytes(sequence, "stdout", &data);
                        sequence = sequence.saturating_add(1);
                    }
                    append_capped(&mut stdout, &mut stdout_truncated, &data);
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if let Some(sink) = sink.filter(|sink| sink.is_continuous()) {
                        sink.emit_remote_bytes(sequence, "stderr", &data);
                        sequence = sequence.saturating_add(1);
                    }
                    append_capped(&mut stderr, &mut stderr_truncated, &data);
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => {
                    exit_status = Some(status as i32);
                }
                Some(ChannelMsg::Eof) => {
                    eof_seen = true;
                }
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
        if eof_seen && exit_status.is_some() {
            // OpenSSH 在 Eof 后还会补 Close，但状态已齐，提前收工避免依赖服务器行为。
            break;
        }
    }
    Ok(SshCommandResult {
        exit_code: exit_status.unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        output_truncated: stdout_truncated || stderr_truncated,
    })
}

// ---------------------------------------------------------------------------
// PTY 通道（S4 的核心原语）
// ---------------------------------------------------------------------------

enum PtyControl {
    Resize(u32, u32),
    Close,
}

/// PTY 会话回调：数据块（含 PTY 的 stderr 合并流）与关闭事件（携带远端退出码）。
pub struct RusshPtyCallbacks {
    pub on_chunk: Box<dyn Fn(Vec<u8>) + Send + 'static>,
    pub on_closed: Box<dyn Fn(Option<u32>) + Send + 'static>,
}

/// 已建立的 russh PTY 会话。持有它会保活连接与 PTY 通道预算名额；drop 即关闭。
pub struct RusshPtySession {
    writer: tokio::sync::Mutex<Box<dyn tokio::io::AsyncWrite + Unpin + Send>>,
    control: tokio::sync::mpsc::UnboundedSender<PtyControl>,
    _pty_permit: OwnedSemaphorePermit,
    _connection: Arc<PooledRusshConnection>,
}

/// 打开 russh PTY 会话。`startup` 提供时走「PTY + exec 启动命令」（等价
/// ssh.exe `-tt remote-command`，如 `cd … && exec $SHELL -li`），否则走交互
/// shell。同步入口，回调在 russh 运行时上触发，不得在回调里做阻塞工作。
pub fn russh_open_pty(
    config: &SshConnectionConfig,
    password: Option<&str>,
    cols: u16,
    rows: u16,
    startup: Option<&str>,
    callbacks: RusshPtyCallbacks,
) -> Result<Arc<RusshPtySession>, RusshCallError> {
    let config = config.clone();
    let password = password.map(str::to_string);
    let startup = startup.map(str::to_string);
    russh_block_on(async move {
        open_pty_async(
            &config,
            password.as_deref(),
            cols,
            rows,
            startup.as_deref(),
            callbacks,
        )
        .await
    })
}

async fn open_pty_async(
    config: &SshConnectionConfig,
    password: Option<&str>,
    cols: u16,
    rows: u16,
    startup: Option<&str>,
    callbacks: RusshPtyCallbacks,
) -> Result<Arc<RusshPtySession>, RusshCallError> {
    // 池内 PTY 预算（≤4）满时溢出到该服务器的专用连接（不进池，会话关闭即断），
    // 而不是让第 5 个终端排队等待（§21.2）。
    let (connection, pty_permit) = {
        let pooled = get_or_connect(config, password).await?;
        match pooled.pty_permits.clone().try_acquire_owned() {
            Ok(permit) => (pooled, permit),
            Err(_) => {
                let dedicated = connect_and_authenticate(config, password).await?;
                let permit = dedicated
                    .pty_permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| RusshCallError::new(RusshErrorKind::Other, "SSH 连接已关闭"))?;
                (dedicated, permit)
            }
        }
    };
    let connect_timeout = Duration::from_secs(config.connect_timeout_secs.clamp(3, 60));
    let channel_open =
        tokio::time::timeout(connect_timeout, connection.handle.channel_open_session())
            .await
            .map_err(|_| {
                invalidate_russh_connection(config);
                RusshCallError::new(RusshErrorKind::Other, "打开 SSH 终端通道超时")
            })?
            .map_err(|error| {
                invalidate_russh_connection(config);
                RusshCallError::new(
                    RusshErrorKind::Other,
                    format!("打开 SSH 终端通道失败：{error}"),
                )
            })?;
    let channel = channel_open;
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(|error| {
            invalidate_russh_connection(config);
            RusshCallError::new(
                RusshErrorKind::Other,
                format!("申请 SSH 终端 PTY 失败：{error}"),
            )
        })?;
    let launch_error = |error: russh::Error| {
        invalidate_russh_connection(config);
        RusshCallError::new(
            RusshErrorKind::Other,
            format!("启动 SSH 远端 shell 失败：{error}"),
        )
    };
    if let Some(startup) = startup {
        channel.exec(true, startup).await.map_err(launch_error)?;
    } else {
        channel.request_shell(true).await.map_err(launch_error)?;
    }
    let writer = channel.make_writer();
    let (control_sender, control_receiver) = tokio::sync::mpsc::unbounded_channel();
    russh_runtime().spawn(pty_reader_loop(channel, control_receiver, callbacks));
    Ok(Arc::new(RusshPtySession {
        writer: tokio::sync::Mutex::new(Box::new(writer)),
        control: control_sender,
        _pty_permit: pty_permit,
        _connection: connection,
    }))
}

async fn pty_reader_loop(
    mut channel: Channel<client::Msg>,
    mut control: tokio::sync::mpsc::UnboundedReceiver<PtyControl>,
    callbacks: RusshPtyCallbacks,
) {
    let mut exit_status = None;
    loop {
        tokio::select! {
            command = control.recv() => match command {
                Some(PtyControl::Resize(cols, rows)) => {
                    let _ = channel.window_change(cols, rows, 0, 0).await;
                }
                Some(PtyControl::Close) | None => break,
            },
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => (callbacks.on_chunk)(data.to_vec()),
                // PTY 模式下远端 stderr 合并进主输出流，与 ConPTY 行为一致。
                Some(ChannelMsg::ExtendedData { data, .. }) => (callbacks.on_chunk)(data.to_vec()),
                Some(ChannelMsg::ExitStatus { exit_status: status }) => {
                    exit_status = Some(status);
                }
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
        }
    }
    (callbacks.on_closed)(exit_status);
}

impl RusshPtySession {
    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let data = data.to_vec();
        russh_block_on(async {
            let mut writer = self.writer.lock().await;
            writer
                .write_all(&data)
                .await
                .map_err(|error| format!("SSH 终端输入失败：{error}"))
        })
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self
            .control
            .send(PtyControl::Resize(cols as u32, rows as u32));
    }

    pub fn close(&self) {
        let _ = self.control.send(PtyControl::Close);
    }
}

// ---------------------------------------------------------------------------
// 连接测试（§21.4 S5a：与引擎决策对齐）
// ---------------------------------------------------------------------------

/// 显式「测试连接」的 russh 通道：强制全新连接（不吃池缓存，校验的就是
/// 当前凭据），跑一条校验命令后即弃。调用方负责先复位引擎记忆。
pub fn russh_test_connection(
    config: &SshConnectionConfig,
    password: Option<&str>,
) -> Result<(), RusshCallError> {
    let config = config.clone();
    let password = password.map(str::to_string);
    russh_block_on(async move {
        let connection = connect_and_authenticate(&config, password.as_deref()).await?;
        // 校验命令与 libssh2 路径同款（printf android-tool-ssh-ok）。
        run_command_on_connection(
            &config,
            &connection,
            "printf android-tool-ssh-ok",
            None,
            std::sync::Arc::new(AtomicBool::new(false)),
            Duration::from_secs(DEFAULT_REMOTE_COMMAND_SECONDS),
            None,
        )
        .await
        .and_then(|result| {
            if result.exit_code == 0 && result.stdout.trim() == "android-tool-ssh-ok" {
                Ok(())
            } else {
                Err(RusshCallError::new(
                    RusshErrorKind::Other,
                    "SSH 已连接，但远程命令验证失败",
                ))
            }
        })
    })
}

// ---------------------------------------------------------------------------
// 测试：进程内 russh 假服务器（§21.4 S1 回归面）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::PrivateKey;
    use russh::server::{self, Auth, Server as _};
    use std::sync::mpsc as std_mpsc;
    use std::time::Instant;

    fn cancellation_flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn test_config(port: u16) -> SshConnectionConfig {
        SshConnectionConfig {
            id: format!("russh-test-{port}"),
            name: "russh-test".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            username: "tester".to_string(),
            private_key_path: None,
            known_hosts_path: None,
            strict_host_key: false,
            default_root: None,
            reuse_connection: true,
            connect_timeout_secs: 5,
            operation_timeout_secs: 30,
            keepalive_interval_secs: 15,
        }
    }

    #[derive(Clone, Copy)]
    enum FakeMode {
        /// exec 请求：回 stdout-ok / stderr-warn / 退出码 0；命令含 "BIG" 时发超限输出。
        Exec,
        /// shell + PTY：把收到的数据加前缀回显。
        PtyEcho,
        /// exec + 消费 stdin：**收到 ChannelMsg::Eof 之前不返回退出码**，模拟
        /// `cat > file`。客户端少发一次 EOF 就会一直挂到超时——这是上传「小文件
        /// 却要等很久」的回归面。
        ExecStdinUntilEof,
        /// exec 接受后**永不回应**（远端卡在一个不会结束的命令）。用来验证取消
        /// 能真正打断等待中的命令。
        ExecHangs,
    }

    #[derive(Clone)]
    struct FakeServerHolder {
        password: Arc<String>,
        mode: FakeMode,
    }

    impl server::Server for FakeServerHolder {
        type Handler = FakeSession;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            FakeSession {
                password: self.password.clone(),
                mode: self.mode,
                shell_ready: false,
                received_stdin: 0,
            }
        }
        fn handle_session_error(&mut self, _error: russh::Error) {}
    }

    struct FakeSession {
        password: Arc<String>,
        mode: FakeMode,
        shell_ready: bool,
        received_stdin: usize,
    }

    impl server::Handler for FakeSession {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            password: &str,
        ) -> Result<Auth, Self::Error> {
            if *self.password == password {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            let _ = channel.id();
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: russh::ChannelId,
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.shell_ready = true;
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            let command = String::from_utf8_lossy(data).to_string();
            if matches!(self.mode, FakeMode::Exec) {
                if command.contains("BIG") {
                    let chunk = vec![b'x'; 64 * 1024];
                    for _ in 0..200 {
                        session.data(channel, chunk.clone())?;
                    }
                } else {
                    session.data(channel, "stdout-ok")?;
                    session.extended_data(channel, 1, "stderr-warn")?;
                }
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }

        async fn channel_eof(
            &mut self,
            channel: russh::ChannelId,
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            // `cat > file` 语义：只有收到 EOF 才结束。
            if matches!(self.mode, FakeMode::ExecStdinUntilEof) {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            if matches!(self.mode, FakeMode::ExecStdinUntilEof) {
                self.received_stdin += data.len();
            }
            if matches!(self.mode, FakeMode::PtyEcho) && self.shell_ready {
                let echo = format!("ECHO:{}", String::from_utf8_lossy(data));
                session.data(channel, echo.into_bytes())?;
            }
            Ok(())
        }
    }
    /// 在 russh 静态运行时上起一个进程内假 SSH 服务器，返回监听端口。
    fn start_fake_server(password: &str, mode: FakeMode) -> u16 {
        let (sender, receiver) = std_mpsc::channel::<u16>();
        let password = password.to_string();
        russh_runtime().spawn(async move {
            // 固定的丢弃型测试密钥（ssh-keygen 一次性生成，仅用于进程内假服务器）。
            // 不用 PrivateKey::random：它要求的 rand_core 版本与本 crate 的 rand 0.9
            // 存在 trait 实现缺口。
            const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBonxYwdjv/jQgKG1AyF+9Fse5FT0Y2SNfvUa8o9BXTVQAAAJjfYnOr32Jz\nqwAAAAtzc2gtZWQyNTUxOQAAACBonxYwdjv/jQgKG1AyF+9Fse5FT0Y2SNfvUa8o9BXTVQ\nAAAEAEXjFwsARFFXbpQMSQiPWUdEOqP3SNysfIdac4FDjEe2ifFjB2O/+NCAobUDIX70Wx\n7kVPRjZI1+9Rryj0FdNVAAAAEDk1MjMzQGZhbmd6aXh1YW4BAgMEBQ==\n-----END OPENSSH PRIVATE KEY-----";
            let config = server::Config {
                keys: vec![PrivateKey::from_openssh(TEST_HOST_KEY)
                    .expect("内置测试主机密钥应可解析")],
                auth_rejection_time: Duration::from_millis(1),
                auth_rejection_time_initial: Some(Duration::from_millis(1)),
                ..server::Config::default()
            };
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("测试监听应绑定成功");
            let port = listener
                .local_addr()
                .expect("测试监听应有本地地址")
                .port();
            let mut holder = FakeServerHolder {
                password: Arc::new(password),
                mode,
            };
            let server = holder.run_on_socket(Arc::new(config), &listener);
            let _ = sender.send(port);
            let _ = server.await;
        });
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("测试服务器应启动")
    }

    /// 回归：「暂停」必须能打断一个卡在等待远端输出的命令。
    ///
    /// 假服务器接受 exec 后永不回应（远端卡在一个不会结束的命令）。取消标志置位
    /// 后，`run_command_on_connection` 的 150ms 轮询应该关掉通道并返回
    /// `Cancelled`，而不是一直挂到超时——这是「点了暂停还在执行」的回归面。
    #[test]
    fn russh_cancellation_interrupts_a_waiting_command() {
        let port = start_fake_server("pw-cancel", FakeMode::ExecHangs);
        let config = test_config(port);
        let flag = cancellation_flag();
        let canceller = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let error = russh_run_command(
            &config,
            Some("pw-cancel"),
            "sleep 600",
            None,
            flag,
            Duration::from_secs(30),
            None,
        )
        .expect_err("取消后应返回错误而不是等满超时");
        assert!(
            matches!(error.kind, RusshErrorKind::Cancelled),
            "实际错误：{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "取消应在轮询周期内生效，实际耗时 {:?}",
            started.elapsed()
        );
    }

    /// 回归：「暂停」必须能打断**排队等通道名额**的调用。
    ///
    /// 四个并行上传（截图里的现场）会各自先跑一次 PREPARE 再抢 exec 名额，预算
    /// 只有 4。名额被占满时后续调用卡在 `acquire_owned()` 上，修复前那里没有取
    /// 消检查，用户点停止只能等它排到队——表现就是「点了暂停还在转」。这里把
    /// 预算全部占住，断言取消能在轮询周期内返回 `Cancelled`。
    #[test]
    fn russh_cancellation_interrupts_a_queued_command() {
        let port = start_fake_server("pw-queue", FakeMode::Exec);
        let config = test_config(port);
        // 先连上并把 4 个 exec 名额全部握在手里，模拟名额耗尽。
        let held = russh_block_on({
            let config = config.clone();
            async move {
                let connection = get_or_connect(&config, Some("pw-queue"))
                    .await
                    .expect("假服务器应能连上");
                let mut permits = Vec::new();
                for index in 0..RUSSH_EXEC_CHANNEL_BUDGET {
                    permits.push(
                        connection
                            .exec_permits
                            .clone()
                            .acquire_owned()
                            .await
                            .unwrap_or_else(|_| panic!("第 {index} 个名额应能拿到")),
                    );
                }
                permits
            }
        });
        let flag = cancellation_flag();
        let canceller = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let error = russh_run_command(
            &config,
            Some("pw-queue"),
            "sleep 600",
            None,
            flag,
            Duration::from_secs(30),
            None,
        )
        .expect_err("名额排满时取消应直接返回错误");
        assert!(
            matches!(error.kind, RusshErrorKind::Cancelled),
            "实际错误：{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "排队阶段的取消应在轮询周期内生效，实际耗时 {:?}",
            started.elapsed()
        );
        drop(held);
    }

    /// 回归：写 stdin 之后必须发 EOF（`AsyncWrite::shutdown`）。
    ///
    /// 假服务器在收到 EOF 之前不返回退出码，等价于远端的 `cat > file`。修复前
    /// 客户端只 `write_all` 不 `shutdown`，通道会一直挂到超时——表现为「不到 1KB
    /// 的文件上传要等很久」。这里断言命令在远短于超时的时间内就返回成功。
    #[test]
    fn russh_stdin_write_ends_with_eof() {
        let port = start_fake_server("pw-eof", FakeMode::ExecStdinUntilEof);
        let config = test_config(port);
        let started = std::time::Instant::now();
        let result = russh_run_command(
            &config,
            Some("pw-eof"),
            "cat > /tmp/f",
            Some(b"hardkey"),
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect("写完 stdin 应立刻结束");
        assert_eq!(result.exit_code, 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "发送 EOF 后应立即返回，实际耗时 {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn russh_exec_round_trips_with_password_auth() {
        let port = start_fake_server("pw-correct", FakeMode::Exec);
        let config = test_config(port);
        let result = russh_run_command(
            &config,
            Some("pw-correct"),
            "printf ok",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect("exec 应成功");
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("stdout-ok"));
        assert!(result.stderr.contains("stderr-warn"));
        assert!(!result.output_truncated);
    }

    /// 约定回归：空闲连接直接复用、被占用才新建。第一条命令结束后连接必须
    /// 留在池里；第二条命令拿到的连接句柄与 PTY 通道互不影响（PTY 预算独立，
    /// 不抢 exec 名额）。
    #[test]
    fn russh_pool_reuses_idle_connection_and_isolates_channels() {
        let port = start_fake_server("pw-correct", FakeMode::Exec);
        let config = test_config(port);
        russh_run_command(
            &config,
            Some("pw-correct"),
            "warm",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect("预热命令应成功");
        assert!(russh_pool_alive(&config), "命令结束后连接应留在池里复用");
        // 池内该服务器的连接对象只有一个：并发的两条命令共享同一连接、各占
        // 一个通道名额，不会为第二条命令重新握手（错密码也能过）。
        let handles: Vec<_> = (0..2)
            .map(|_| {
                std::thread::spawn({
                    let config = config.clone();
                    move || {
                        russh_run_command(
                            &config,
                            Some("pw-wrong-on-reconnect"),
                            "concurrent",
                            None,
                            cancellation_flag(),
                            Duration::from_secs(20),
                            None,
                        )
                    }
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .expect("并发 exec 线程不应 panic")
                .expect("并发 exec 应复用空闲连接");
        }
    }

    #[test]
    fn russh_exec_pool_reuses_authenticated_connection() {
        let port = start_fake_server("pw-correct", FakeMode::Exec);
        let config = test_config(port);
        let first = russh_run_command(
            &config,
            Some("pw-correct"),
            "one",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect("第一次 exec 应成功");
        assert!(first.stdout.contains("stdout-ok"));
        // 第二次走池路径（不再重新认证）。假服务器拒绝错误密码，若重新认证会失败。
        let second = russh_run_command(
            &config,
            Some("pw-wrong-on-reconnect"),
            "two",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect("池命中应跳过认证直接复用连接");
        assert!(second.stdout.contains("stdout-ok"));
    }

    #[test]
    fn russh_auth_failure_is_not_transport_fallback() {
        let port = start_fake_server("pw-correct", FakeMode::Exec);
        let config = test_config(port);
        let error = russh_run_command(
            &config,
            Some("pw-wrong"),
            "true",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect_err("认证应失败");
        assert!(
            error.message.contains("认证"),
            "认证错误应可读：{}",
            error.message
        );
        assert_eq!(error.kind, RusshErrorKind::Auth);
        assert!(
            !russh_transport_failing(&config),
            "认证失败不得标记引擎回退"
        );
    }

    #[test]
    fn russh_connect_refused_is_not_transport_fallback() {
        // 借一个临时端口然后立刻释放，制造「连接被拒绝」。
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("临时监听应成功");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let config = test_config(port);
        let error = russh_run_command(
            &config,
            Some("pw"),
            "true",
            None,
            cancellation_flag(),
            Duration::from_secs(20),
            None,
        )
        .expect_err("连接应失败");
        assert_eq!(error.kind, RusshErrorKind::Network, "实际错误：{error}");
        assert!(
            !russh_transport_failing(&config),
            "网络层失败不得标记引擎回退"
        );
    }

    #[test]
    fn russh_engine_memory_set_and_reset() {
        let config = test_config(1);
        remember_russh_transport_failure(&config);
        assert!(russh_transport_failing(&config));
        reset_russh_transport_failure(&config);
        assert!(!russh_transport_failing(&config));
    }

    #[test]
    fn russh_exec_truncates_oversized_output() {
        let port = start_fake_server("pw", FakeMode::Exec);
        let config = test_config(port);
        let result = russh_run_command(
            &config,
            Some("pw"),
            "cat BIG",
            None,
            cancellation_flag(),
            Duration::from_secs(60),
            None,
        )
        .expect("超限输出命令应执行成功");
        assert!(result.output_truncated, "超过 8MB 应标记截断");
        assert_eq!(
            result.stdout.len(),
            MAX_REMOTE_COMMAND_OUTPUT_BYTES,
            "stdout 应恰好截到上限"
        );
    }

    #[test]
    fn russh_pty_round_trip() {
        let port = start_fake_server("pw", FakeMode::PtyEcho);
        let config = test_config(port);
        let (chunk_sender, chunk_receiver) = std_mpsc::channel::<Vec<u8>>();
        let (closed_sender, closed_receiver) = std_mpsc::channel::<Option<u32>>();
        let callbacks = RusshPtyCallbacks {
            on_chunk: Box::new(move |data| {
                let _ = chunk_sender.send(data);
            }),
            on_closed: Box::new(move |exit| {
                let _ = closed_sender.send(exit);
            }),
        };
        let session =
            russh_open_pty(&config, Some("pw"), 80, 24, None, callbacks).expect("PTY 会话应建立");
        session.write(b"hello\r").expect("PTY 写入应成功");
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut received = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match chunk_receiver.recv_timeout(remaining) {
                Ok(chunk) => {
                    received.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&received).contains("ECHO:hello") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(
            String::from_utf8_lossy(&received).contains("ECHO:hello"),
            "PTY 应回显输入，实际收到：{:?}",
            String::from_utf8_lossy(&received)
        );
        session.close();
        let exit = closed_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("关闭回调应触发");
        assert_eq!(exit, None);
    }
}
