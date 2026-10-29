// P1-E 的等待语义单独成文件，方便在不拉起整条准备流水线的情况下做行为锁。
// 放在独立模块而不是 turn.rs：turn.rs 已经三千多行，准备阶段的超时逻辑
// 与「准备什么」无关，只与「等了多久」有关。

/// 准备阶段（SSH 握手 / WSL 探测 / Skills 索引）的默认整体超时。
/// 与工具权限超时（P6-B，默认 300s）无关：这里等的是本地/远端环境就绪，
/// 卡住时用户看到的是「一直正在准备」，30s 足够判定异常。
pub(super) const DEFAULT_PREPARE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 超时后的报错文案。带上可重试提示，前端直接展示即可。
pub(super) fn prepare_timeout_message(limit: std::time::Duration) -> String {
    format!(
        "[prepare_timeout] 准备执行环境超过 {} 秒仍未完成（SSH 连接、WSL 探测或 Skills 索引卡住）。本次请求未执行，可直接重试或切换执行目标。",
        limit.as_secs()
    )
}

/// 把准备阶段的 future 套上整体超时。
///
/// 语义与 `tokio::select!` 里的取消分支一致：超时即丢弃 future 并返回同样的
/// 可重试错误，不做硬中断（已发出的 SSH 握手照常在后台自行收尾）。超时会先
/// 置取消标志，让仍在执行的准备步骤在下一个检查点自行短路，避免后台继续
/// 占用连接。
pub(super) async fn prepare_with_timeout<F, T>(
    limit: std::time::Duration,
    cancellation: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    prepare: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(limit, prepare).await {
        Ok(result) => result,
        Err(_) => {
            cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
            Err(prepare_timeout_message(limit))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn a_stuck_prepare_step_fails_with_a_retryable_timeout_and_marks_cancelled() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let result = prepare_with_timeout(
            std::time::Duration::from_millis(20),
            &cancellation,
            std::future::pending::<Result<(), String>>(),
        )
        .await;
        let error = result.expect_err("卡住的准备必须超时失败");
        assert!(error.starts_with("[prepare_timeout]"), "{error}");
        assert!(error.contains("可直接重试"), "{error}");
        // 超时同时置取消标志：后台仍在跑的步骤会在下一个检查点短路。
        assert!(cancellation.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_fast_prepare_step_passes_through_untouched() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let result =
            prepare_with_timeout(std::time::Duration::from_secs(5), &cancellation, async {
                Ok::<_, String>("ready")
            })
            .await;
        assert_eq!(result.unwrap(), "ready");
        assert!(!cancellation.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn prepare_errors_are_not_rewritten_into_timeouts() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let result =
            prepare_with_timeout(std::time::Duration::from_secs(5), &cancellation, async {
                Err::<(), _>("已跳过 SSH 连接，本轮远程操作未执行。".to_string())
            })
            .await;
        assert_eq!(result.unwrap_err(), "已跳过 SSH 连接，本轮远程操作未执行。");
        assert!(!cancellation.load(Ordering::Relaxed));
    }
}
