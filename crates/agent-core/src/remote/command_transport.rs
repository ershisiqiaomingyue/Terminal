use super::*;

pub(super) fn command_line(input: &Value) -> Result<String, String> {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "run_local_command 需要 command".to_string())?;
    if command.chars().count() > 64 * 1024 {
        return Err("run_local_command 的 command 不能超过 64 KB".to_string());
    }
    let args = match input.get("args") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                value
                    .as_str()
                    .filter(|value| value.chars().count() <= 16 * 1024)
                    .map(shell_escape)
                    .ok_or_else(|| {
                        format!("run_local_command 的 args[{index}] 必须是 16 KB 以内的字符串")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("run_local_command 的 args 必须是字符串数组".to_string()),
    };
    if args.is_empty() {
        Ok(command.to_string())
    } else {
        Ok(format!("{} {}", command, args.join(" ")))
    }
}

pub(super) fn execute_remote_command(
    channel: Channel,
    session: &Session,
    command: &str,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
    _process_id: Option<Arc<Mutex<Option<u32>>>>,
) -> Result<Value, String> {
    execute_remote_command_with_input(channel, session, command, None, context, sink, timeout)
}

pub(super) fn execute_remote_command_with_input(
    mut channel: Channel,
    session: &Session,
    command: &str,
    input: Option<&[u8]>,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
) -> Result<Value, String> {
    cancelled(&context.cancellation)?;
    let root = execute_remote_control(session, crate::owned_process::PREPARE)?;
    if !crate::owned_process::valid_control_root(&root) {
        return Err("SSH 返回了无效的进程控制目录，命令未执行".into());
    }
    let execution =
        crate::owned_process::shell_command(&root, "sh", &["-c".into(), command.into()]);
    let result = if context.cancellation.load(Ordering::Relaxed) {
        Err("请求已取消".into())
    } else {
        execute_remote_command_inner_with_input(
            &mut channel,
            session,
            &execution,
            input,
            context,
            sink,
            timeout,
        )
    };
    let cleanup = finalize_remote_process(session, context, &root, false);
    let _ = channel.close();
    cleanup.map_err(|error| {
        format!(
            "{}SSH 子进程停止未确认: {error}",
            crate::process::PROCESS_CLEANUP_ERROR_PREFIX
        )
    })?;
    result.map(|mut value| {
        value["command"] = json!(command);
        value
    })
}

pub(super) fn execute_remote_control(session: &Session, command: &str) -> Result<String, String> {
    let timeout = Duration::from_secs(10);
    let started = Instant::now();
    let channel = retry_nonblocking("打开 SSH 进程控制通道", timeout, || {
        session.channel_session()
    })?;
    execute_remote_control_on_channel(channel, command, started, timeout)
}

#[derive(Clone, Copy)]
pub(super) enum SshCleanup {
    Drain,
    DrainPty,
    Remove,
}

// Keep ownership metadata until a drain has been acknowledged. Retrying a
// lost reply can then inspect the same process identity instead of replaying
// the original command or mistaking an already removed directory for failure.
pub(super) fn remote_cleanup_command(root: &str, operation: SshCleanup) -> Result<String, String> {
    if !crate::owned_process::valid_control_root(root) {
        return Err("SSH 进程控制目录无效，拒绝清理".into());
    }
    let action = match operation {
        SshCleanup::Drain => crate::owned_process::DRAIN,
        SshCleanup::DrainPty => crate::owned_process::DRAIN_PTY,
        SshCleanup::Remove => crate::owned_process::REMOVE,
    };
    let removing = matches!(operation, SshCleanup::Remove);
    let script = format!(
        r#"root=$1
if test ! -e "$root" && test ! -L "$root"; then exit {missing}; fi
test -d "$root" && test ! -L "$root" || exit 125
exec 5>"$root/cleanup.lock" || exit 125
flock -x 5 || exit 125
if test ! -e "$root" && test ! -L "$root"; then exit {missing}; fi
{action}
"#,
        missing = if removing { 0 } else { 125 },
        action = if removing {
            format!("test -f \"$root/drained\" || exit 125\n{action}")
        } else {
            format!(
                "test ! -f \"$root/drained\" || exit 0\nsh -c {} android-tool-drain \"$root\" || exit $?\n: > \"$root/drained\"",
                shell_escape(action)
            )
        },
    );
    Ok(format!(
        "sh -c {} android-tool-cleanup {}",
        shell_escape(&script),
        shell_escape(root)
    ))
}

