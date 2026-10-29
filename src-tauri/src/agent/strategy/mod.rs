/// P6/S 段：工具权限与提问两档超时的读取、覆盖与钳位。
///
/// 从一个 3500 行的大文件里拆出来（G5-C），行为与顺序完全不变——
/// `turn.rs` 只保留 turn 主流程本身。

/// Tool approval prompts expire; an unattended prompt must not block a turn.
/// P6-B：默认 300 秒（P6-A 从 30s 放宽而来），可经 `~/.android-tool/agent-settings.json`
/// 的 `permissionTimeoutSec`（1–3600，钳位）覆盖。超时走合成 Deny + N1 的
/// permission_resolved 收尾。
/// P6-C：`permissionTimeoutSec: 0` 表示**完全不超时**（crush 语义）——权限条
/// 只能由用户决策或停止回合来收尾；安全性由 P1 的取消链路兜底（随时可停）。
/// P6-B UI：设置页写入后经 [`set_permission_timeout_override`] 立即生效，
/// 不必重启；未经过设置页时仍按进程内缓存的文件值。
pub(super) fn tool_permission_timeout() -> Option<std::time::Duration> {
    let seconds = permission_timeout_override().unwrap_or_else(|| {
        static TIMEOUT_SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *TIMEOUT_SECS.get_or_init(|| load_timeout_secs("permissionTimeoutSec"))
    });
    (seconds > 0).then(|| std::time::Duration::from_secs(seconds))
}

/// P6-D：`ask_user` 等交互提问单独一档（缺省 300 秒，与权限同源钳位）。
pub(super) fn tool_question_timeout() -> Option<std::time::Duration> {
    let seconds = question_timeout_override().unwrap_or_else(|| {
        static TIMEOUT_SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *TIMEOUT_SECS.get_or_init(|| load_timeout_secs("questionTimeoutSec"))
    });
    (seconds > 0).then(|| std::time::Duration::from_secs(seconds))
}

/// 从 `agent-settings.json` 读取某一档超时；缺失/非法时回落到默认 300 秒。
fn load_timeout_secs(key: &str) -> u64 {
    let Ok(root) = crate::user_storage::root() else {
        return 300;
    };
    let Ok(text) = std::fs::read_to_string(root.join("agent-settings.json")) else {
        return 300;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get(key).and_then(serde_json::Value::as_u64))
        .unwrap_or(300)
        .min(3600)
}

/// P6-B UI：设置页写入的覆盖值，优先级高于进程内缓存的文件值。
/// P6-D：权限与提问各自一档，`None` 表示「未设置，跟随文件/默认」。
static PERMISSION_TIMEOUT_OVERRIDE: std::sync::OnceLock<(
    std::sync::atomic::AtomicU64,
    std::sync::atomic::AtomicBool,
)> = std::sync::OnceLock::new();
static QUESTION_TIMEOUT_OVERRIDE: std::sync::OnceLock<(
    std::sync::atomic::AtomicU64,
    std::sync::atomic::AtomicBool,
)> = std::sync::OnceLock::new();

fn permission_timeout_override_cell(
) -> &'static (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicBool) {
    PERMISSION_TIMEOUT_OVERRIDE.get_or_init(|| {
        (
            std::sync::atomic::AtomicU64::new(0),
            std::sync::atomic::AtomicBool::new(false),
        )
    })
}

fn question_timeout_override_cell(
) -> &'static (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicBool) {
    QUESTION_TIMEOUT_OVERRIDE.get_or_init(|| {
        (
            std::sync::atomic::AtomicU64::new(0),
            std::sync::atomic::AtomicBool::new(false),
        )
    })
}

fn override_value(
    cell: &'static (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicBool),
) -> Option<u64> {
    let (value, set) = cell;
    set.load(std::sync::atomic::Ordering::Relaxed)
        .then(|| value.load(std::sync::atomic::Ordering::Relaxed))
}

fn permission_timeout_override() -> Option<u64> {
    override_value(permission_timeout_override_cell())
}

fn question_timeout_override() -> Option<u64> {
    override_value(question_timeout_override_cell())
}

/// 设置页保存后写入内存覆盖值；0 表示永不超时。
pub(crate) fn set_permission_timeout_override(seconds: u64) {
    let (value, set) = permission_timeout_override_cell();
    value.store(seconds, std::sync::atomic::Ordering::Relaxed);
    set.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// P6-D：提问（ask_user）单独一档超时；0 表示永不超时。
pub(crate) fn set_question_timeout_override(seconds: u64) {
    let (value, set) = question_timeout_override_cell();
    value.store(seconds, std::sync::atomic::Ordering::Relaxed);
    set.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Decide whether the next request must be hard-compacted before it is sent.
///
/// The local estimate alone is not a safe gate: it undercounts the real charge by a few
/// percent, so a provider-reported over-window charge forces the compaction even while
/// the estimate still looks affordable. `reported_usage_compacted` keeps that to a single
/// extra compaction per turn.
pub(super) fn context_window_budget_exceeded(
    estimated_tokens: usize,
    reported_tokens: Option<usize>,
    reported_usage_compacted: bool,
    reserved_output_tokens: usize,
    context_window_limit: usize,
) -> bool {
    if estimated_tokens.saturating_add(reserved_output_tokens) > context_window_limit {
        return true;
    }
    !reported_usage_compacted
        && reported_tokens.is_some_and(|reported| {
            reported.saturating_add(reserved_output_tokens) > context_window_limit
        })
}
