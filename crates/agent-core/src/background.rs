//! Host-independent execution for shells whose lifetime exceeds an AI turn.
use crate::{AllowPermission, CommandOutputSink, LocalToolContext, SshToolContext, ToolRegistry};
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Arc};

pub const MAX_BACKGROUND_TIMEOUT_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;
pub const BACKGROUND_RESULT_PREFIX: &str = "background-result:";

/// A task name is model-authored display text, never shell input.
pub fn background_task_title(input: &Value) -> Result<String, String> {
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if title.is_empty()
        || title.chars().count() > 60
        || title.chars().any(char::is_control)
        || !title
            .chars()
            .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        return Err("后台任务需要 title：请用不超过 60 字的简洁中文说明任务目的，例如“编译 CarAdapter 客户端”，不要填写 shell 指令。补充 title 后重新调用。".into());
    }
    Ok(title.to_string())
}

/// Older saved jobs have no model-authored name; do not expose a command as one.
pub fn legacy_background_task_title(command: &str) -> &'static str {
    let command = command.to_ascii_lowercase();
    if command.contains("bitbake") {
        "Yocto 编译任务"
    } else if command.contains("lunch ") || command.contains("ninja ") {
        "Android 编译任务"
    } else if command.contains("gradle") {
        "Gradle 构建任务"
    } else {
        "后台执行任务"
    }
}

#[derive(Clone)]
pub enum BackgroundShellContext {
    Local(LocalToolContext),
    Ssh(SshToolContext),
}

impl BackgroundShellContext {
    pub fn environment_label(&self) -> &'static str {
        match self {
            Self::Local(context) => context.environment_label(),
            Self::Ssh(_) => "ssh",
        }
    }
    /// The host must authorize the original shell input before calling this.
    /// Detached cancellation and output never borrow the originating turn.
    pub async fn execute(
        &self,
        mut input: Value,
        cancellation: Arc<AtomicBool>,
        sink: CommandOutputSink,
    ) -> Result<Value, String> {
        let timeout = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_BACKGROUND_TIMEOUT_MS)
            .clamp(1000, MAX_BACKGROUND_TIMEOUT_MS);
        let object = input.as_object_mut().ok_or("后台 shell 参数必须是对象")?;
        object.remove("background");
        object.remove("title");
        object.insert("timeout_ms".into(), json!(timeout));
        let mut registry = ToolRegistry::new();
        match self {
            Self::Local(context) => crate::register_local_tools(
                &mut registry,
                context
                    .for_background()
                    .with_cancellation(cancellation)
                    .with_output_sink(Some(sink)),
            )?,
            Self::Ssh(context) => crate::register_ssh_tools(
                &mut registry,
                context
                    .for_background()
                    .with_cancellation(cancellation)
                    .with_output_sink(Some(sink)),
            )?,
        }
        registry
            .execute("run_local_shell", input, &AllowPermission)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Preserve the end of UTF-8 logs, where builds normally report their result.
pub fn append_log_tail(tail: &mut String, text: &str, max_bytes: usize) {
    tail.push_str(text);
    if tail.len() > max_bytes {
        let mut start = tail.len() - max_bytes;
        while !tail.is_char_boundary(start) {
            start += 1;
        }
        tail.drain(..start);
    }
}

