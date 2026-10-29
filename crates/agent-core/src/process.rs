use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

pub const MAX_COMMAND_OUTPUT_CHARS: usize = 64 * 1024;
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 120_000;
const MAX_LIVE_OUTPUT_CHARS: usize = MAX_COMMAND_OUTPUT_CHARS * 2;
const MAX_LIVE_OUTPUT_CHUNKS: usize = 512;
const MAX_COMMAND_DETAIL_CHARS: usize = MAX_COMMAND_OUTPUT_CHARS * 2;
const MAX_COMMAND_CAPTURE_BYTES: usize = MAX_COMMAND_OUTPUT_CHARS * 4 + 4;
pub(crate) const PROCESS_CLEANUP_ERROR_PREFIX: &str = "[process_cleanup_unconfirmed] ";

/// Creates a command for a console-subsystem helper (rg.exe, taskkill.exe, ...)
/// that must never allocate a console window.
///
/// The Tauri host is a GUI-subsystem process with no console of its own. A child
/// created with the default creation flags therefore makes Windows allocate a
/// brand-new console window, which is the black `cmd` flash that shows up while
/// the agent searches: redirecting stdio does not change it, only
/// `CREATE_NO_WINDOW` does.
pub(crate) fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

pub type CommandOutputListener = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Default)]
struct LiveOutputBudget {
    chars: usize,
    chunks: usize,
    truncated: bool,
}

#[derive(Clone)]
pub struct CommandOutputSink {
    listener: CommandOutputListener,
    session_id: String,
    turn_id: String,
    tool_name: String,
    call_id: String,
    budget: Arc<Mutex<LiveOutputBudget>>,
    continuous: bool,
    pending_utf8: Arc<Mutex<[Vec<u8>; 2]>>,
}

impl CommandOutputSink {
    pub fn new(
        listener: CommandOutputListener,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        Self {
            listener,
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            tool_name: tool_name.into(),
            call_id: String::new(),
            budget: Arc::new(Mutex::new(LiveOutputBudget::default())),
            continuous: false,
            pending_utf8: Arc::new(Mutex::new([Vec::new(), Vec::new()])),
        }
    }

    pub fn with_tool(&self, tool_name: impl Into<String>) -> Self {
        Self {
            listener: self.listener.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            tool_name: tool_name.into(),
            call_id: self.call_id.clone(),
            budget: Arc::new(Mutex::new(LiveOutputBudget::default())),
            continuous: self.continuous,
            pending_utf8: Arc::new(Mutex::new([Vec::new(), Vec::new()])),
        }
    }

    pub fn with_call_id(&self, call_id: impl Into<String>) -> Self {
        Self {
            listener: self.listener.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            tool_name: self.tool_name.clone(),
            call_id: call_id.into(),
            budget: Arc::new(Mutex::new(LiveOutputBudget::default())),
            continuous: self.continuous,
            pending_utf8: Arc::new(Mutex::new([Vec::new(), Vec::new()])),
        }
    }

    fn emit(&self, payload: Value) {
        (self.listener)(payload);
    }

    pub fn emit_started(&self, command: &str, args: &[String], cwd: &Path) {
        self.emit(json!({
            "type": "command_output",
            "phase": "started",
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "call_id": (!self.call_id.is_empty()).then_some(self.call_id.clone()),
            "command": command,
            "args": args,
            "cwd": cwd.to_string_lossy(),
        }));
    }

    fn emit_chunk(&self, sequence: u64, stream: CommandOutputStream, text: &str) {
        if self.continuous {
            self.emit(json!({"phase": "chunk", "stream": stream.as_str(), "text": text, "output_sequence": sequence}));
            return;
        }
        let Ok(mut budget) = self.budget.lock() else {
            return;
        };
        if budget.truncated {
            return;
        }
        let remaining = MAX_LIVE_OUTPUT_CHARS.saturating_sub(budget.chars);
        let chunk_limit_reached = budget.chunks >= MAX_LIVE_OUTPUT_CHUNKS || remaining == 0;
        let mut visible = text.chars().take(remaining).collect::<String>();
        let truncated = chunk_limit_reached || visible.chars().count() < text.chars().count();
        if truncated {
            visible.push_str("\n[实时输出已截断]");
            budget.truncated = true;
        }
        budget.chars = budget.chars.saturating_add(visible.chars().count());
        budget.chunks = budget.chunks.saturating_add(1);
        drop(budget);
        self.emit(json!({
            "type": "command_output",
            "phase": "chunk",
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "call_id": (!self.call_id.is_empty()).then_some(self.call_id.clone()),
            "output_sequence": sequence,
            "stream": stream.as_str(),
            "text": visible,
        }));
    }

