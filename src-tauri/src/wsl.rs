use agent_core::{SshRemoteStatus, WslRuntimeConfig};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use crate::background_process::command as background_command;

const WSL_CONFIG_FILE: &str = "wsl-profiles.json";
const MAX_WSL_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// Keep draining a potentially noisy Linux process while retaining its final
/// diagnostic output. AOSP failures are normally emitted at the end, and
/// retaining only that bounded tail keeps a lengthy build from growing the
/// desktop process without bound.
fn read_command_stream_tail(mut stream: impl Read, sink: Option<agent_core::CommandOutputSink>, name: &str) -> Vec<u8> {
    let mut tail = VecDeque::<u8>::with_capacity(MAX_WSL_COMMAND_OUTPUT_BYTES);
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let length = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) => length,
        };
        if let Some(sink) = &sink {
            sink.emit_remote_bytes(0, name, &buffer[..length]);
        }
        let overflow = tail
            .len()
            .saturating_add(length)
            .saturating_sub(MAX_WSL_COMMAND_OUTPUT_BYTES);
        for _ in 0..overflow {
            let _ = tail.pop_front();
        }
        tail.extend(&buffer[..length]);
        truncated |= overflow > 0;
    }
    let mut output = if truncated {
        "[WSL 输出过长，以下保留末尾诊断内容]\n".as_bytes().to_vec()
    } else {
        Vec::new()
    };
    output.extend(tail);
    output
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslProfileConfig {
    pub id: String,
    pub name: String,
    pub distribution: String,
    #[serde(rename = "defaultRoot", default)]
    pub default_root: Option<String>,
    #[serde(default = "default_shell")]
    pub shell: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StoredWslProfiles {
    profiles: Vec<WslProfileConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslDistributionInfo {
    pub name: String,
    pub version: Option<u8>,
    pub state: Option<String>,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
}

fn default_shell() -> String {
    "bash".to_string()
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(WSL_CONFIG_FILE))
        .map_err(|error| format!("无法定位 WSL 配置目录: {error}"))
}

fn load_profiles(app: &AppHandle) -> Result<Vec<WslProfileConfig>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("无法读取 WSL 配置: {error}"))?;
    serde_json::from_str::<StoredWslProfiles>(&content)
        .map(|stored| stored.profiles)
        .map_err(|error| format!("WSL 配置格式无效: {error}"))
}

fn save_profiles(app: &AppHandle, profiles: &[WslProfileConfig]) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "WSL 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 WSL 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(&StoredWslProfiles {
        profiles: profiles.to_vec(),
    })
    .map_err(|error| format!("无法序列化 WSL 配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 WSL 配置: {error}"))
}

fn normalized_profile(mut profile: WslProfileConfig) -> Result<WslProfileConfig, String> {
    profile.id = profile.id.trim().to_string();
    profile.name = profile.name.trim().to_string();
    profile.distribution = profile.distribution.trim().to_string();
    profile.shell = profile.shell.trim().to_ascii_lowercase();
    if profile.id.is_empty() || profile.name.is_empty() || profile.distribution.is_empty() {
        return Err("WSL 名称和发行版不能为空".to_string());
    }
    if !matches!(profile.shell.as_str(), "bash" | "sh" | "zsh") {
        return Err("WSL Shell 只支持 bash、sh 或 zsh".to_string());
    }
    let runtime = WslRuntimeConfig {
        distribution: profile.distribution.clone(),
        root: profile
            .default_root
            .clone()
            .unwrap_or_else(|| "/".to_string()),
    }
    .normalized()?;
    profile.distribution = runtime.distribution;
    profile.default_root = Some(runtime.root);
    Ok(profile)
}

fn wsl_executable() -> &'static str {
    "wsl.exe"
}