pub(super) fn finalize_remote_process(
    session: &Session,
    context: &SshToolContext,
    root: &str,
    pty: bool,
) -> Result<String, String> {
    execute_remote_cleanup_control(
        session,
        context,
        root,
        if pty {
            SshCleanup::DrainPty
        } else {
            SshCleanup::Drain
        },
    )?;
    execute_remote_cleanup_control(session, context, root, SshCleanup::Remove)
}

pub(super) fn execute_remote_cleanup_control(
    session: &Session,
    context: &SshToolContext,
    root: &str,
    operation: SshCleanup,
) -> Result<String, String> {
    let command = remote_cleanup_command(root, operation)?;
    let started = Instant::now();
    let result = retry_nonblocking("打开 SSH 清理通道", Duration::from_secs(3), || {
        session.channel_session()
    })
    .and_then(|channel| {
        execute_remote_control_on_channel(channel, &command, started, Duration::from_secs(10))
    });
    recover_remote_cleanup(context, &command, result)
}

pub(super) fn recover_remote_cleanup(
    context: &SshToolContext,
    command: &str,
    result: Result<String, String>,
) -> Result<String, String> {
    match result {
        Ok(output) => Ok(output),
        Err(first_error) => {
            // These typed cleanup operations are idempotent, including when
            // exec succeeded but its acknowledgement/output was lost.
            invalidate_cached_connection(&context.config);
            let mut config = context.config.clone();
            config.reuse_connection = false;
            config.connect_timeout_secs = config.connect_timeout_secs.clamp(3, 5);
            let fresh = connect_fresh(&config, context.password.as_deref())
                .map_err(|error| format!("{first_error}; SSH 清理重连失败: {error}"))?;
            fresh.set_blocking(false);
            execute_remote_control(&fresh, command)
                .map_err(|error| format!("{first_error}; SSH 重连后清理仍未确认: {error}"))
        }
    }
}