    /// Emit output produced by a non-local process while preserving the same
    /// command timeline contract used by local tools.
    pub fn emit_remote_chunk(&self, sequence: u64, stream: &str, text: &str) {
        let stream = if stream.eq_ignore_ascii_case("stderr") {
            CommandOutputStream::Stderr
        } else {
            CommandOutputStream::Stdout
        };
        self.emit_chunk(sequence, stream, text);
    }

    /// Desktop build logs persist output to disk and must keep streaming after
    /// the bounded agent transcript/capture has filled up.
    pub fn continuous(mut self) -> Self {
        self.continuous = true;
        self
    }

    pub fn is_continuous(&self) -> bool {
        self.continuous
    }

    pub fn emit_remote_bytes(&self, sequence: u64, stream: &str, bytes: &[u8]) {
        let Ok(mut pending) = self.pending_utf8.lock() else {
            return;
        };
        let buffer = &mut pending[usize::from(stream == "stderr")];
        buffer.extend_from_slice(bytes);
        let valid_len = match std::str::from_utf8(buffer) {
            Ok(_) => buffer.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => buffer.len(),
        };
        if valid_len > 0 {
            self.emit_remote_chunk(
                sequence,
                stream,
                &String::from_utf8_lossy(&buffer[..valid_len]),
            );
            buffer.drain(..valid_len);
        }
    }

    /// Emit structured progress for a remote operation that is not a shell
    /// command. The desktop uses this to render remote search and directory
    /// traversal without exposing the provider-facing JSON payload.
    pub fn emit_remote_progress(&self, progress: Value) {
        self.emit(json!({
            "type": "command_output",
            "phase": "progress",
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "call_id": (!self.call_id.is_empty()).then_some(self.call_id.clone()),
            "progress": progress,
        }));
    }

    fn emit_finished(&self, termination: Option<&str>, summary: Value, details: Value) {
        self.emit(json!({
            "type": "command_output",
            "phase": "finished",
            "session_id": self.session_id,
            "turn_id": self.turn_id,
            "tool_name": self.tool_name,
            "call_id": (!self.call_id.is_empty()).then_some(self.call_id.clone()),
            "termination": termination,
            "summary": summary,
            "details": details,
        }));
    }

    pub fn emit_remote_finished(&self, termination: Option<&str>, summary: Value, details: Value) {
        self.emit_finished(termination, summary, details);
    }
}

#[derive(Clone, Copy)]
enum CommandOutputStream {
    Stdout,
    Stderr,
}