/// `wsl.exe --cd` does not expand a shell home shortcut. Resolve the only
/// supported shorthand before passing the path to WSL so AOSP profiles can
/// use `~` without treating it as a literal directory name.
fn resolve_wsl_home_path(profile: &WslProfileConfig, path: &str) -> Result<String, String> {
    let path = path.trim();
    if path != "~" && !path.starts_with("~/") {
        return Ok(path.to_string());
    }
    let output = background_command(wsl_executable())
        .args([
            "--distribution",
            &profile.distribution,
            "--exec",
            &profile.shell,
            "-lc",
            "printf %s \"$HOME\"",
        ])
        .output()
        .map_err(|error| format!("无法读取 WSL 用户目录：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取 WSL 用户目录：{}",
            clean_wsl_output(&output.stderr)
        ));
    }
    let home = clean_wsl_output(&output.stdout);
    if !home.starts_with('/') {
        return Err("WSL 用户目录无效。".to_string());
    }
    Ok(if path == "~" {
        home
    } else {
        format!("{home}/{}", &path[2..])
    })
}

fn run_wsl(
    profile: &WslProfileConfig,
    cwd: Option<&str>,
    command: &str,
) -> Result<std::process::Output, String> {
    let root = cwd
        .filter(|value| !value.trim().is_empty())
        .or(profile.default_root.as_deref())
        .unwrap_or("/");
    let runtime = WslRuntimeConfig {
        distribution: profile.distribution.clone(),
        root: root.to_string(),
    }
    .normalized()?;
    let output = background_command(wsl_executable())
        .args([
            "--distribution",
            &runtime.distribution,
            "--cd",
            &runtime.root,
            "--exec",
            &profile.shell,
            "-lc",
            command,
        ])
        .output()
        .map_err(|error| format!("无法启动 WSL：{error}"))?;
    Ok(output)
}