pub(super) fn execute_remote_control_on_channel(
    mut channel: Channel,
    command: &str,
    started: Instant,
    timeout: Duration,
) -> Result<String, String> {
    retry_nonblocking(
        "启动 SSH 进程控制",
        timeout.saturating_sub(started.elapsed()),
        || channel.exec(command),
    )?;
    retry_nonblocking(
        "结束 SSH 控制输入",
        timeout.saturating_sub(started.elapsed()),
        || channel.send_eof(),
    )?;
    let mut output = Vec::new();
    let mut buffer = [0; 1024];
    let mut finished = [false, false];
    while !finished.iter().all(|finished| *finished) {
        if started.elapsed() >= timeout {
            return Err("SSH 进程控制超时".into());
        }
        for (index, mut stream) in [channel.stream(0), channel.stderr()]
            .into_iter()
            .enumerate()
        {
            match stream.read(&mut buffer) {
                Ok(0) => finished[index] = true,
                Ok(length) if index == 0 => {
                    if output.len() + length > 4096 {
                        return Err("SSH 进程控制返回了过长的数据".into());
                    }
                    output.extend_from_slice(&buffer[..length]);
                }
                Ok(_) => {}
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::TimedOut => {}
                Err(error) => return Err(format!("读取 SSH 进程控制结果失败: {error}")),
            }
        }
        if !finished.iter().all(|finished| *finished) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    retry_nonblocking(
        "等待 SSH 进程控制结束",
        timeout.saturating_sub(started.elapsed()),
        || channel.wait_close(),
    )?;
    let exit = channel.exit_status().map_err(ssh_error)?;
    if exit != 0 {
        return Err(format!("SSH 进程控制失败（{exit}）"));
    }
    Ok(String::from_utf8_lossy(&output).trim().into())
}

#[cfg(all(test, windows))]
pub(super) fn execute_remote_command_inner(
    channel: &mut Channel,
    session: &Session,
    command: &str,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
) -> Result<Value, String> {
    execute_remote_command_inner_with_input(channel, session, command, None, context, sink, timeout)
}

fn execute_remote_command_inner_with_input(
    channel: &mut Channel,
    session: &Session,
    command: &str,
    input: Option<&[u8]>,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
) -> Result<Value, String> {
    retry_nonblocking("启动远程命令", timeout, || channel.exec(command))?;
    if let Some(input) = input {
        let mut offset = 0;
        while offset < input.len() {
            let count = retry_io_with_cancel(
                "写入远程命令输入",
                timeout,
                &context.cancellation,
                || channel.write(&input[offset..]),
            )?;
            if count == 0 {
                return Err("远程输入通道已关闭".into());
            }
            offset += count;
        }
    }
    retry_nonblocking("结束远程命令输入", timeout, || channel.send_eof())?;
    let started = Instant::now();
    let mut last_wait_progress = started;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let mut sequence = 0u64;
    let mut buffer = [0u8; 8192];
    let mut read_stream = |stream: &mut dyn Read,
                           target: &mut Vec<u8>,
                           truncated: &mut bool,
                           name: &str|
     -> Result<bool, String> {
        for _ in 0..64 {
            cancelled(&context.cancellation)?;
            if started.elapsed() >= timeout {
                return Err(format!("远程命令执行超时（{} 秒）", timeout.as_secs()));
            }
            match stream.read(&mut buffer) {
                Ok(0) => return Ok(true),
                Ok(length) => {
                    let visible_bytes = &buffer[..length];
                    if let Some(sink) = sink.filter(|sink| sink.is_continuous()) {
                        sink.emit_remote_bytes(sequence, name, visible_bytes);
                        sequence = sequence.saturating_add(1);
                    }
                    let remaining = MAX_REMOTE_COMMAND_OUTPUT_BYTES.saturating_sub(target.len());
                    let exceeded = visible_bytes.len() > remaining;
                    let first_truncated_chunk = exceeded && !*truncated;
                    if remaining > 0 {
                        target.extend_from_slice(
                            &visible_bytes[..visible_bytes.len().min(remaining)],
                        );
                    }
                    if exceeded {
                        *truncated = true;
                    }
                    if let Some(sink) = sink.filter(|sink| !sink.is_continuous()) {
                        if remaining > 0 {
                            let visible = String::from_utf8_lossy(
                                &visible_bytes[..visible_bytes.len().min(remaining)],
                            );
                            sink.emit_remote_chunk(sequence, name, &visible);
                            sequence = sequence.saturating_add(1);
                        }
                        if first_truncated_chunk {
                            sink.emit_remote_chunk(
                                sequence,
                                name,
                                &format!("[远程 {name} 已达到输出上限，后续内容已省略]\\n"),
                            );
                            sequence = sequence.saturating_add(1);
                        }
                    }
                }
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::TimedOut =>
                {
                    return Ok(false)
                }
                Err(error) => return Err(format!("远程 {name} 输出读取失败: {error}")),
            }
        }
        Ok(false)
    };
    while !(stdout_eof && stderr_eof) {
        if context.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        if let Err(error) = send_session_keepalive(session) {
            if !error.contains(&SSH2_ERROR_EAGAIN.to_string()) {
                return Err(error);
            }
        }
        if started.elapsed() > timeout {
            return Err(format!("远程命令执行超时（{} 秒）", timeout.as_secs()));
        }
        if last_wait_progress.elapsed() >= Duration::from_secs(5) {
            if let Some(sink) = sink {
                sink.emit_remote_progress(json!({"stage": "waiting", "elapsedSeconds": started.elapsed().as_secs(), "message": format!("远程命令仍在执行，已等待 {} 秒", started.elapsed().as_secs())}));
            }
            last_wait_progress = Instant::now();
        }
        stdout_eof = read_stream(
            &mut channel.stream(0),
            &mut stdout,
            &mut stdout_truncated,
            "stdout",
        )?;
        stderr_eof = read_stream(
            &mut channel.stderr(),
            &mut stderr,
            &mut stderr_truncated,
            "stderr",
        )?;
        if !(stdout_eof && stderr_eof) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if let Err(error) =
        retry_nonblocking("等待远程命令结束", timeout, || channel.wait_close())
    {
        return Err(error);
    }
    let exit_code = match retry_nonblocking("读取远程命令退出码", timeout, || {
        channel.exit_status()
    }) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            return Err(error);
        }
    };
    let stdout_text = String::from_utf8_lossy(&stdout);
    let stderr_text = String::from_utf8_lossy(&stderr);
    Ok(json!({
        "command": command,
        "exitCode": exit_code,
        "success": exit_code == 0,
        "stdout": if stdout_truncated { format!("{stdout_text}\n[远程 stdout 已截断]") } else { stdout_text.into_owned() },
        "stderr": if stderr_truncated { format!("{stderr_text}\n[远程 stderr 已截断]") } else { stderr_text.into_owned() },
        "outputTruncated": stdout_truncated || stderr_truncated,
        "outputChunks": sequence,
    }))
}

