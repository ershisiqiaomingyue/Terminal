//! A Linux process group belongs to one WSL tool call, not to wsl.exe's Windows tree.

use crate::owned_process::{valid_control_root, DRAIN, LEADER, PREPARE, REMOVE, SUPERVISOR};
use crate::process::{execute_process_with_cleanup, ProcessCleanup, PROCESS_CLEANUP_ERROR_PREFIX};
use crate::{CommandExecutionResult, CommandOutputSink};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct WslProcessGuard {
    distribution: String,
    root: String,
}

async fn control(distribution: &str, script: &str, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("wsl.exe");
    command.creation_flags(0x08000000);
    command
        .args([
            "--distribution",
            distribution,
            "--exec",
            "sh",
            "-c",
            script,
            "android-tool-control",
        ])
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "WSL 进程控制超时，无法确认子进程已结束".to_string())?
        .map_err(|error| format!("WSL 进程控制启动失败: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "WSL 进程控制失败（{}），无法确认子进程已结束",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[async_trait::async_trait]
impl ProcessCleanup for WslProcessGuard {
    async fn before_terminate(&self) -> Result<(), String> {
        control(&self.distribution, DRAIN, &[&self.root])
            .await
            .map(|_| ())
            .map_err(|error| format!("{PROCESS_CLEANUP_ERROR_PREFIX}{error}"))
    }
}

pub(crate) async fn execute(
    distribution: &str,
    linux_cwd: &str,
    executable: &str,
    args: &[String],
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
    sink: Option<&CommandOutputSink>,
) -> Result<CommandExecutionResult, String> {
    let mut terminal_override = Some("setup_error");
    let terminal = Arc::new(Mutex::new(None::<Value>));
    let deferred_sink = sink.map(|sink| {
        let sink = sink.clone();
        let terminal = terminal.clone();
        CommandOutputSink::new(
            Arc::new(move |event| {
                if event["phase"] == "finished" {
                    *terminal.lock().unwrap() = Some(event);
                } else {
                    sink.emit_remote_chunk(
                        event["output_sequence"].as_u64().unwrap_or_default(),
                        event["stream"].as_str().unwrap_or("stdout"),
                        event["text"].as_str().unwrap_or_default(),
                    );
                }
            }),
            "wsl-process",
            "wsl-process",
            executable,
        )
    });
    let result = async {
        if cancellation.load(Ordering::Relaxed) {
            terminal_override = Some("cancelled");
            return Err("请求已取消".to_string());
        }
        let root = control(distribution, PREPARE, &[]).await?;
        if !valid_control_root(&root) {
            return Err("WSL 返回了无效的进程控制目录，命令未执行".to_string());
        }
        let guard = WslProcessGuard {
            distribution: distribution.into(),
            root,
        };
        let mut wsl_args = vec![
            "--distribution".into(),
            distribution.into(),
            "--cd".into(),
            linux_cwd.into(),
            "--exec".into(),
            "sh".into(),
            "-c".into(),
            SUPERVISOR.into(),
            "android-tool-supervisor".into(),
            guard.root.clone(),
            LEADER.into(),
            executable.into(),
        ];
        wsl_args.extend(args.iter().cloned());
        let result = if cancellation.load(Ordering::Relaxed) {
            terminal_override = Some("cancelled");
            Err("请求已取消".to_string())
        } else {
            terminal_override = None;
            execute_process_with_cleanup(
                PathBuf::from("wsl.exe"),
                &wsl_args,
                Path::new("."),
                timeout_ms,
                cancellation.clone(),
                deferred_sink.as_ref(),
                Some(&guard),
            )
            .await
        };
        // Also drain on normal completion, including command-created background children.
        if let Err(error) =
            control(distribution, &format!("{DRAIN}\n{REMOVE}"), &[&guard.root]).await
        {
            terminal_override = Some("cleanup_error");
            return Err(format!("{PROCESS_CLEANUP_ERROR_PREFIX}{error}"));
        }
        result
    }
    .await;
    if let Some(sink) = sink {
        let event = terminal.lock().unwrap().take();
        let termination = terminal_override.or_else(|| {
            event
                .as_ref()
                .and_then(|event| event["termination"].as_str())
        });
        let mut summary = event
            .as_ref()
            .map(|event| event["summary"].clone())
            .unwrap_or_else(|| json!({"success": false}));
        let mut details = event
            .as_ref()
            .map(|event| event["details"].clone())
            .unwrap_or_else(|| json!({}));
        if let Err(error) = &result {
            summary["success"] = json!(false);
            details["error"] = json!(error);
        }
        summary["termination"] = json!(termination);
        sink.emit_remote_finished(termination, summary, details);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn control_root_cannot_escape_the_private_namespace() {
        assert!(super::valid_control_root(
            "/tmp/android-tool-process.ab12CD34ef56"
        ));
        for root in [
            "/tmp",
            "/",
            "/tmp/android-tool-process../etc",
            "/tmp/android-tool-process.abc",
            "/tmp/android-tool-process.ab12CD34ef56\n",
        ] {
            assert!(!super::valid_control_root(root));
        }
    }

    #[tokio::test]
    async fn already_cancelled_wsl_command_never_starts_a_distribution() {
        let result = super::execute(
            "not-a-real-distro",
            "/tmp",
            "not-a-real-command",
            &[],
            1,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            None,
        )
        .await;
        assert!(matches!(result, Err(message) if message == "请求已取消"));
    }

    #[tokio::test]
    #[ignore = "Requires ANDROID_DSH_TEST_WSL; runs only test-owned Linux commands"]
    async fn real_wsl_arguments_failures_and_parallel_ownership() {
        use super::*;
        let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select a test distro");
        let text = "WSL 空 格 ' \" $ ; &";
        let args = [
            "-c".into(),
            "printf '%s\\n' \"$1\"; printf err >&2; exit 7".into(),
            "test".into(),
            text.into(),
        ];
        let result = execute(
            &distribution,
            "/tmp",
            "sh",
            &args,
            10_000,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(result.status.code(), Some(7));
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), text);
        assert!(String::from_utf8_lossy(&result.stderr).contains("err"));
        let result = execute(
            &distribution,
            "/tmp",
            "android-tool-no-such-executable",
            &[],
            10_000,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(result.status.code(), Some(127));

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_on_output = cancel.clone();
        let terminal_seen = Arc::new(AtomicBool::new(false));
        let receipt = terminal_seen.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| {
                if event["stream"] == "stdout"
                    && event["text"].as_str().unwrap_or_default().contains("ready")
                {
                    cancel_on_output.store(true, Ordering::Relaxed);
                }
                if event["phase"] == "finished" {
                    assert_eq!(event["termination"], "cancelled");
                    assert_eq!(event["summary"]["success"], false);
                    receipt.store(true, Ordering::Relaxed);
                }
            }),
            "test",
            "test",
            "test",
        );
        let cancelled_args = ["-c".into(), "printf 'ready\\n'; sleep 45".into()];
        let sentinel_args = ["-c".into(), "sleep 2; printf 'sentinel-ok\\n'".into()];
        let (cancelled, sentinel) = tokio::join!(
            execute(
                &distribution,
                "/tmp",
                "sh",
                &cancelled_args,
                10_000,
                cancel,
                Some(&sink)
            ),
            execute(
                &distribution,
                "/tmp",
                "sh",
                &sentinel_args,
                10_000,
                Arc::new(AtomicBool::new(false)),
                None
            )
        );
        assert!(matches!(cancelled, Err(message) if message.contains("取消")));
        assert!(terminal_seen.load(Ordering::Relaxed));
        let sentinel = sentinel.unwrap_or_else(|error| panic!("{error}"));
        assert!(sentinel.status.success());
        assert_eq!(
            String::from_utf8_lossy(&sentinel.stdout).trim(),
            "sentinel-ok"
        );
    }
}