/// Runs a non-interactive Linux command for a typed desktop workflow. The
/// caller owns cancellation and must keep the command template constrained.
pub fn run_wsl_workspace_command(
    profile: &WslProfileConfig,
    cwd: &str,
    command: &str,
    cancellation: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<Output, String> {
    run_wsl_workspace_command_streaming(profile, cwd, command, cancellation, timeout, None)
}

pub fn run_wsl_workspace_command_streaming(
    profile: &WslProfileConfig,
    cwd: &str,
    command: &str,
    cancellation: &Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<agent_core::CommandOutputSink>,
) -> Result<Output, String> {
    let root = if cwd.trim().is_empty() {
        profile.default_root.as_deref().unwrap_or("/")
    } else {
        cwd.trim()
    };
    let root = resolve_wsl_home_path(profile, root)?;
    let runtime = WslRuntimeConfig {
        distribution: profile.distribution.clone(),
        root,
    }
    .normalized()?;
    let mut child = background_command(wsl_executable())
        .args([
            "--distribution",
            &runtime.distribution,
            "--cd",
            &runtime.root,
            "--exec",
            &profile.shell,
            "-lc",
            command,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 WSL：{error}"))?;
    // AOSP can produce enough output to fill either pipe long before the
    // command completes. Drain both streams while polling so that output
    // pressure cannot make a healthy build appear to hang.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_sink = sink.clone();
    let stdout_reader =
        std::thread::spawn(move || stdout.map(|stream| read_command_stream_tail(stream, stdout_sink, "stdout")).unwrap_or_default());
    let stderr_reader =
        std::thread::spawn(move || stderr.map(|stream| read_command_stream_tail(stream, sink, "stderr")).unwrap_or_default());
    let started = Instant::now();
    loop {
        if cancellation.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("请求已取消".to_string());
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!("WSL 命令执行超时（{} 秒）", timeout.as_secs()));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("无法等待 WSL 命令：{error}"))?
        {
            let stdout = stdout_reader
                .join()
                .map_err(|_| "无法读取 WSL 标准输出。".to_string())?;
            let stderr = stderr_reader
                .join()
                .map_err(|_| "无法读取 WSL 错误输出。".to_string())?;
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Converts a local Windows staging path into the selected distribution's
/// Linux path before a typed workflow copies a build artifact into it.
pub fn local_path_in_wsl(
    profile: &WslProfileConfig,
    local_path: &std::path::Path,
) -> Result<String, String> {
    let output = background_command(wsl_executable())
        .args([
            "--distribution",
            &profile.distribution,
            "--exec",
            "wslpath",
            "-u",
        ])
        .arg(local_path)
        .output()
        .map_err(|error| format!("无法转换 WSL 暂存路径：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法转换 WSL 暂存路径：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let path = clean_wsl_output(&output.stdout);
    if path.is_empty() || !path.starts_with('/') {
        return Err("WSL 暂存路径无效。".to_string());
    }
    Ok(path)
}

fn clean_wsl_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .replace('\0', "")
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}

fn parse_distribution_lines(output: &[u8]) -> Vec<WslDistributionInfo> {
    let text = clean_wsl_output(output);
    text.lines()
        .filter_map(|line| {
            let is_default = line.trim_start().starts_with('*');
            let line = line.trim().trim_start_matches('*').trim();
            if line.is_empty() || line.eq_ignore_ascii_case("name") {
                return None;
            }
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts
                .first()
                .is_some_and(|part| part.eq_ignore_ascii_case("name"))
            {
                return None;
            }
            let name = parts.first()?.to_string();
            Some(WslDistributionInfo {
                name,
                version: parts.iter().rev().find_map(|part| part.parse::<u8>().ok()),
                state: parts.get(1).map(|part| part.to_string()),
                is_default,
            })
        })
        .collect()
}

#[tauri::command(async)]
pub fn list_wsl_distributions() -> Result<Vec<WslDistributionInfo>, String> {
    let output = background_command(wsl_executable())
        .args(["--list", "--verbose"])
        .output()
        .map_err(|error| format!("无法查询 WSL 发行版：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "WSL 查询失败：{}",
            clean_wsl_output(&output.stderr)
        ));
    }
    Ok(parse_distribution_lines(&output.stdout))
}

#[tauri::command]
pub fn list_wsl_profiles(app: AppHandle) -> Result<Vec<WslProfileConfig>, String> {
    load_profiles(&app)
}

#[tauri::command]
pub fn save_wsl_profile(
    app: AppHandle,
    profile: WslProfileConfig,
) -> Result<WslProfileConfig, String> {
    let profile = normalized_profile(profile)?;
    let mut profiles = load_profiles(&app)?;
    profiles.retain(|item| item.id != profile.id);
    profiles.push(profile.clone());
    save_profiles(&app, &profiles)?;
    Ok(profile)
}

#[tauri::command]
pub fn delete_wsl_profile(app: AppHandle, id: String) -> Result<(), String> {
    let id = id.trim();
    let mut profiles = load_profiles(&app)?;
    let original = profiles.len();
    profiles.retain(|profile| profile.id != id);
    if profiles.len() == original {
        return Err("WSL 配置不存在".to_string());
    }
    save_profiles(&app, &profiles)
}

pub fn load_profile_for_agent(app: &AppHandle, id: &str) -> Result<WslProfileConfig, String> {
    load_profiles(app)?
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "WSL 配置不存在，请先在设置中保存配置".to_string())
}

#[tauri::command(async)]
pub fn test_wsl_profile(app: AppHandle, id: String) -> Result<(), String> {
    let profile = load_profile_for_agent(&app, id.trim())?;
    let output = run_wsl(&profile, None, "printf wsl-ready")?;
    if output.status.success() && clean_wsl_output(&output.stdout) == "wsl-ready" {
        Ok(())
    } else {
        Err(format!(
            "WSL 测试失败：{}",
            clean_wsl_output(&output.stderr)
        ))
    }
}

#[tauri::command(async)]
pub fn get_wsl_status(
    app: AppHandle,
    id: String,
    path: Option<String>,
) -> Result<SshRemoteStatus, String> {
    let profile = load_profile_for_agent(&app, id.trim())?;
    let checked_path = path
        .filter(|value| !value.trim().is_empty())
        .or_else(|| profile.default_root.clone())
        .unwrap_or_else(|| "/".to_string());
    let started = Instant::now();
    let command = format!("{}; {}", r#"printf '__AT_HOST__ %s\n' "$(hostname)"; printf '__AT_PLATFORM__ %s\n' "$(uname -s)"; printf '__AT_CPU__ %s\n' "$(getconf _NPROCESSORS_ONLN 2>/dev/null)"; free -b 2>/dev/null | awk 'NR==2 {printf "__AT_MEM__ %s %s\n", $2, $7}'; df -Pk -- "$PWD" 2>/dev/null | awk 'NR==2 && $2>0 {gsub("%", "", $5); printf "__AT_DISK__ %.0f %.0f %s %.0f\n", $2*1024, $4*1024, $5, $3*1024}'"#, agent_core::LINUX_RESOURCE_PROBE);
    let output = run_wsl(&profile, Some(&checked_path), &command)?;
    if !output.status.success() {
        return Err(format!(
            "WSL 状态读取失败：{}",
            clean_wsl_output(&output.stderr)
        ));
    }
    Ok(agent_core::parse_wsl_resource_status(
        &clean_wsl_output(&output.stdout),
        started.elapsed().as_millis() as u64,
        checked_path,
        &format!("wsl:{}", profile.distribution),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn streams_workspace_output_before_command_finishes() {
        let Ok(distribution) = std::env::var("ANDROID_TOOL_AOSP_TEST_WSL") else { return };
        let profile = WslProfileConfig {
            id: "aosp-test".to_string(),
            name: "AOSP output test".to_string(),
            distribution,
            default_root: None,
            shell: "bash".to_string(),
        };
        let (send, receive) = std::sync::mpsc::channel();
        let sink = agent_core::CommandOutputSink::new(Arc::new(move |event| {
            if let Some(text) = event.get("text").and_then(serde_json::Value::as_str) {
                let _ = send.send(text.to_string());
            }
        }), "test", "", "aosp").continuous();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let worker = std::thread::spawn(move || {
            let output = run_wsl_workspace_command_streaming(&profile, "/tmp",
                "printf 'stream-before-exit\\n'; sleep 2; printf 'final-output\\n' >&2",
                &Arc::new(AtomicBool::new(false)), Duration::from_secs(30), Some(sink));
            done.store(true, Ordering::Relaxed);
            output
        });
        let started = Instant::now();
        let mut received = String::new();
        while !received.contains("stream-before-exit") {
            received.push_str(&receive.recv_timeout(Duration::from_secs(20).saturating_sub(started.elapsed())).unwrap());
        }
        assert!(!finished.load(Ordering::Relaxed), "Output must arrive while the command is still running");
        let output = worker.join().unwrap().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("final-output"));
        received.extend(receive.try_iter());
        assert!(received.contains("final-output"));
    }

    #[test]
    fn parses_distribution_state_and_version_from_wsl_output() {
        let distributions = parse_distribution_lines(
            b"\0 \0N\0A\0M\0E\0  \0S\0T\0A\0T\0E\0  \0V\0E\0R\0S\0I\0O\0N\0\r\0\n\0*\0  \0U\0b\0u\0n\0t\0u\0-\02\02\0.\00\04\0  \0R\0u\0n\0n\0i\0n\0g\0  \02\0\r\0\n\0  \0d\0o\0c\0k\0e\0r\0-\0d\0e\0s\0k\0t\0o\0p\0  \0S\0t\0o\0p\0p\0e\0d\0  \02\0\r\0\n",
        );
        assert_eq!(distributions.len(), 2);
        assert_eq!(distributions[0].name, "Ubuntu-22.04");
        assert_eq!(distributions[0].state.as_deref(), Some("Running"));
        assert_eq!(distributions[0].version, Some(2));
        assert!(distributions[0].is_default);
        assert_eq!(distributions[1].state.as_deref(), Some("Stopped"));
    }

    #[test]
    fn keeps_absolute_wsl_paths_without_running_a_shell() {
        let profile = WslProfileConfig {
            id: "ubuntu".to_string(),
            name: "Ubuntu".to_string(),
            distribution: "Ubuntu-22.04".to_string(),
            default_root: None,
            shell: "bash".to_string(),
        };
        assert_eq!(
            resolve_wsl_home_path(&profile, "/home/dev/workspace/android14").unwrap(),
            "/home/dev/workspace/android14"
        );
    }
}