impl CommandOutputStream {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

enum ProcessOutputMessage {
    Chunk {
        stream: CommandOutputStream,
        bytes: Vec<u8>,
    },
    Finished(CommandOutputStream),
    Error {
        stream: CommandOutputStream,
        message: String,
    },
}

#[derive(Default)]
struct CommandOutputAccumulator {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_capture_truncated: bool,
    stderr_capture_truncated: bool,
    ordered_output: Vec<Value>,
    ordered_chars: usize,
    ordered_truncated: bool,
    next_sequence: u64,
    reader_error: Option<String>,
}

pub struct CommandExecutionResult {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_capture_truncated: bool,
    pub stderr_capture_truncated: bool,
    pub ordered_output: Vec<Value>,
    pub ordered_truncated: bool,
}

async fn read_process_stream<R>(
    stream: CommandOutputStream,
    mut reader: R,
    sender: mpsc::Sender<ProcessOutputMessage>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buffer = vec![0u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(length) => {
                if sender
                    .send(ProcessOutputMessage::Chunk {
                        stream,
                        bytes: buffer[..length].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender
                    .send(ProcessOutputMessage::Error {
                        stream,
                        message: error.to_string(),
                    })
                    .await;
                break;
            }
        }
    }
    let _ = sender.send(ProcessOutputMessage::Finished(stream)).await;
}

fn append_captured_output(target: &mut Vec<u8>, bytes: &[u8], truncated: &mut bool) {
    let remaining = MAX_COMMAND_CAPTURE_BYTES.saturating_sub(target.len());
    if bytes.len() > remaining {
        target.extend_from_slice(&bytes[..remaining]);
        *truncated = true;
    } else {
        target.extend_from_slice(bytes);
    }
}

fn append_ordered_output(
    accumulator: &mut CommandOutputAccumulator,
    stream: CommandOutputStream,
    bytes: &[u8],
    sink: Option<&CommandOutputSink>,
) {
    let sequence = accumulator.next_sequence;
    accumulator.next_sequence = accumulator.next_sequence.saturating_add(1);
    let text = String::from_utf8_lossy(bytes).into_owned();
    if let Some(sink) = sink {
        sink.emit_chunk(sequence, stream, &text);
    }
    if accumulator.ordered_chars >= MAX_COMMAND_DETAIL_CHARS {
        accumulator.ordered_truncated = true;
        return;
    }
    let remaining = MAX_COMMAND_DETAIL_CHARS - accumulator.ordered_chars;
    let detail_text = text.chars().take(remaining).collect::<String>();
    accumulator.ordered_chars += detail_text.chars().count();
    accumulator.ordered_truncated |= detail_text.chars().count() < text.chars().count();
    accumulator.ordered_output.push(json!({
        "sequence": sequence,
        "stream": stream.as_str(),
        "text": detail_text,
    }));
}

pub fn truncate_output_bytes(value: &mut String, max_bytes: usize) -> bool {
    if value.len() <= max_bytes {
        return false;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    true
}

pub fn truncate_command_output(value: &str) -> (String, bool) {
    let mut output = value
        .chars()
        .take(MAX_COMMAND_OUTPUT_CHARS)
        .collect::<String>();
    let truncated = value.chars().count() > MAX_COMMAND_OUTPUT_CHARS;
    if truncated {
        output.push_str("\n[输出已截断]");
    }
    (output, truncated)
}

fn command_output_text(bytes: &[u8], captured_truncated: bool) -> (String, bool) {
    let (mut text, truncated) = truncate_command_output(&String::from_utf8_lossy(bytes));
    let truncated = truncated || captured_truncated;
    if captured_truncated && !text.ends_with("\n[输出已截断]") {
        text.push_str("\n[输出已截断]");
    }
    (text, truncated)
}

fn command_output_details(accumulator: &CommandOutputAccumulator) -> Value {
    let (stdout, stdout_truncated) =
        command_output_text(&accumulator.stdout, accumulator.stdout_capture_truncated);
    let (stderr, stderr_truncated) =
        command_output_text(&accumulator.stderr, accumulator.stderr_capture_truncated);
    json!({
        "stdout": stdout,
        "stderr": stderr,
        "output": accumulator.ordered_output,
        "truncated": stdout_truncated || stderr_truncated || accumulator.ordered_truncated,
        "outputTruncated": accumulator.ordered_truncated,
    })
}

fn command_output_summary(
    accumulator: &CommandOutputAccumulator,
    status: Option<&std::process::ExitStatus>,
    termination: Option<&str>,
) -> Value {
    let (stdout, stdout_truncated) =
        command_output_text(&accumulator.stdout, accumulator.stdout_capture_truncated);
    let (stderr, stderr_truncated) =
        command_output_text(&accumulator.stderr, accumulator.stderr_capture_truncated);
    json!({
        "exitCode": status.and_then(std::process::ExitStatus::code),
        "success": status.map(std::process::ExitStatus::success).unwrap_or(false),
        "termination": termination,
        "stdoutChars": stdout.chars().count(),
        "stderrChars": stderr.chars().count(),
        "outputChunks": accumulator.next_sequence,
        "truncated": stdout_truncated || stderr_truncated || accumulator.ordered_truncated,
    })
}

pub fn command_result_value(base: Value, output: &CommandExecutionResult) -> Value {
    let (stdout, stdout_truncated) =
        command_output_text(&output.stdout, output.stdout_capture_truncated);
    let (stderr, stderr_truncated) =
        command_output_text(&output.stderr, output.stderr_capture_truncated);
    let truncated = stdout_truncated || stderr_truncated || output.ordered_truncated;
    let details = json!({
        "stdout": stdout,
        "stderr": stderr,
        "output": output.ordered_output.clone(),
        "truncated": truncated,
        "outputTruncated": output.ordered_truncated,
    });
    let mut result = base.as_object().cloned().unwrap_or_default();
    result.insert("exitCode".to_string(), json!(output.status.code()));
    result.insert("success".to_string(), json!(output.status.success()));
    result.insert("stdout".to_string(), json!(stdout));
    result.insert("stderr".to_string(), json!(stderr));
    result.insert("output".to_string(), json!(output.ordered_output.clone()));
    result.insert(
        "summary".to_string(),
        json!({
            "exitCode": output.status.code(),
            "success": output.status.success(),
            "stdoutChars": result
                .get("stdout")
                .and_then(Value::as_str)
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or_default(),
            "stderrChars": result
                .get("stderr")
                .and_then(Value::as_str)
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or_default(),
            "outputChunks": output.ordered_output.len(),
            "truncated": truncated,
        }),
    );
    result.insert("details".to_string(), details);
    result.insert("truncated".to_string(), json!(truncated));
    Value::Object(result)
}

async fn terminate_process(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x08000000)
                .status(),
        )
        .await;
    }
    let _ = child.kill().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
}

