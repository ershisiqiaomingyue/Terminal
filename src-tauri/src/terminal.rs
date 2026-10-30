use agent_core::{
    is_forbidden_destructive_command_line, russh_open_pty, russh_pty_preferred,
    russh_runtime_ensure_started, RusshErrorKind, RusshPtyCallbacks, SshAskPass,
    FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{ssh, wsl};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalTarget {
    Local {
        shell: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    Wsl {
        #[serde(rename = "profileId")]
        profile_id: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    Ssh {
        #[serde(rename = "connectionId")]
        connection_id: String,
        #[serde(default)]
        cwd: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionInfo {
    pub session_id: String,
    pub target_kind: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputEvent {
    session_id: String,
    data: String,
    sequence: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalStatusEvent {
    session_id: String,
    status: String,
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalExitEvent {
    session_id: String,
    code: Option<u32>,
    success: bool,
    /// russh 桥的意外断开可一键重连（重开通道 + 输出缓冲重放）。
    #[serde(default)]
    reconnectable: bool,
}

/// 终端会话的传输载体：本地 PTY 对（本机/WSL/ssh.exe 兜底）或 russh
/// PTY 通道（v3 §21.3，SSH 主引擎——与对话/文件共享同一条连接）。
enum SessionTransport {
    LocalPty {
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    },
    RusshPty(std::sync::Arc<agent_core::RusshPtySession>),
}

struct TerminalSession {
    owner: Mutex<String>,
    output: std::sync::Arc<Mutex<crate::terminal_output::TerminalOutput>>,
    transport: Mutex<SessionTransport>,
    respond_to_terminal_queries: bool,
    /// SSH 终端所属的连接 ID（断开确认弹窗统计受影响终端数用，v3 §21.4 S5c）。
    ssh_connection_id: Option<String>,
    // Keeps the helper alive until the SSH child exits.
    _ssh_askpass: Option<SshAskPass>,
}

struct TerminalLaunch {
    command: CommandBuilder,
    ssh_askpass: Option<SshAskPass>,
}

impl TerminalSession {
    fn stop(&self) {
        if let Ok(mut transport) = self.transport.lock() {
            match &mut *transport {
                SessionTransport::LocalPty { child, .. } => {
                    let _ = child.kill();
                }
                SessionTransport::RusshPty(session) => session.close(),
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
pub struct TerminalState {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
}

impl TerminalState {
    fn insert(&self, id: String, session: Arc<TerminalSession>) -> Result<(), String> {
        self.sessions
            .lock()
            .map_err(|_| "终端会话状态已损坏".to_string())?
            .insert(id, session);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Arc<TerminalSession>, String> {
        self.sessions
            .lock()
            .map_err(|_| "终端会话状态已损坏".to_string())?
            .get(id)
            .cloned()
            .ok_or_else(|| "终端会话不存在或已经关闭".to_string())
    }

    fn remove(&self, id: &str) -> Result<Option<Arc<TerminalSession>>, String> {
        Ok(self
            .sessions
            .lock()
            .map_err(|_| "终端会话状态已损坏".to_string())?
            .remove(id))
    }
}

fn next_session_id() -> String {
    let sequence = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    format!("terminal-{}-{}", std::process::id(), sequence)
}

fn target_kind(target: &TerminalTarget) -> &'static str {
    match target {
        TerminalTarget::Local { .. } => "local",
        TerminalTarget::Wsl { .. } => "wsl",
        TerminalTarget::Ssh { .. } => "ssh",
    }
}

fn normalized_shell(shell: &str) -> Result<&'static str, String> {
    match shell.trim().to_ascii_lowercase().as_str() {
        "pwsh" => Ok("pwsh"),
        "powershell" | "powershell.exe" => Ok("powershell"),
        "cmd" | "cmd.exe" => Ok("cmd"),
        _ => Err("终端只支持 pwsh、PowerShell 或 cmd".to_string()),
    }
}

/// PowerShell 的 OSC 7 上报脚本（UTF-16LE + base64，避开引号与代码页问题）。
///
/// 为什么需要它：本地 shell 默认不发 OSC 7，终端内核就没有 `onPwdChange`，
/// 右栏文件面板只能一直停在打开时的目录——看起来就是「不跟随 cd」。
/// 这段脚本只在**第一次**执行时挂钩 `prompt`，之后每次提示符都先写出
/// `ESC ] 7 ; file://HOST/D:/x/y ESC \`，再把控制权交回原 `prompt`。
const PWSH_PWD_HOOK: &str = concat!(
    "$global:__atPrompt = $function:prompt; ",
    "function global:prompt { ",
    "try { ",
    "$p = (Get-Location).ProviderPath; ",
    "if ($p) { ",
    "$e = [char]27; ",
    "Write-Host -NoNewline ($e + \"]7;file://\" + $env:COMPUTERNAME + \"/\" + ($p -replace \"\\\\\", \"/\") + $e + \"\\\") ",
    "} ",
    "} catch { }; ",
    "if ($global:__atPrompt) { & $global:__atPrompt } else { \"PS> \" } ",
    "}",
);

/// `-NoExit -EncodedCommand` 的 base64（UTF-16LE，带 BOM 不需要）。
fn encoded_pwd_hook() -> String {
    let mut bytes = Vec::with_capacity(PWSH_PWD_HOOK.len() * 2);
    for unit in PWSH_PWD_HOOK.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    BASE64_STANDARD.encode(&bytes)
}

fn local_command(shell: &str) -> Result<CommandBuilder, String> {
    let shell = normalized_shell(shell)?;
    let mut command = if shell == "pwsh" {
        CommandBuilder::new("pwsh.exe")
    } else if shell == "powershell" {
        CommandBuilder::new("powershell.exe")
    } else {
        CommandBuilder::new("cmd.exe")
    };
    if shell == "pwsh" {
        command.args(["-NoLogo", "-NoExit", "-NoProfile"]);
        command.args(["-EncodedCommand", &encoded_pwd_hook()]);
    } else if shell == "powershell" {
        command.args(["-NoLogo", "-NoExit", "-NoProfile"]);
        command.args(["-EncodedCommand", &encoded_pwd_hook()]);
    } else {
        // cmd.exe 没有可挂钩的 prompt 函数，只能带 `$E]7;...` 的 PROMPT 变量，
        // 但它拿不到当前目录的绝对路径，所以本地 cmd 不做 cwd 跟随。
        command.args(["/Q"]);
    }
    Ok(command)
}

fn linux_shell_args(shell: &str) -> Result<Vec<String>, String> {
    let shell = shell.trim().to_ascii_lowercase();
    if !matches!(shell.as_str(), "bash" | "sh" | "zsh") {
        return Err("Linux 终端只支持 bash、sh 或 zsh".to_string());
    }
    Ok(vec!["-l".to_string(), "-i".to_string()])
}

fn expand_path(value: &str) -> PathBuf {
    let mut value = value.trim().to_string();
    if let Ok(home) = env::var("USERPROFILE").or_else(|_| env::var("HOME")) {
        value = value
            .replace("%USERPROFILE%", &home)
            .replace("%USERPROFILE%", &home)
            .replace("$HOME", &home);
        if value == "~" {
            value = home;
        } else if value.starts_with("~/") || value.starts_with("~\\") {
            value = format!("{}{}", home, &value[1..]);
        }
    }
    PathBuf::from(value)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn normalize_remote_root(value: &str) -> String {
    value.trim().replace('\\', "/")
}

/// 让远端交互式 shell 每次画提示符前上报当前目录（OSC 7）。
///
/// 为什么需要它：`-li` 是**登录** shell，bash 只读 `~/.bash_profile` / `~/.profile`，
/// 不读 `~/.bashrc`；而把 OSC 7 写进 `PROMPT_COMMAND` 的习惯几乎都放在 `.bashrc` 里。
/// 于是一条远端 `cd` 之后终端内核永远收不到 `PWD_CHANGED`，右栏文件树就停在打开时的目录，
/// 看起来就是「文件树不跟随终端」。
/// 这里把同一段钩子做成环境变量带进登录 shell：`sh` 先导出，`exec` 出去的 shell 继承它。
/// 用户的 profile 若整段覆盖 `PROMPT_COMMAND`，钩子会失效，但不会破坏任何原有行为。
const REMOTE_PWD_HOOK: &str = concat!(
    "PROMPT_COMMAND='",
    "printf \"\\033]7;file://%s%s\\033\\\\\" \"${HOSTNAME:-localhost}\" \"$PWD\"",
    "'; export PROMPT_COMMAND; ",
);

fn ssh_terminal_startup(root: &str) -> String {
    let root = normalize_remote_root(root);
    // Expand home shortcuts on the remote host, while quoting the path suffix literally.
    let path = if matches!(root.as_str(), "~" | "$HOME" | "${HOME}") {
        "\"$HOME\"".to_string()
    } else if let Some(suffix) = root
        .strip_prefix("~/")
        .or_else(|| root.strip_prefix("$HOME/"))
        .or_else(|| root.strip_prefix("${HOME}/"))
    {
        format!("\"$HOME\"/{}", shell_quote(suffix))
    } else {
        shell_quote(&root)
    };
    // 钩子必须排在 `exec` 之前：`sh` 先把它导出到环境里，`exec` 出去的 shell 才继承得到。
    format!("cd -- {path} && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li")
}

fn ssh_command(
    app: &AppHandle,
    connection_id: &str,
    cwd: Option<&str>,
) -> Result<TerminalLaunch, String> {
    let (config, saved_ssh_password) = ssh::load_connection_for_agent(app, connection_id.trim())?;
    let ssh_askpass = saved_ssh_password
        .filter(|password| !password.is_empty())
        .map(|password| SshAskPass::new(&password))
        .transpose()?;
    let mut command = CommandBuilder::new("ssh.exe");
    // OpenSSH forwards TERM in its remote PTY request; zsh cannot redraw a
    // colored prompt correctly when Windows leaves it unset (TERM=dumb).
    command.env("TERM", "xterm-256color");
    command.args(["-tt", "-p", &config.port.max(1).to_string()]);
    command.args(["-o", "BatchMode=no"]);
    if let Some(askpass) = &ssh_askpass {
        command.env("SSH_ASKPASS", askpass.program_path());
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", "terminal");
    }
    command.args([
        "-o",
        &format!(
            "ConnectTimeout={}",
            config.connect_timeout_secs.clamp(3, 60)
        ),
    ]);
    command.args([
        "-o",
        &format!(
            "ServerAliveInterval={}",
            config.keepalive_interval_secs.clamp(5, 300)
        ),
    ]);
    command.args(["-o", "ServerAliveCountMax=3"]);
    if config.strict_host_key {
        if let Some(path) = config
            .known_hosts_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            command.args([
                "-o",
                &format!("UserKnownHostsFile={}", expand_path(path).display()),
            ]);
        }
    } else {
        command.args(["-o", "StrictHostKeyChecking=no"]);
        command.args([
            "-o",
            if cfg!(windows) {
                "UserKnownHostsFile=NUL"
            } else {
                "UserKnownHostsFile=/dev/null"
            },
        ]);
    }
    if let Some(path) = config
        .private_key_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        command.args(["-i", &expand_path(path).display().to_string()]);
    }
    command.arg(format!("{}@{}", config.username, config.host));
    if let Some(root) = cwd
        .filter(|value| !value.trim().is_empty())
        .or(config.default_root.as_deref())
    {
        command.arg(ssh_terminal_startup(root));
    }
    Ok(TerminalLaunch {
        command,
        ssh_askpass,
    })
}

fn wsl_command(
    app: &AppHandle,
    profile_id: &str,
    cwd: Option<&str>,
) -> Result<CommandBuilder, String> {
    let profile = wsl::load_profile_for_agent(app, profile_id.trim())?;
    let root = cwd
        .filter(|value| !value.trim().is_empty())
        .or(profile.default_root.as_deref())
        .unwrap_or("/");
    let args = linux_shell_args(&profile.shell)?;
    let mut command = CommandBuilder::new("wsl.exe");
    command.args([
        "--distribution",
        &profile.distribution,
        "--cd",
        root,
        "--exec",
        &profile.shell,
    ]);
    command.args(args);
    Ok(command)
}

fn build_command(app: &AppHandle, target: &TerminalTarget) -> Result<TerminalLaunch, String> {
    match target {
        TerminalTarget::Local { shell, cwd } => {
            let mut command = local_command(shell)?;
            // Only the local shell runs on Windows and can see the locally
            // configured toolchain; SSH and WSL are Linux-side and must not
            // receive Windows paths.
            for (name, value) in crate::runtime_env::child_environment_variables() {
                command.env(name, value);
            }
            for (name, value) in agent_core::child_proxy_environment() {
                command.env(name, value);
            }
            if let Some(cwd) = cwd.as_deref().filter(|value| !value.trim().is_empty()) {
                command.cwd(expand_path(cwd));
            }
            Ok(TerminalLaunch {
                command,
                ssh_askpass: None,
            })
        }
        TerminalTarget::Wsl { profile_id, cwd } => Ok(TerminalLaunch {
            command: wsl_command(app, profile_id, cwd.as_deref())?,
            ssh_askpass: None,
        }),
        TerminalTarget::Ssh { connection_id, cwd } => {
            ssh_command(app, connection_id, cwd.as_deref())
        }
    }
}

fn emit_status(app: &AppHandle, session_id: &str, status: &str, message: Option<String>) {
    let _ = app.emit(
        "terminal:status",
        TerminalStatusEvent {
            session_id: session_id.to_string(),
            status: status.to_string(),
            message,
        },
    );
}

fn respond_to_terminal_queries(writer: &mut dyn Write, data: &str) {
    let mut response = String::new();
    // CPR（ESC[6n）不能在这里代答：前端内核（xterm.js / Turbo）收到输出流里的
    // 光标查询时会自己把**真实**位置写回输入通道，后端再补一份固定位置
    // `ESC[1;1R` 就是重复应答——PSReadLine 会把多出来的那份当作未识别按键，
    // 用户看到的正是"本地 pwsh 第一个按键响一声系统提示音"（SSH 走 russh
    // 没有这条代答路径，因此从不出问题）。5n / DA 同理风险较低，先保留。
    if data.contains("\u{1b}[5n") {
        response.push_str("\u{1b}[0n");
    }
    if data.contains("\u{1b}[c") {
        response.push_str("\u{1b}[?1;2c");
    }
    if response.is_empty() {
        return;
    }
    let _ = writer.write_all(response.as_bytes());
    let _ = writer.flush();
}

fn start_reader(
    app: &AppHandle,
    session_id: String,
    session: Arc<TerminalSession>,
) -> Result<(), String> {
    let app = app.clone();
    let mut reader = {
        let transport = session
            .transport
            .lock()
            .map_err(|_| "终端主控状态已损坏".to_string())?;
        match &*transport {
            SessionTransport::LocalPty { master, .. } => master
                .try_clone_reader()
                .map_err(|error| format!("无法读取终端输出：{error}"))?,
            // russh 会话由回调推送输出，不存在本地 reader。
            SessionTransport::RusshPty(_) => return Ok(()),
        }
    };
    std::thread::Builder::new()
        .name(format!("terminal-reader-{session_id}"))
        .spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        let data = String::from_utf8_lossy(&buffer[..size]).to_string();
                        if session.respond_to_terminal_queries {
                            if let Ok(mut transport) = session.transport.lock() {
                                if let SessionTransport::LocalPty { writer, .. } = &mut *transport {
                                    respond_to_terminal_queries(writer.as_mut(), &data);
                                }
                            }
                        }
                        let chunk = match session.output.lock() {
                            Ok(mut output) => output.append(data),
                            Err(_) => break,
                        };
                        let _ = app.emit(
                            "terminal:output",
                            TerminalOutputEvent {
                                session_id: session_id.clone(),
                                data: chunk.data,
                                sequence: chunk.sequence,
                            },
                        );
                    }
                    Err(error) => {
                        emit_status(
                            &app,
                            &session_id,
                            "error",
                            Some(format!("终端读取失败：{error}")),
                        );
                        break;
                    }
                }
            }
            let result = session.transport.lock().ok().and_then(|mut transport| {
                match &mut *transport {
                    SessionTransport::LocalPty { child, .. } => child.wait().ok(),
                    SessionTransport::RusshPty(_) => None,
                }
            });
            // The reader owns the session until exit. A natural shell exit
            // must release PTY handles and replay buffers even if the UI tab stays open.
            let _ = app.state::<TerminalState>().remove(&session_id);
            let _ = app.emit(
                "terminal:exit",
                TerminalExitEvent {
                    session_id,
                    code: result.as_ref().map(|status| status.exit_code()),
                    success: result.as_ref().is_some_and(|status| status.success()),
                    reconnectable: false,
                },
            );
        })
        .map_err(|error| format!("无法启动终端读取线程：{error}"))?;
    Ok(())
}

/// v3 §21.3：SSH 终端的 russh PTY 桥。成功返回会话；引擎降级（Transport/Auth，
/// 前者已写入回退记忆）返回 None 由调用方落回 ConPTY+ssh.exe；网络/取消类
/// 错误直接报错（ssh.exe 同样连不上，避免双倍连接超时）。
fn try_open_russh_terminal(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    session_id: &str,
    connection_id: &str,
    cwd: Option<&str>,
    cols: u16,
    rows: u16,
) -> Result<Option<Arc<TerminalSession>>, String> {
    let (config, saved_password) = ssh::load_connection_for_agent(app, connection_id.trim())?;
    if !russh_pty_preferred(&config) {
        return Ok(None);
    }
    let root = cwd
        .filter(|value| !value.trim().is_empty())
        .or(config.default_root.as_deref());
    let startup = root.map(ssh_terminal_startup);
    let output: Arc<Mutex<crate::terminal_output::TerminalOutput>> =
        Arc::new(Mutex::new(crate::terminal_output::TerminalOutput::default()));
    let output_for_chunks = output.clone();
    let app_for_chunks = app.clone();
    let session_id_owned = session_id.to_string();
    let app_for_close = app.clone();
    let session_id_for_close = session_id.to_string();
    let callbacks = RusshPtyCallbacks {
        on_chunk: Box::new(move |data: Vec<u8>| {
            let data = String::from_utf8_lossy(&data).to_string();
            let Some(chunk) = output_for_chunks
                .lock()
                .ok()
                .map(|mut buffer| buffer.append(data))
            else {
                return;
            };
            let _ = app_for_chunks.emit(
                "terminal:output",
                TerminalOutputEvent {
                    session_id: session_id_owned.clone(),
                    data: chunk.data,
                    sequence: chunk.sequence,
                },
            );
        }),
        on_closed: Box::new(move |exit_code: Option<u32>| {
            // 通道关闭（自然退出或断连）：释放会话槽并广播 exit；reconnectable
            // 让前端保留页签缓冲，断线时可一键重开通道。
            let _ = app_for_close
                .state::<TerminalState>()
                .remove(&session_id_for_close);
            let _ = app_for_close.emit(
                "terminal:exit",
                TerminalExitEvent {
                    session_id: session_id_for_close.clone(),
                    code: exit_code,
                    success: exit_code == Some(0),
                    reconnectable: true,
                },
            );
        }),
    };
    match russh_open_pty(
        &config,
        saved_password.as_deref(),
        cols,
        rows,
        startup.as_deref(),
        callbacks,
    ) {
        Ok(pty_session) => {
            let session = Arc::new(TerminalSession {
                owner: Mutex::new(window.label().into()),
                output,
                transport: Mutex::new(SessionTransport::RusshPty(pty_session)),
                respond_to_terminal_queries: false,
                ssh_connection_id: Some(connection_id.trim().to_string()),
                _ssh_askpass: None,
            });
            Ok(Some(session))
        }
        Err(error) => match error.kind {
            RusshErrorKind::Transport | RusshErrorKind::Auth => Ok(None),
            RusshErrorKind::Cancelled
            | RusshErrorKind::Network
            | RusshErrorKind::Other => Err(error.to_string()),
        },
    }
}

/// 打开一个终端会话。
///
/// 必须是 `async`：Tauri 的同步命令跑在主线程上，而这里要建 SSH 连接、加载凭证、
/// 启动 ConPTY —— 远端不可达时一次就要等十几秒，整个界面会一起卡住（对话页右上角
/// 打开终端后「没有任何反应、也打不进字」就是这个原因：会话还没建好，前端拿不到
/// session_id，输入全被丢掉）。`async` 命令由 Tauri 放到工作线程执行，主线程继续
/// 处理消息。
#[tauri::command]
pub async fn open_terminal_session(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, TerminalState>,
    target: TerminalTarget,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalSessionInfo, String> {
    let session_id = next_session_id();
    let size = PtySize {
        rows: rows.unwrap_or(30).clamp(5, 300),
        cols: cols.unwrap_or(100).clamp(20, 500),
        pixel_width: 0,
        pixel_height: 0,
    };
    // v3 §21.3：SSH 目标先走 russh PTY 桥（与对话/文件共享连接池）；
    // 引擎降级或本地目标落回 ConPTY。
    if let TerminalTarget::Ssh {
        connection_id,
        cwd,
    } = &target
    {
        // 池化终端的完整调用面（复用空闲连接、被占用才新建）允许在 tokio 工作线程
        // 上执行 `Runtime::block_on`：命中已建好的全局运行时时它只是 `enter()`
        // 分支，不会嵌套运行时。前提是运行时**已经**建好，所以这里先预热——
        // 否则首次初始化会真的去创建嵌套运行时。
        russh_runtime_ensure_started();
        let session = {
            let app = app.clone();
            let window = window.clone();
            let session_id = session_id.clone();
            let connection_id = connection_id.clone();
            let cwd = cwd.clone();
            tauri::async_runtime::spawn_blocking(move || {
                try_open_russh_terminal(
                    &app,
                    &window,
                    &session_id,
                    &connection_id,
                    cwd.as_deref(),
                    size.cols,
                    size.rows,
                )
            })
            .await
            .map_err(|error| format!("无法连接远程终端：{error}"))??
        };
        if let Some(session) = session {
            state.insert(session_id.clone(), session)?;
            emit_status(&app, &session_id, "connected", None);
            return Ok(TerminalSessionInfo {
                session_id,
                target_kind: "ssh".to_string(),
                status: "connected".to_string(),
            });
        }
    }
    let pty = native_pty_system();
    let pair = pty
        .openpty(size)
        .map_err(|error| format!("无法创建终端：{error}"))?;
    let launch = build_command(&app, &target)?;
    let child = pair
        .slave
        .spawn_command(launch.command)
        .map_err(|error| format!("无法启动终端：{error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("无法打开终端输入：{error}"))?;
    let session = Arc::new(TerminalSession {
        owner: Mutex::new(window.label().into()),
        output: Arc::new(Mutex::new(crate::terminal_output::TerminalOutput::default())),
        transport: Mutex::new(SessionTransport::LocalPty {
            master: pair.master,
            writer,
            child,
        }),
        respond_to_terminal_queries: matches!(
            target,
            TerminalTarget::Local { .. } | TerminalTarget::Wsl { .. }
        ),
        ssh_connection_id: match &target {
            TerminalTarget::Ssh { connection_id, .. } => Some(connection_id.trim().to_string()),
            _ => None,
        },
        _ssh_askpass: launch.ssh_askpass,
    });
    state.insert(session_id.clone(), session.clone())?;
    if let Err(error) = start_reader(&app, session_id.clone(), session) {
        let _ = state.remove(&session_id);
        return Err(error);
    }
    emit_status(&app, &session_id, "connected", None);
    Ok(TerminalSessionInfo {
        session_id,
        target_kind: target_kind(&target).to_string(),
        status: "connected".to_string(),
    })
}

#[tauri::command]
pub fn read_terminal_output(
    state: State<'_, TerminalState>,
    session_id: String,
    after_sequence: u64,
) -> Result<Vec<crate::terminal_output::OutputChunk>, String> {
    let session = state.get(&session_id)?;
    let output = session.output.lock().map_err(|_| "终端输出状态不可用")?;
    Ok(output.since(after_sequence))
}

#[tauri::command]
pub fn write_terminal_input(
    state: State<'_, TerminalState>,
    session_id: String,
    input: String,
) -> Result<(), String> {
    if input
        .split(['\n', '\r'])
        .any(|line| is_forbidden_destructive_command_line(line))
    {
        return Err(FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
    }
    let session = state.get(session_id.trim())?;
    let mut transport = session
        .transport
        .lock()
        .map_err(|_| "终端输入状态已损坏".to_string())?;
    match &mut *transport {
        SessionTransport::LocalPty { writer, .. } => {
            writer
                .write_all(input.as_bytes())
                .map_err(|error| format!("终端输入失败：{error}"))?;
            writer
                .flush()
                .map_err(|error| format!("终端输入刷新失败：{error}"))
        }
        SessionTransport::RusshPty(ssh_session) => ssh_session.write(input.as_bytes()),
    }
}

/**
 * AI 生成命令的专用写入通道（开发方案 10.1 第三层防线 / T4-6）。
 *
 * 与 `write_terminal_input` 分开的三个理由：
 * 1. `approved` 是显式参数：AI 草稿没有经过用户确认就不该能落到 PTY，
 *    普通键盘输入不存在"确认"这一步，混在同一个命令里就没法区分；
 * 2. 后端要**独立**再跑一次破坏性检查——前端分级可能被绕过或漏判，
 *    这是最后一道闸门；
 * 3. 便于审计与限流，同时不把 AI 路径的开销压到普通按键输入上。
 */
#[tauri::command]
pub fn write_ai_terminal_input(
    state: State<'_, TerminalState>,
    session_id: String,
    input: String,
    approved: bool,
) -> Result<(), String> {
    if !approved {
        return Err("AI 命令未经用户确认，已拒绝执行".to_string());
    }
    if input
        .split(['\n', '\r'])
        .any(|line| is_forbidden_destructive_command_line(line))
    {
        return Err(FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string());
    }
    write_terminal_input(state, session_id, input)
}

#[tauri::command]
pub fn resize_terminal_session(    state: State<'_, TerminalState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = state.get(session_id.trim())?;
    let transport = session
        .transport
        .lock()
        .map_err(|_| "终端主控状态已损坏".to_string())?;
    match &*transport {
        SessionTransport::LocalPty { master, .. } => master
            .resize(PtySize {
                rows: rows.clamp(5, 300),
                cols: cols.clamp(20, 500),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("终端大小调整失败：{error}")),
        // SSH 窗口变更走通道级 window-change，无需本地 PTY。
        SessionTransport::RusshPty(ssh_session) => {
            ssh_session.resize(cols.clamp(20, 500), rows.clamp(5, 300));
            Ok(())
        }
    }
}

#[tauri::command]
pub fn interrupt_terminal_session(
    state: State<'_, TerminalState>,
    session_id: String,
) -> Result<(), String> {
    write_terminal_input(state, session_id, "\u{3}".to_string())
}

#[tauri::command]
pub fn close_terminal_session(
    window: tauri::WebviewWindow,
    state: State<'_, TerminalState>,
    session_id: String,
) -> Result<(), String> {
    if let Ok(session) = state.get(session_id.trim()) {
        if *session.owner.lock().map_err(|_| "终端所属窗口不可用")? != window.label() {
            return Ok(());
        }
    }
    if let Some(session) = state.remove(session_id.trim())? {
        session.stop();
    }
    Ok(())
}

pub(crate) fn transfer_window_terminals(
    app: &AppHandle,
    source: &str,
    destination: &str,
    state: &serde_json::Value,
) -> Result<(), String> {
    let Some(tabs) = state
        .pointer("/aosp-terminals/tabs")
        .or_else(|| state.pointer("/shell/tabs"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    let runtime = app.state::<TerminalState>();
    for tab in tabs {
        if let Some(id) = tab.get("sessionId").and_then(serde_json::Value::as_str) {
            if let Ok(session) = runtime.get(id) {
                let mut owner = session.owner.lock().map_err(|_| "终端所属窗口不可用")?;
                if *owner == source {
                    *owner = destination.into();
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn close_window_terminals(app: &AppHandle, label: &str) {
    let runtime = app.state::<TerminalState>();
    if let Ok(mut sessions) = runtime.sessions.lock() {
        sessions.retain(|_, session| {
            let owned = session
                .owner
                .lock()
                .map(|owner| *owner == label)
                .unwrap_or(false);
            if owned {
                session.stop();
            }
            !owned
        });
    };
}

/// v3 §21.4 S5c：统计某台服务器当前的活跃终端数（编辑/删除连接前的确认弹窗用）。
#[tauri::command]
pub fn count_active_ssh_terminals(
    state: State<'_, TerminalState>,
    connection_id: String,
) -> Result<u64, String> {
    let id = connection_id.trim();
    if id.is_empty() {
        return Ok(0);
    }
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "终端会话状态已损坏".to_string())?;
    Ok(sessions
        .values()
        .filter(|session| {
            session
                .ssh_connection_id
                .as_deref()
                .is_some_and(|value| value == id)
        })
        .count() as u64)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{
        encoded_pwd_hook, local_command, normalize_remote_root, respond_to_terminal_queries,
        ssh_terminal_startup, BASE64_STANDARD, REMOTE_PWD_HOOK,
    };
    use base64::Engine as _;
    use portable_pty::{native_pty_system, PtySize};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// 测试专用：替前端内核回答游标位置查询（CPR）。
    ///
    /// 生产环境里这一步由 xterm/Turbo 在渲染线程上完成；无头 PTY 里没人回，
    /// PSReadLine 会卡在「先报一下我在哪」上，命令与提示符都不会出现。
    fn answer_cursor_queries(writer: &Arc<Mutex<Box<dyn Write + Send>>>, data: &str) {
        if !data.contains("\u{1b}[6n") {
            return;
        }
        let mut guard = writer.lock().expect("PTY 写端应可用");
        let _ = guard.write_all(b"\x1b[1;1R");
        let _ = guard.flush();
    }

    #[test]
    fn local_power_shell_executes_a_command_from_pty_input() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("PTY should open");
        let mut child = pair
            .slave
            .spawn_command(local_command("pwsh").expect("pwsh command should be valid"))
            .expect("PowerShell should start");
        let writer = Arc::new(Mutex::new(
            pair.master.take_writer().expect("PTY writer should open"),
        ));
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("PTY reader should open");

        writer
            .lock()
            .expect("PTY writer lock should open")
            .write_all(b"Write-Output terminal-pty-test\r\n")
            .expect("PTY input should be writable");
        writer
            .lock()
            .expect("PTY writer lock should open")
            .flush()
            .expect("PTY input should flush");

        let (sender, receiver) = mpsc::channel();
        let response_writer = writer.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        let data = String::from_utf8_lossy(&buffer[..size]).to_string();
                        // 无头 PTY 没有前端内核，PSReadLine 的 CPR（`ESC[6n`）没人代答
                        // 就会一直等，命令根本不执行——历史上这条测试正是因此红着。
                        answer_cursor_queries(&response_writer, &data);
                        respond_to_terminal_queries(&mut *response_writer.lock().expect("PTY 写端应可用"), &data);
                        if sender.send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = String::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(chunk) => output.push_str(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout)
                | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
            if output.contains("terminal-pty-test") {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            output.contains("terminal-pty-test"),
            "PowerShell output did not contain the command result: {output:?}"
        );
    }

    #[test]
    fn local_shell_hook_reports_osc7_working_directory() {
        // 代价：终端内核只有在收到 OSC 7 时才会触发 onPwdChange，右栏文件面板才
        // 能跟随 cd。本地 shell 默认不发，只能靠注入 prompt 钩子补上。
        let encoded = encoded_pwd_hook();
        let bytes = BASE64_STANDARD
            .decode(encoded.as_bytes())
            .expect("钩子必须是合法 base64");
        assert_eq!(bytes.len() % 2, 0, "UTF-16LE 的字节数必须是偶数");
        let script: String = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .map(|unit| char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}'))
            .collect();
        assert!(script.contains("]7;file://"), "必须是 OSC 7：{script}");
        assert!(script.contains("Get-Location"), "必须报当前目录：{script}");
        assert!(script.contains("$function:prompt"), "必须保留原提示符：{script}");
        // 原样单引号/反斜杠转义出错会在这里露出来——PowerShell 里 `"\\"` 是两个字符。
        assert!(script.contains(r#"-replace "\\", "/""#), "反斜杠替换写错了：{script}");
    }

    #[test]
    fn local_power_shell_command_carries_the_pwd_hook() {
        for shell in ["pwsh", "powershell"] {
            let command = local_command(shell).expect("PWZ 命令应当合法");
            let debug = format!("{command:?}");
            assert!(debug.contains("-EncodedCommand"), "{shell} 缺 OSC 7 钩子：{debug}");
            assert!(debug.contains("-NoExit"), "{shell} 必须留在交互模式：{debug}");
        }
        // cmd.exe 没有可挂钩的 prompt 函数，不能塞进同样的参数（会直接启动失败）。
        let cmd = format!("{:?}", local_command("cmd").expect("cmd 命令应当合法"));
        assert!(!cmd.contains("-EncodedCommand"), "cmd 不应带 PowerShell 参数：{cmd}");
    }

    #[test]
    fn local_shell_hook_emits_osc7_inside_a_real_pty() {
        // 端到端：真正开一个 ConPTY 跑注入后的 pwsh，看输出流里有没有 OSC 7。
        // 之所以要自己回 `ESC[6n`（CPR）：无头 PTY 里没有前端内核代答，PSReadLine
        // 会一直等光标位置，后面的提示符与 OSC 7 都不会出现。
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("PTY should open");
        let mut child = pair
            .slave
            .spawn_command(local_command("pwsh").expect("pwsh command should be valid"))
            .expect("PowerShell should start");
        let writer = Arc::new(Mutex::new(
            pair.master.take_writer().expect("PTY writer should open"),
        ));
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("PTY reader should open");

        let (sender, receiver) = mpsc::channel();
        let response_writer = writer.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        let data = String::from_utf8_lossy(&buffer[..size]).to_string();
                        if data.contains("\u{1b}[6n") {
                            let mut guard = response_writer.lock().expect("PTY 写端应可用");
                            let _ = guard.write_all(b"\x1b[1;1R");
                            let _ = guard.flush();
                        }
                        if data.contains("\u{1b}[5n") {
                            let mut guard = response_writer.lock().expect("PTY 写端应可用");
                            let _ = guard.write_all(b"\x1b[0n");
                            let _ = guard.flush();
                        }
                        if sender.send(data).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // 让第一个提示符出来（OSC 7 就是它写出来的）。
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut output = String::new();
        while Instant::now() < deadline && !output.contains("]7;file://") {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(chunk) => output.push_str(&chunk),
                Err(_) => break,
            }
        }
        let first_prompt_saw_osc7 = output.contains("]7;file://");

        // 再 cd 一次，确认钩子是「跟着目录变」而不是开机写一次。
        output.clear();
        {
            let mut guard = writer.lock().expect("PTY writer lock should open");
            let _ = guard.write_all(b"cd C:\\\\Windows\r\n");
            let _ = guard.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut after_cd = String::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(chunk) => after_cd.push_str(&chunk),
                Err(_) => break,
            }
            if after_cd.contains("]7;file://") && after_cd.to_ascii_lowercase().contains("windows") {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            first_prompt_saw_osc7,
            "首个提示符没有发出 OSC 7，钩子没生效：{output:?}"
        );
        assert!(
            after_cd.contains("]7;file://"),
            "cd 之后没有再发 OSC 7：{after_cd:?}"
        );
        assert!(
            after_cd.to_ascii_lowercase().contains("windows"),
            "OSC 7 没跟上新目录：{after_cd:?}"
        );
    }

    #[test]
    fn normalizes_windows_separators_for_remote_terminal_roots() {
        assert_eq!(
            normalize_remote_root(" /home/app1whu\\workspace "),
            "/home/app1whu/workspace"
        );
    }

    #[test]
    fn ssh_terminal_expands_remote_home_shortcuts() {
        for root in ["~", " ~ ", "$HOME", "${HOME}"] {
            assert_eq!(
                ssh_terminal_startup(root),
                format!("cd -- \"$HOME\" && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li")
            );
        }
        for root in [
            "~/work space",
            "~\\work space",
            "$HOME/work space",
            "${HOME}/work space",
        ] {
            assert_eq!(
                ssh_terminal_startup(root),
                format!(
                    "cd -- \"$HOME\"/'work space' && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li"
                )
            );
        }
    }

    #[test]
    fn ssh_terminal_quotes_literal_paths_and_shell_metacharacters() {
        assert_eq!(
            ssh_terminal_startup("/tmp/work space"),
            format!("cd -- '/tmp/work space' && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li")
        );
        assert_eq!(
            ssh_terminal_startup("~/it's $(printf injected);dir"),
            format!(
                "cd -- \"$HOME\"/'it'\\''s $(printf injected);dir' && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li"
            )
        );
        assert_eq!(
            ssh_terminal_startup("-relative"),
            format!("cd -- '-relative' && {REMOTE_PWD_HOOK}exec \"${{SHELL:-bash}}\" -li")
        );
    }

    /// 钩子里的 OSC 7 是远端 shell 用 `printf` 发出来的，两处都不能写错：
    /// 序列必须以 ESC `]` 开头（不是字面量的反斜杠+033），结尾必须是 `ESC \`。
    #[test]
    fn remote_pwd_hook_emits_an_osc7_sequence_before_each_prompt() {
        assert!(REMOTE_PWD_HOOK.contains("\\033]7;file://%s%s\\033\\\\"));
        assert!(REMOTE_PWD_HOOK.ends_with("export PROMPT_COMMAND; "));
        // 钩子塞在 `sh` 的一段命令里，不能出现会提前结束外层引号的裸单引号。
        assert_eq!(REMOTE_PWD_HOOK.matches('\'').count(), 2);
        // 整条启动命令里钩子必须排在 exec 前面，否则导出的变量到不了新 shell。
        let startup = ssh_terminal_startup("/srv");
        let hook_at = startup.find(REMOTE_PWD_HOOK).expect("hook missing");
        let exec_at = startup.find("exec ").expect("exec missing");
        assert!(hook_at < exec_at);
    }
}
