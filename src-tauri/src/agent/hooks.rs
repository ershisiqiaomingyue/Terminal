//! G4-D：Rust 侧最小 TurnHook 面——外部命令 hook，单注册点，先覆盖有
//! 决策语义的两个事件（PreToolUse 可 Block/Modify，PostToolUse 通知）。
//!
//! 四条硬约束（总纲 G4）：
//! 1. 必须能 Block——PreToolUse 返回 Block 时工具不执行；
//! 2. 必须有超时——默认 10s（可按 handler 配置），超时杀进程；
//! 3. 失败必须 fail-safe——handler 非零退出/超时/输出不可解析 → Block 该工具
//!    （pi 原则：a tool_call handler failure blocks the tool as a fail-safe）；
//! 4. 注册点只有一个——本文件是唯一入口，接入方只调 run_*，不直接碰配置。
//!
//! 配置：`~/.android-tool/hooks.json`（启动加载一次，改配置需重启应用）：
//! ```json
//! {
//!   "PreToolUse":  [{ "matcher": "shell", "command": "python check.py", "timeout_sec": 10 }],
//!   "PostToolUse": [{ "matcher": "*",      "command": "python log.py" }]
//! }
//! ```
//! handler 协议：stdin 收 `{event, tool_name, input, is_error}`；stdout 答一个
//! JSON 决策 `{"decision":"allow"}` / `{"decision":"block","reason":"..."}`
//! / `{"decision":"modify","input":{...}}`；stdout 为空或 `allow` 等价放行。

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone)]
struct HookHandler {
    matcher: String,
    command: String,
    timeout: Duration,
}

#[derive(Debug, Clone, Default)]
struct HookConfig {
    pre_tool_use: Vec<HookHandler>,
    post_tool_use: Vec<HookHandler>,
    /// G4-A 最小增量：三个通知型生命周期事件（无决策语义，纯通知）。
    user_prompt_submit: Vec<HookHandler>,
    turn_end: Vec<HookHandler>,
    interrupt: Vec<HookHandler>,
    /// G4-A：同步 request/response 的 TurnHook 通道——handler 的 stdout 直接
    /// 作为工具结果**回注**给模型（而不是像 PreToolUse 那样只做放行判断）。
    /// 这是插件化能力的第一个可用面：外部命令可以「代模型执行」某个工具。
    sync_turn_hook: Vec<HookHandler>,
}

impl HookConfig {
    fn load() -> HookConfig {
        let Ok(root) = crate::user_storage::root() else {
            return HookConfig::default();
        };
        let Ok(text) = std::fs::read_to_string(root.join("hooks.json")) else {
            return HookConfig::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            eprintln!("hooks.json 不是合法 JSON，全部 hook 被忽略");
            return HookConfig::default();
        };
        let parse = |key: &str| -> Vec<HookHandler> {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let command = entry.get("command")?.as_str()?.to_string();
                            if command.trim().is_empty() {
                                return None;
                            }
                            Some(HookHandler {
                                matcher: entry
                                    .get("matcher")
                                    .and_then(Value::as_str)
                                    .unwrap_or("*")
                                    .to_string(),
                                command,
                                timeout: Duration::from_secs(
                                    entry
                                        .get("timeout_sec")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(10)
                                        .clamp(1, 120),
                                ),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        HookConfig {
            pre_tool_use: parse("PreToolUse"),
            post_tool_use: parse("PostToolUse"),
            user_prompt_submit: parse("UserPromptSubmit"),
            turn_end: parse("TurnEnd"),
            interrupt: parse("Interrupt"),
            sync_turn_hook: parse("TurnHook"),
        }
    }

    fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
            && self.post_tool_use.is_empty()
            && self.user_prompt_submit.is_empty()
            && self.turn_end.is_empty()
            && self.interrupt.is_empty()
            && self.sync_turn_hook.is_empty()
    }
}

fn hook_config() -> &'static HookConfig {
    static CONFIG: OnceLock<HookConfig> = OnceLock::new();
    CONFIG.get_or_init(HookConfig::load)
}

fn handlers_for<'a>(handlers: &'a [HookHandler], tool_name: &str) -> Vec<&'a HookHandler> {
    handlers
        .iter()
        .filter(|handler| handler.matcher == "*" || handler.matcher == tool_name)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookDecision {
    Allow,
    Block { reason: String },
    Modify { input: Value },
}