async fn wait_for_cancellation(cancellation: &Arc<AtomicBool>) {
    while !cancellation.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[async_trait::async_trait]
pub(crate) trait ProcessCleanup: Send + Sync {
    async fn before_terminate(&self) -> Result<(), String>;
}

async fn terminate_owned_process(
    child: &mut tokio::process::Child,
    cleanup: Option<&dyn ProcessCleanup>,
) -> Result<(), String> {
    let result = match cleanup {
        Some(cleanup) => cleanup.before_terminate().await,
        None => Ok(()),
    };
    terminate_process(child).await;
    result
}

pub async fn execute_process(
    executable: PathBuf,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
    output_sink: Option<&CommandOutputSink>,
) -> Result<CommandExecutionResult, String> {
    execute_process_with_options(
        executable,
        args,
        cwd,
        timeout_ms,
        cancellation,
        output_sink,
        ProcessSandbox::default(),
    )
    .await
}

/// G3-A：是否以受限令牌降权运行。`Default` = 不降权，保持既有行为。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessSandbox {
    pub restricted: bool,
}

/// [`execute_process`] + 显式的沙盒选项。
pub async fn execute_process_with_options(
    executable: PathBuf,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
    output_sink: Option<&CommandOutputSink>,
    sandbox: ProcessSandbox,
) -> Result<CommandExecutionResult, String> {
    execute_process_with_cleanup_and(
        executable,
        args,
        cwd,
        timeout_ms,
        cancellation,
        output_sink,
        None,
        sandbox,
    )
    .await
}

pub(crate) async fn execute_process_with_cleanup(
    executable: PathBuf,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
    output_sink: Option<&CommandOutputSink>,
    cleanup: Option<&dyn ProcessCleanup>,
) -> Result<CommandExecutionResult, String> {
    execute_process_with_cleanup_and(
        executable,
        args,
        cwd,
        timeout_ms,
        cancellation,
        output_sink,
        cleanup,
        ProcessSandbox::default(),
    )
    .await
}

/// 沙盒请求是否应当生效：仅在本地 Windows、非 WSL、且调用方明确要求时。
fn sandbox_applies(sandbox: ProcessSandbox) -> bool {
    #[cfg(windows)]
    {
        sandbox.restricted && crate::sandbox_windows::sandbox_available()
    }
    #[cfg(not(windows))]
    {
        let _ = sandbox;
        false
    }
}

/// G3-A：受限令牌下跑完整个命令，产出与普通路径同型的执行结果。
///
/// 退出码用 `std::process::ExitStatus` 表达不了（Windows 上只能从原始 u32 还原），
/// 因此这里返回一个小的描述，由调用点决定如何落成结果。
#[cfg(windows)]
fn run_restricted(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    output_sink: Option<&CommandOutputSink>,
) -> Option<crate::sandbox_windows::SandboxedOutput> {
    let output = crate::sandbox_windows::run_sandboxed(executable, args, cwd, timeout_ms)
        .ok()
        .flatten()?;
    if let Some(sink) = output_sink {
        sink.emit_chunk(
            0,
            CommandOutputStream::Stdout,
            &String::from_utf8_lossy(&output.stdout),
        );
        sink.emit_chunk(
            1,
            CommandOutputStream::Stderr,
            &String::from_utf8_lossy(&output.stderr),
        );
    }
    Some(output)
}