pub fn terminal_status(result: &Result<Value, String>, cancelled: bool) -> &'static str {
    if cancelled {
        return "cancelled";
    }
    match result {
        Ok(value)
            if value.get("success").and_then(Value::as_bool) == Some(true)
                && value.get("exitCode").and_then(Value::as_i64) == Some(0) =>
        {
            "done"
        }
        Ok(_) => "error",
        Err(error) if error.contains("超时") || error.contains("timed out") => "timeout",
        Err(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_title_requires_short_chinese_display_text() {
        assert_eq!(
            background_task_title(&json!({"title":" 编译 CarAdapter 客户端 "})).unwrap(),
            "编译 CarAdapter 客户端"
        );
        for input in [
            json!({}),
            json!({"title":42}),
            json!({"title":"bitbake caradapter-client"}),
            json!({"title":"编译\n执行"}),
            json!({"title":"编".repeat(61)}),
        ] {
            assert!(background_task_title(&input).is_err());
        }
        assert_eq!(
            legacy_background_task_title("cd /repo && bitbake caradapter-client"),
            "Yocto 编译任务"
        );
        assert_eq!(legacy_background_task_title("sleep 60"), "后台执行任务");
    }
    #[test]
    fn tail_is_bounded_and_keeps_unicode() {
        let mut tail = "旧输出".to_string();
        append_log_tail(&mut tail, "\n编译错误❌", 13);
        assert!(tail.len() <= 13);
        assert!(tail.ends_with("错误❌"));
    }
    #[test]
    fn unknown_exit_is_never_success() {
        assert_eq!(
            terminal_status(&Ok(json!({"exitCode": null})), false),
            "error"
        );
        assert_eq!(
            terminal_status(&Ok(json!({"success":true,"exitCode":null})), false),
            "error"
        );
        assert_eq!(
            terminal_status(&Ok(json!({"success":true,"exitCode":0})), false),
            "done"
        );
        assert_eq!(
            terminal_status(&Ok(json!({"success": true})), true),
            "cancelled"
        );
        assert_eq!(terminal_status(&Err("命令超时".into()), false), "timeout");
    }
    #[tokio::test]
    async fn detached_shell_streams_after_origin_turn_cancelled() {
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let origin_cancel = Arc::new(AtomicBool::new(true));
        let context = BackgroundShellContext::Local(LocalToolContext::new(
            vec![root.clone()],
            vec![],
            false,
            origin_cancel,
            None,
        ));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |value| captured.lock().unwrap().push(value)),
            "s",
            "t",
            "shell",
        )
        .continuous();
        let (shell, command) = if cfg!(windows) {
            (
                "powershell",
                "Write-Output '后台开始'; Start-Sleep -Milliseconds 80; Write-Output '后台完成'",
            )
        } else {
            (
                "bash",
                "printf '后台开始\n'; sleep 0.08; printf '后台完成\n'",
            )
        };
        let result = context
            .execute(
                json!({"shell":shell,"command":command,"cwd":root,"title":"验证后台输出","background":true}),
                Arc::new(AtomicBool::new(false)),
                sink,
            )
            .await
            .unwrap();
        assert_eq!(result["success"], true);
        assert!(result["stdout"].as_str().unwrap().contains("后台完成"));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["phase"] == "chunk"));
    }

    #[tokio::test]
    async fn background_failure_preserves_exit_code() {
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let context = BackgroundShellContext::Local(LocalToolContext::new(
            vec![root.clone()],
            vec![],
            false,
            Arc::new(AtomicBool::new(false)),
            None,
        ));
        let sink = CommandOutputSink::new(Arc::new(|_| {}), "s", "t", "shell");
        let shell = if cfg!(windows) { "powershell" } else { "bash" };
        let result = context
            .execute(
                json!({"shell":shell,"command":"exit 23","cwd":root}),
                Arc::new(AtomicBool::new(false)),
                sink,
            )
            .await
            .unwrap();
        assert_eq!(result["exitCode"], 23);
        assert_eq!(result["success"], false);
    }

    #[tokio::test]
    async fn background_has_its_own_cancellation() {
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let context = BackgroundShellContext::Local(LocalToolContext::new(
            vec![root.clone()],
            vec![],
            false,
            Arc::new(AtomicBool::new(false)),
            None,
        ));
        let sink = CommandOutputSink::new(Arc::new(|_| {}), "s", "t", "shell");
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let (shell, command) = if cfg!(windows) {
            ("powershell", "Start-Sleep -Seconds 60")
        } else {
            ("bash", "sleep 60")
        };
        let result = context
            .execute(
                json!({"shell":shell,"command":command,"cwd":root}),
                cancel,
                sink,
            )
            .await;
        assert!(result.unwrap_err().contains("取消"));
    }
}