/// 运行一个 handler 进程并解析其决策。失败（非零退出/超时/不可解析）返回
/// Block（fail-safe）。stdout 为空视为 allow（纯旁观型 handler 合法）。
async fn run_handler(
    handler: &HookHandler,
    payload: &Value,
) -> Result<Option<HookDecision>, String> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.arg("/C").arg(&handler.command);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(&handler.command);
        command
    };
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("无法启动 hook 命令: {error}"))?;
    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin
            .write_all(
                serde_json::to_string(payload)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .await;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(handler.timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("hook 超时（{}s）", handler.timeout.as_secs()))?
        .map_err(|error| format!("hook 执行失败: {error}"))?;
    if !output.status.success() {
        return Err(format!("hook 退出码 {:?}", output.status.code()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None);
    }
    let decision: Value =
        serde_json::from_str(&stdout).map_err(|error| format!("hook 输出不可解析: {error}"))?;
    match decision.get("decision").and_then(Value::as_str) {
        Some("allow") => Ok(Some(HookDecision::Allow)),
        Some("block") => Ok(Some(HookDecision::Block {
            reason: decision
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("被 PreToolUse hook 拦截")
                .to_string(),
        })),
        Some("modify") => {
            let input = decision.get("input").cloned().ok_or("modify 缺少 input")?;
            Ok(Some(HookDecision::Modify { input }))
        }
        _ => Err("hook 决策未知（allow/block/modify）".to_string()),
    }
}

/// PreToolUse：按注册顺序运行匹配的 handler，任一 Block 立即返回；后面的
/// Modify 覆盖前面的。无匹配 handler 时零开销放行。
pub async fn run_pre_tool_hooks(tool_name: &str, input: &Value) -> HookDecision {
    let config = hook_config();
    if config.is_empty() {
        return HookDecision::Allow;
    }
    let mut decision = HookDecision::Allow;
    let mut current_input = input.clone();
    for handler in handlers_for(&config.pre_tool_use, tool_name) {
        let payload = json!({
            "event": "PreToolUse",
            "tool_name": tool_name,
            "input": current_input,
        });
        match run_handler(handler, &payload).await {
            // fail-safe：handler 失败即 Block 该工具。
            Err(reason) => {
                return HookDecision::Block { reason };
            }
            Ok(Some(HookDecision::Block { reason })) => return HookDecision::Block { reason },
            Ok(Some(HookDecision::Modify { input: modified })) => {
                current_input = modified;
                decision = HookDecision::Modify {
                    input: current_input.clone(),
                };
            }
            Ok(Some(HookDecision::Allow)) | Ok(None) => {}
        }
    }
    decision
}

/// PostToolUse：纯通知（不产生决策），handler 失败只记日志——结果已经发生，
/// fail-safe 无从谈起，也不应拖垮回合。
pub async fn run_post_tool_hooks(tool_name: &str, output: &Value, is_error: bool) {
    let config = hook_config();
    if config.post_tool_use.is_empty() {
        return;
    }
    for handler in handlers_for(&config.post_tool_use, tool_name) {
        let payload = json!({
            "event": "PostToolUse",
            "tool_name": tool_name,
            "output": output,
            "is_error": is_error,
        });
        if let Err(error) = run_handler(handler, &payload).await {
            eprintln!("PostToolUse hook 失败（已忽略）: {error}");
        }
    }
}

/// G4-A 最小增量：通知型生命周期 hook 的统一入口（无决策语义——handler
/// 失败只记日志；无 turn/工具过滤，按配置顺序全部运行）。
async fn run_notify_hooks(event: &str, handlers: &[HookHandler], payload: Value) {
    for handler in handlers {
        if let Err(error) = run_handler(handler, &payload).await {
            eprintln!("{event} hook 失败（已忽略）: {error}");
        }
    }
}

/// UserPromptSubmit：用户输入被受理后（turn_start/steer 接受时）通知。
pub async fn run_user_prompt_submit_hooks(session_id: &str, turn_id: &str, message: &str) {
    let config = hook_config();
    if config.user_prompt_submit.is_empty() {
        return;
    }
    run_notify_hooks(
        "UserPromptSubmit",
        &config.user_prompt_submit,
        json!({ "event": "UserPromptSubmit", "session_id": session_id, "turn_id": turn_id, "message": message }),
    )
    .await;
}

/// TurnEnd：turn 到达终态（completed/cancelled/failed）后通知。
pub async fn run_turn_end_hooks(session_id: &str, turn_id: &str, status: &str) {
    let config = hook_config();
    if config.turn_end.is_empty() {
        return;
    }
    run_notify_hooks(
        "TurnEnd",
        &config.turn_end,
        json!({ "event": "TurnEnd", "session_id": session_id, "turn_id": turn_id, "status": status }),
    )
    .await;
}

/// Interrupt：用户打断回合时通知（打断处理尾部，与 codex 的 Interrupt hook
/// 同位置——「Right before an interrupted turn is aborted」）。
pub async fn run_interrupt_hooks(session_id: &str, turn_id: &str) {
    let config = hook_config();
    if config.interrupt.is_empty() {
        return;
    }
    run_notify_hooks(
        "Interrupt",
        &config.interrupt,
        json!({ "event": "Interrupt", "session_id": session_id, "turn_id": turn_id }),
    )
    .await;
}

/// 供 diagnostics 询问 hook 面是否启用（无配置时接入点零开销）。
pub fn hooks_enabled() -> bool {
    !hook_config().is_empty()
}

/// G4-A 完整形态：**同步 request/response** 的 TurnHook 通道。
///
/// 与 PreToolUse 的区别：PreToolUse 只回答「允不允许」，这里 handler 的
/// stdout 会被当作**工具结果**回注给模型——外部命令因此可以真正接管某个
/// 工具（插件的第一种可用形态：把厂商 SDK / 脚本包成工具）。
///
/// 契约（fail-safe，与 PreToolUse 相反的方向）：
/// - 无匹配 handler → `None`，调用方照常执行内置工具。
/// - handler 失败（非零退出/超时/不可解析）→ `Err(reason)`，调用方把这段
///   文字当作工具错误回注，**不静默降级**到内置实现（模型会看到原因）。
/// - handler 成功 → `Ok(Some(text))`，`text` 即工具结果（空 stdout 表示
///   成功但无输出）。
pub async fn run_turn_hooks(
    session_id: &str,
    turn_id: &str,
    tool_name: &str,
    input: &Value,
) -> Option<Result<String, String>> {
    let config = hook_config();
    let handlers = handlers_for(&config.sync_turn_hook, tool_name);
    if handlers.is_empty() {
        return None;
    }
    let payload = json!({
        "event": "TurnHook",
        "session_id": session_id,
        "turn_id": turn_id,
        "tool_name": tool_name,
        "input": input,
    });
    let mut last: Option<String> = None;
    for handler in handlers {
        match run_raw_handler(handler, &payload).await {
            Ok(text) => last = Some(text),
            Err(reason) => return Some(Err(reason)),
        }
    }
    Some(Ok(last.unwrap_or_default()))
}

/// 与 [`run_handler`] 同一进程协议，但返回**原始 stdout**（TurnHook 用它当
/// 工具结果，不解析 decision 字段）。空 stdout 返回空串。
async fn run_raw_handler(handler: &HookHandler, payload: &Value) -> Result<String, String> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.arg("/C").arg(&handler.command);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(&handler.command);
        command
    };
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("无法启动 hook 命令: {error}"))?;
    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin
            .write_all(
                serde_json::to_string(payload)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .await;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(handler.timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("hook 超时（{}s）", handler.timeout.as_secs()))?
        .map_err(|error| format!("hook 执行失败: {error}"))?;
    if !output.status.success() {
        return Err(format!("hook 退出码 {:?}", output.status.code()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod hook_tests {
    use super::*;

    #[test]
    fn matcher_selects_tools_or_wildcard() {
        let config = HookConfig {
            pre_tool_use: vec![HookHandler {
                matcher: "shell".into(),
                command: "noop".into(),
                timeout: Duration::from_secs(1),
            }],
            post_tool_use: vec![],
            user_prompt_submit: vec![],
            turn_end: vec![],
            interrupt: vec![],
            sync_turn_hook: vec![],
        };
        assert_eq!(handlers_for(&config.pre_tool_use, "shell").len(), 1);
        assert_eq!(
            handlers_for(&config.pre_tool_use, "read_local_file").len(),
            0
        );
        let wildcard = HookConfig {
            pre_tool_use: vec![HookHandler {
                matcher: "*".into(),
                command: "noop".into(),
                timeout: Duration::from_secs(1),
            }],
            post_tool_use: vec![],
            user_prompt_submit: vec![],
            turn_end: vec![],
            interrupt: vec![],
            sync_turn_hook: vec![],
        };
        assert_eq!(handlers_for(&wildcard.pre_tool_use, "anything").len(), 1);
    }

    #[tokio::test]
    async fn absent_config_allows_immediately() {
        // 没有配置文件时（测试环境的 ~/.android-tool 大概率没有 hooks.json，
        // 即便有也只验证放行路径不 panic）。
        let decision = run_pre_tool_hooks("shell", &json!({"command": "ls"})).await;
        assert_eq!(decision, HookDecision::Allow);
    }

    // G4-A：TurnHook 只在有匹配 handler 时才返回 Some；无配置时为零开销直通，
    // 内置工具照常执行（这正是「插件接管」与「静默降级」的分界线）。
    #[tokio::test]
    async fn turn_hook_only_answers_when_a_handler_matches() {
        let outcome = run_turn_hooks("session", "turn", "no_such_tool_xyz", &json!({})).await;
        assert!(outcome.is_none(), "无匹配 handler 时必须让内置实现接手");
    }
}
