//! Disk-backed binary transfer. Never capture archives in command output.
use super::*;

pub(super) fn transfer_binary(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
    local: &Path,
    upload: bool,
    cancellation: Arc<AtomicBool>,
    progress: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<(), String> {
    transfer_binary_impl(
        config,
        password,
        command,
        local,
        upload,
        false,
        cancellation,
        progress,
    )
}

/// 续传用：远端命令返回的是「从断点开始」的剩余字节，本地文件已经存在，
/// 必须**追加**而不是覆盖，否则前面已经传完的部分会被清掉。
pub(super) fn append_binary(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
    local: &Path,
    cancellation: Arc<AtomicBool>,
    progress: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<(), String> {
    transfer_binary_impl(
        config,
        password,
        command,
        local,
        false,
        true,
        cancellation,
        progress,
    )
}

fn local_sink(local: &Path, append: bool) -> Result<fs::File, String> {
    let opened = if append {
        fs::OpenOptions::new().append(true).create(true).open(local)
    } else {
        fs::File::create(local)
    };
    opened.map_err(|error| error.to_string())
}

fn transfer_binary_impl(
    config: &SshConnectionConfig,
    password: Option<&str>,
    command: &str,
    local: &Path,
    upload: bool,
    append: bool,
    cancellation: Arc<AtomicBool>,
    progress: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<(), String> {
    let timeout = Duration::from_secs(24 * 60 * 60);
    if uses_openssh_fallback(config) {
        let (mut child, askpass) = spawn_openssh_command(
            config,
            command,
            if upload { Some(&[]) } else { None },
            timeout,
            password,
            false,
        )?;
        let stderr = child.stderr.take().ok_or("SSH 错误管道不可用")?;
        let errors = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.take(8192).read_to_end(&mut bytes);
            bytes
        });
        let source = child.stdout.take().ok_or("SSH 输出管道不可用")?;
        let destination = child.stdin.take();
        let local = local.to_path_buf();
        let cancel = cancellation.clone();
        let worker = std::thread::spawn(move || -> Result<(), String> {
            if upload {
                let mut source = fs::File::open(local).map_err(|error| error.to_string())?;
                let mut destination = destination.ok_or("SSH 输入管道不可用")?;
                copy_stream(&mut source, &mut destination, &cancel, &progress)?;
                drop(destination);
            } else {
                let mut source = source;
                let mut destination = local_sink(&local, append)?;
                copy_stream(&mut source, &mut destination, &cancel, &progress)?;
            }
            Ok(())
        });
        let started = Instant::now();
        let status = loop {
            if cancellation.load(Ordering::Relaxed) || started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                break Err("传输已取消或超时".to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error.to_string());
                }
            }
        };
        let copied = worker
            .join()
            .map_err(|_| "传输线程异常".to_string())
            .and_then(|result| result);
        let errors = errors.join().unwrap_or_default();
        remove_temporary_askpass(&askpass);
        let status = status?;
        if !status.success() {
            return Err(format!(
                "SSH 传输失败：{}",
                String::from_utf8_lossy(&errors).trim()
            ));
        }
        return copied;
    }
    let session = connect(config, password)?;
    session.set_blocking(false);
    let mut channel = retry_nonblocking_with_cancel(
        "打开传输通道",
        Duration::from_secs(30),
        &cancellation,
        || session.channel_session(),
    )?;
    let result = (|| {
        retry_nonblocking_with_cancel(
            "启动传输",
            Duration::from_secs(30),
            &cancellation,
            || channel.exec(command),
        )?;
        if upload {
            let mut source = fs::File::open(local).map_err(|error| error.to_string())?;
            copy_stream(&mut source, &mut channel, &cancellation, &progress)?;
            retry_nonblocking_with_cancel(
                "结束上传",
                Duration::from_secs(30),
                &cancellation,
                || channel.send_eof(),
            )?;
        } else {
            let mut destination = local_sink(local, append)?;
            copy_stream(&mut channel, &mut destination, &cancellation, &progress)?;
        }
        let mut stderr = Vec::new();
        let mut stream = channel.stderr();
        let mut buffer = [0; 1024];
        loop {
            let length = retry_io_with_cancel(
                "读取传输结果",
                Duration::from_secs(30),
                &cancellation,
                || stream.read(&mut buffer),
            )?;
            if length == 0 {
                break;
            }
            if stderr.len() < 8192 {
                stderr.extend_from_slice(&buffer[..length.min(8192 - stderr.len())]);
            }
        }
        retry_nonblocking_with_cancel(
            "关闭传输",
            Duration::from_secs(30),
            &cancellation,
            || channel.wait_close(),
        )?;
        if channel.exit_status().map_err(ssh_error)? != 0 {
            return Err(format!(
                "SSH 传输失败：{}",
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        Ok(())
    })();
    let _ = channel.close();
    result
}

fn copy_stream(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancel: &Arc<AtomicBool>,
    progress: &Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<(), String> {
    let mut buffer = [0; 64 * 1024];
    let mut completed = 0;
    let mut reported = Instant::now();
    loop {
        let length = retry_io_with_cancel(
            "读取传输数据",
            Duration::from_secs(120),
            cancel,
            || reader.read(&mut buffer),
        )?;
        if length == 0 {
            break;
        }
        let mut offset = 0;
        while offset < length {
            let count = retry_io_with_cancel(
                "写入传输数据",
                Duration::from_secs(120),
                cancel,
                || writer.write(&buffer[offset..length]),
            )?;
            if count == 0 {
                return Err("传输通道提前关闭".into());
            }
            offset += count;
        }
        completed += length as u64;
        if reported.elapsed() >= Duration::from_millis(200) {
            progress(completed);
            reported = Instant::now();
        }
    }
    progress(completed);
    Ok(())
}