pub(super) fn execute_remote_pty_command(
    mut channel: Channel,
    session: &Session,
    command: &str,
    input: Option<&[u8]>,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
) -> Result<Value, String> {
    cancelled(&context.cancellation)?;
    let root = execute_remote_control(session, crate::owned_process::PREPARE)?;
    if !crate::owned_process::valid_control_root(&root) {
        return Err("SSH 返回了无效的终端控制目录，命令未执行".into());
    }
    let result = execute_remote_pty_command_inner(
        &mut channel,
        session,
        command,
        input,
        context,
        sink,
        timeout,
        &root,
    );
    let cleanup = if result.is_ok() {
        execute_remote_cleanup_control(session, context, &root, SshCleanup::Remove)
    } else {
        finalize_remote_process(session, context, &root, true)
    };
    let _ = channel.close();
    cleanup.map_err(|error| {
        format!(
            "{}SSH 终端子进程停止未确认: {error}",
            crate::process::PROCESS_CLEANUP_ERROR_PREFIX
        )
    })?;
    result
}

pub(super) fn execute_remote_pty_command_inner(
    channel: &mut Channel,
    session: &Session,
    command: &str,
    input: Option<&[u8]>,
    context: &SshToolContext,
    sink: Option<&CommandOutputSink>,
    timeout: Duration,
    root: &str,
) -> Result<Value, String> {
    cancelled(&context.cancellation)?;
    retry_nonblocking("申请远程 PTY", timeout, || {
        channel.request_pty("xterm-256color", None, Some((120, 40, 0, 0)))
    })?;
    retry_nonblocking("启动远程交互命令", timeout, || {
        channel.exec(&crate::owned_process::pty_command(root, command))
    })?;
    let started = Instant::now();
    if let Some(input) = input.filter(|value| !value.is_empty()) {
        let mut offset = 0usize;
        while offset < input.len() {
            if started.elapsed() >= timeout {
                return Err(format!("远程交互命令执行超时（{} 秒）", timeout.as_secs()));
            }
            let written = retry_io_with_cancel(
                "写入远程交互输入",
                timeout.saturating_sub(started.elapsed()),
                &context.cancellation,
                || channel.write(&input[offset..]),
            )?;
            if written == 0 {
                return Err("远程交互终端已关闭，无法继续写入输入".to_string());
            }
            offset += written;
        }
    }
    retry_nonblocking("结束远程交互输入", timeout, || channel.send_eof())?;
    let mut output = Vec::new();
    let mut truncated = false;
    let mut exit_code = None;
    let mut drained_at = None;
    let mut last_status = started - Duration::from_millis(100);
    let mut sequence = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        if context.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".to_string());
        }
        if exit_code.is_none() && started.elapsed() >= timeout {
            return Err(format!("远程交互命令执行超时（{} 秒）", timeout.as_secs()));
        }
        if drained_at.is_some_and(|started: Instant| started.elapsed() >= Duration::from_secs(10)) {
            return Err("远程交互命令输出通道未能结束".into());
        }
        if exit_code.is_none() && last_status.elapsed() >= Duration::from_millis(100) {
            let status = execute_remote_control(
                session,
                &format!(
                    "if test -r {root}/exit; then cat {root}/exit; fi",
                    root = shell_escape(root)
                ),
            )?;
            last_status = Instant::now();
            if !status.is_empty() {
                let code = status
                    .parse::<i32>()
                    .ok()
                    .filter(|code| (0..=255).contains(code))
                    .ok_or("SSH 终端返回了无效的退出码")?;
                execute_remote_cleanup_control(session, context, root, SshCleanup::DrainPty)
                    .map_err(|error| {
                        format!(
                            "{}SSH 终端子进程停止未确认: {error}",
                            crate::process::PROCESS_CLEANUP_ERROR_PREFIX
                        )
                    })?;
                exit_code = Some(code);
                drained_at = Some(Instant::now());
            }
        }
        match channel.read(&mut buffer) {
            Ok(0) if channel.eof() => {
                if exit_code.is_none() {
                    return Err("SSH 终端在发布退出状态前关闭".into());
                }
                break;
            }
            Ok(0) => std::thread::sleep(Duration::from_millis(20)),
            Ok(length) => {
                let remaining = MAX_REMOTE_COMMAND_OUTPUT_BYTES.saturating_sub(output.len());
                let visible_length = length.min(remaining);
                output.extend_from_slice(&buffer[..visible_length]);
                if let Some(sink) = sink.filter(|_| visible_length > 0) {
                    sink.emit_remote_chunk(
                        sequence,
                        "stdout",
                        &String::from_utf8_lossy(&buffer[..visible_length]),
                    );
                    sequence = sequence.saturating_add(1);
                }
                if visible_length < length {
                    truncated = true;
                }
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("远程交互输出读取失败: {error}")),
        }
    }
    retry_nonblocking("等待远程交互命令结束", timeout, || {
        channel.wait_close()
    })?;
    let exit_code = exit_code.ok_or("SSH 终端没有返回退出码")?;
    Ok(json!({
        "command": command,
        "exitCode": exit_code,
        "success": exit_code == 0,
        "stdout": String::from_utf8_lossy(&output),
        "stderr": "",
        "pty": true,
        "outputTruncated": truncated,
        "outputChunks": sequence,
    }))
}

pub(super) fn remote_command_scripts(cwd: &str, command: &str) -> (String, String) {
    let display_command = format!("cd {} && {}", shell_escape(cwd), command);
    let execution_command = display_command.clone();
    (display_command, execution_command)
}

pub(super) fn finish_remote_sink(sink: Option<&CommandOutputSink>, result: &Value) {
    if let Some(sink) = sink {
        let success = result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        sink.emit_remote_finished(
            None,
            json!({ "success": success, "exitCode": result.get("exitCode") }),
            json!({
                "stdout": result.get("stdout"),
                "stderr": result.get("stderr"),
            }),
        );
    }
}

pub(super) fn finish_remote_operation<T>(
    sink: Option<&CommandOutputSink>,
    result: Result<T, String>,
) -> Result<T, String> {
    if let Some(sink) = sink {
        match &result {
            Ok(_) => sink.emit_remote_finished(None, json!({ "success": true }), json!({})),
            Err(error) => sink.emit_remote_finished(
                Some("error"),
                json!({ "success": false }),
                json!({ "error": error }),
            ),
        }
    }
    result
}
