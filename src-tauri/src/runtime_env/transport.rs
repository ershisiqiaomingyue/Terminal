use super::*;

pub(super) fn read_process_stream_tail(mut stream: impl Read, max_bytes: usize) -> Vec<u8> {
    let mut tail = VecDeque::<u8>::with_capacity(max_bytes);
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let length = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) => length,
        };
        if length >= max_bytes {
            tail.clear();
            tail.extend(&buffer[length - max_bytes..length]);
            truncated = true;
            continue;
        }
        let overflow = tail.len().saturating_add(length).saturating_sub(max_bytes);
        for _ in 0..overflow {
            let _ = tail.pop_front();
        }
        tail.extend(&buffer[..length]);
        truncated |= overflow > 0;
    }
    let mut output = if truncated {
        b"[process output truncated; retained trailing diagnostics]\n".to_vec()
    } else {
        Vec::new()
    };
    output.extend(tail);
    output
}

pub(super) struct CapturedCommandOutput {
    pub(super) child: Child,
    pub(super) stdout_reader: Option<std::thread::JoinHandle<Vec<u8>>>,
    pub(super) stderr_reader: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl CapturedCommandOutput {
    pub(super) fn spawn(command: Command) -> Result<Self, String> {
        Self::spawn_with_output_limit(command, MAX_PROCESS_OUTPUT_BYTES)
    }

    pub(super) fn spawn_with_output_limit(
        mut command: Command,
        max_output_bytes: usize,
    ) -> Result<Self, String> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动命令：{error}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Ok(Self {
            child,
            stdout_reader: Some(std::thread::spawn(move || {
                stdout
                    .map(|stream| read_process_stream_tail(stream, max_output_bytes))
                    .unwrap_or_default()
            })),
            stderr_reader: Some(std::thread::spawn(move || {
                stderr
                    .map(|stream| read_process_stream_tail(stream, max_output_bytes))
                    .unwrap_or_default()
            })),
        })
    }

    pub(super) fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        self.child
            .try_wait()
            .map_err(|error| format!("无法读取命令状态：{error}"))
    }

    pub(super) fn finish(
        &mut self,
        status: std::process::ExitStatus,
    ) -> Result<std::process::Output, String> {
        let stdout = self
            .stdout_reader
            .take()
            .ok_or_else(|| "命令标准输出已读取。".to_string())?
            .join()
            .map_err(|_| "无法读取命令标准输出。".to_string())?;
        let stderr = self
            .stderr_reader
            .take()
            .ok_or_else(|| "命令错误输出已读取。".to_string())?
            .join()
            .map_err(|_| "无法读取命令错误输出。".to_string())?;
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    }

    pub(super) fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

pub(super) fn command_output_with_timeout(
    command: Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    command_output_with_timeout_limit(command, timeout, MAX_PROCESS_OUTPUT_BYTES)
}

pub(super) fn command_output_with_timeout_limit(
    command: Command,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<std::process::Output, String> {
    // Draining both pipes while the process runs prevents a verbose tool
    // (npm, pip, etc.) from blocking on a full pipe.
    let mut child = CapturedCommandOutput::spawn_with_output_limit(command, max_output_bytes)?;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return child.finish(status);
        }
        if started.elapsed() >= timeout {
            child.terminate();
            return Err(format!("命令执行超时（{} 秒）。", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