/// 受限令牌路径的结果：退出状态从原始码还原，stdout/stderr 一次性进有序输出。
#[cfg(windows)]
fn sandbox_execution_result(
    output: crate::sandbox_windows::SandboxedOutput,
) -> CommandExecutionResult {
    use std::os::windows::process::ExitStatusExt;
    let mut ordered_output = Vec::new();
    if !output.stdout.is_empty() {
        ordered_output.push(json!({
            "stream": "stdout",
            "text": String::from_utf8_lossy(&output.stdout),
        }));
    }
    if !output.stderr.is_empty() {
        ordered_output.push(json!({
            "stream": "stderr",
            "text": String::from_utf8_lossy(&output.stderr),
        }));
    }
    CommandExecutionResult {
        status: std::process::ExitStatus::from_raw(output.exit_code as u32),
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_capture_truncated: false,
        stderr_capture_truncated: false,
        ordered_output,
        ordered_truncated: false,
    }
}

pub(crate) async fn execute_process_with_cleanup_and(
    executable: PathBuf,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
    output_sink: Option<&CommandOutputSink>,
    cleanup: Option<&dyn ProcessCleanup>,
    sandbox: ProcessSandbox,
) -> Result<CommandExecutionResult, String> {
    if cancellation.load(Ordering::Relaxed) {
        if let Some(sink) = output_sink {
            sink.emit_finished(
                Some("cancelled"),
                json!({"success": false, "termination": "cancelled"}),
                json!({"error": "请求已取消，命令未启动"}),
            );
        }
        return Err("请求已取消".to_string());
    }
    // G3-A：受限令牌路径。用阻塞线程跑（CreateProcessAsUser + 读管道本身是同步的），
    // 失败/不可用一律回退到下面的普通 spawn，保证任何环境都能启动命令。
    #[cfg(windows)]
    if sandbox_applies(sandbox) && cleanup.is_none() {
        let executable_for_sandbox = executable.clone();
        let args_for_sandbox = args.to_vec();
        let cwd_for_sandbox = cwd.to_path_buf();
        let sandboxed = tokio::task::spawn_blocking(move || {
            run_restricted(
                &executable_for_sandbox,
                &args_for_sandbox,
                &cwd_for_sandbox,
                timeout_ms,
                None,
            )
        })
        .await
        .ok()
        .flatten();
        if let Some(output) = sandboxed {
            if let Some(sink) = output_sink {
                sink.emit_finished(
                    None,
                    json!({"success": output.exit_code == 0, "exitCode": output.exit_code}),
                    json!({"sandbox": "restricted_token"}),
                );
            }
            return Ok(sandbox_execution_result(output));
        }
    }
    let mut process = tokio::process::Command::new(executable);
    #[cfg(windows)]
    process.creation_flags(0x08000000);
    crate::network::apply_child_environment(&mut process);
    process
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(sink) = output_sink {
                sink.emit_finished(
                    Some("spawn_error"),
                    json!({ "success": false, "termination": "spawn_error" }),
                    json!({ "error": error.to_string() }),
                );
            }
            return Err(format!("命令启动失败: {error}"));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let cleanup_result = terminate_owned_process(&mut child, cleanup).await;
            if let Some(sink) = output_sink {
                sink.emit_finished(
                    Some("pipe_error"),
                    json!({ "success": false, "termination": "pipe_error" }),
                    json!({ "error": "stdout 未创建管道" }),
                );
            }
            cleanup_result?;
            return Err("命令输出不可用: stdout 未创建管道".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let cleanup_result = terminate_owned_process(&mut child, cleanup).await;
            if let Some(sink) = output_sink {
                sink.emit_finished(
                    Some("pipe_error"),
                    json!({ "success": false, "termination": "pipe_error" }),
                    json!({ "error": "stderr 未创建管道" }),
                );
            }
            cleanup_result?;
            return Err("命令输出不可用: stderr 未创建管道".to_string());
        }
    };
    let (sender, mut receiver) = mpsc::channel(64);
    let stdout_task = tokio::spawn(read_process_stream(
        CommandOutputStream::Stdout,
        stdout,
        sender.clone(),
    ));
    let stderr_task = tokio::spawn(read_process_stream(
        CommandOutputStream::Stderr,
        stderr,
        sender.clone(),
    ));
    drop(sender);
    let mut accumulator = CommandOutputAccumulator::default();
    let mut stdout_finished = false;
    let mut stderr_finished = false;
    let mut status = None;
    let mut termination = None;
    let mut process_error = None;
    let timeout = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let cancellation_wait = wait_for_cancellation(&cancellation);
    tokio::pin!(cancellation_wait);
    loop {
        if status.is_some() && stdout_finished && stderr_finished {
            break;
        }
        tokio::select! {
            biased;
            _ = &mut cancellation_wait => { termination = Some("cancelled"); break; }
            _ = &mut timeout => { termination = Some("timeout"); break; }
            result = child.wait(), if status.is_none() => {
                match result {
                    Ok(exit_status) => status = Some(exit_status),
                    Err(error) => {
                        process_error = Some(format!("命令执行失败: {error}"));
                        termination = Some("process_error");
                        break;
                    }
                }
            }
            message = receiver.recv(), if !stdout_finished || !stderr_finished => match message {
                Some(ProcessOutputMessage::Chunk { stream, bytes }) => {
                    match stream {
                        CommandOutputStream::Stdout => append_captured_output(&mut accumulator.stdout, &bytes, &mut accumulator.stdout_capture_truncated),
                        CommandOutputStream::Stderr => append_captured_output(&mut accumulator.stderr, &bytes, &mut accumulator.stderr_capture_truncated),
                    }
                    append_ordered_output(&mut accumulator, stream, &bytes, output_sink);
                }
                Some(ProcessOutputMessage::Finished(stream)) => match stream {
                    CommandOutputStream::Stdout => stdout_finished = true,
                    CommandOutputStream::Stderr => stderr_finished = true,
                },
                Some(ProcessOutputMessage::Error { stream, message }) => accumulator.reader_error = Some(format!("{} 输出读取失败: {message}", stream.as_str())),
                // The pipe readers can finish before `Child::wait()` publishes
                // the exit status. Closing the channel is therefore a normal
                // end-of-stream race, not a command failure.
                None => {
                    stdout_finished = true;
                    stderr_finished = true;
                }
            }
        }
    }
    if let Some(termination) = termination {
        let cleanup_result = terminate_owned_process(&mut child, cleanup).await;
        stdout_task.abort();
        stderr_task.abort();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        if let Some(sink) = output_sink {
            let reported_termination = if cleanup_result.is_err() {
                "cleanup_error"
            } else {
                termination
            };
            sink.emit_finished(
                Some(reported_termination),
                command_output_summary(&accumulator, status.as_ref(), Some(reported_termination)),
                command_output_details(&accumulator),
            );
        }
        cleanup_result?;
        return Err(match termination {
            "cancelled" => "请求已取消".to_string(),
            "timeout" => format!("命令执行超时（{} ms）", timeout_ms),
            "process_error" => process_error.unwrap_or_else(|| "命令执行失败".to_string()),
            _ => "命令输出通道已关闭".to_string(),
        });
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if let Some(error) = accumulator.reader_error.take() {
        if let Some(sink) = output_sink {
            sink.emit_finished(
                Some("reader_error"),
                command_output_summary(&accumulator, status.as_ref(), Some("reader_error")),
                command_output_details(&accumulator),
            );
        }
        return Err(error);
    }
    let status = match status {
        Some(status) => status,
        None => {
            if let Some(sink) = output_sink {
                sink.emit_finished(
                    Some("process_error"),
                    command_output_summary(&accumulator, None, Some("process_error")),
                    command_output_details(&accumulator),
                );
            }
            return Err("命令未返回退出状态".to_string());
        }
    };
    if let Some(sink) = output_sink {
        sink.emit_finished(
            None,
            command_output_summary(&accumulator, Some(&status), None),
            command_output_details(&accumulator),
        );
    }
    Ok(CommandExecutionResult {
        status,
        stdout: accumulator.stdout,
        stderr: accumulator.stderr,
        stdout_capture_truncated: accumulator.stdout_capture_truncated,
        stderr_capture_truncated: accumulator.stderr_capture_truncated,
        ordered_output: accumulator.ordered_output,
        ordered_truncated: accumulator.ordered_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::truncate_command_output;

    #[test]
    fn sandbox_defaults_to_no_privilege_drop_and_only_applies_when_asked() {
        use super::*;
        assert_eq!(
            ProcessSandbox::default(),
            ProcessSandbox { restricted: false }
        );
        assert!(!sandbox_applies(ProcessSandbox::default()));
        let requested = ProcessSandbox { restricted: true };
        #[cfg(windows)]
        assert_eq!(
            sandbox_applies(requested),
            crate::sandbox_windows::sandbox_available()
        );
        #[cfg(not(windows))]
        assert!(!sandbox_applies(requested));
    }

    #[test]
    fn continuous_build_output_survives_budget_and_split_utf8() {
        use super::*;
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            "build",
            "",
            "aosp",
        )
        .continuous();
        for sequence in 0..600 {
            sink.emit_remote_bytes(sequence, "stdout", &vec![b'x'; 1024]);
        }
        let chinese = "构建".as_bytes();
        sink.emit_remote_bytes(601, "stdout", &chinese[..2]);
        sink.emit_remote_bytes(602, "stderr", "错误".as_bytes());
        sink.emit_remote_bytes(603, "stdout", &chinese[2..]);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 602);
        assert_eq!(events[600]["text"], "错误");
        assert_eq!(events[601]["text"], "构建");
        assert!(!events
            .iter()
            .any(|event| event["text"].as_str().unwrap().contains("截断")));
    }

    #[tokio::test]
    async fn already_cancelled_command_does_not_attempt_to_spawn() {
        use super::*;
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            "test",
            "test",
            "test",
        );
        let result = execute_process(
            PathBuf::from("android-tool-no-such-executable"),
            &[],
            Path::new("."),
            1,
            Arc::new(AtomicBool::new(true)),
            Some(&sink),
        )
        .await;
        assert!(matches!(result, Err(message) if message == "请求已取消"));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["termination"], "cancelled");
        assert_eq!(events[0]["summary"]["success"], false);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cleanup_failure_is_reported_after_cleanup_and_never_as_stopped() {
        use super::*;
        struct FailedCleanup(Arc<AtomicBool>);
        #[async_trait::async_trait]
        impl ProcessCleanup for FailedCleanup {
            async fn before_terminate(&self) -> Result<(), String> {
                self.0.store(true, Ordering::Relaxed);
                Err(format!(
                    "{PROCESS_CLEANUP_ERROR_PREFIX}test cleanup not confirmed"
                ))
            }
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        let cleanup_seen = Arc::new(AtomicBool::new(false));
        let terminal_seen = Arc::new(AtomicBool::new(false));
        let cancel = cancellation.clone();
        let cleaned = cleanup_seen.clone();
        let finished = terminal_seen.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| {
                if event["phase"] == "chunk" {
                    cancel.store(true, Ordering::Relaxed);
                }
                if event["phase"] == "finished" {
                    assert!(cleaned.load(Ordering::Relaxed));
                    assert_eq!(event["termination"], "cleanup_error");
                    assert_eq!(event["summary"]["success"], false);
                    finished.store(true, Ordering::Relaxed);
                }
            }),
            "test",
            "test",
            "test",
        );
        let result = execute_process_with_cleanup(
            PathBuf::from("cmd.exe"),
            &["/c".into(), "echo ready & ping -n 20 127.0.0.1 >nul".into()],
            Path::new("."),
            10_000,
            cancellation,
            Some(&sink),
            Some(&FailedCleanup(cleanup_seen)),
        )
        .await;
        assert!(
            matches!(result, Err(message) if message.starts_with(PROCESS_CLEANUP_ERROR_PREFIX))
        );
        assert!(terminal_seen.load(Ordering::Relaxed));
    }

    #[test]
    fn byte_bounded_tool_output_never_splits_utf8() {
        for prefix in ["", "a", "ab"] {
            let source = format!("{prefix}{}", "界😀".repeat(20_000));
            for limit in [0, 1, 2, 3, 64 * 1024, 96 * 1024] {
                let mut output = source.clone();
                assert!(super::truncate_output_bytes(&mut output, limit));
                assert!(output.len() <= limit);
                assert!(source.starts_with(&output));
                assert!(!super::truncate_output_bytes(&mut output, limit));
            }
        }
    }

    #[test]
    fn keeps_small_command_output_unchanged() {
        assert_eq!(
            truncate_command_output("stdout"),
            ("stdout".to_string(), false)
        );
    }

    #[test]
    fn bounds_large_command_output_with_a_visible_marker() {
        let (output, truncated) =
            truncate_command_output(&"x".repeat(super::MAX_COMMAND_OUTPUT_CHARS + 1));
        assert!(truncated);
        assert!(output.ends_with("\n[输出已截断]"));
        assert_eq!(
            output.chars().count(),
            super::MAX_COMMAND_OUTPUT_CHARS + "\n[输出已截断]".chars().count()
        );
    }
}
