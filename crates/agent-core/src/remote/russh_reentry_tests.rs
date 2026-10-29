//! SSH 入口的异步上下文回归测试（v3 §21）。
//!
//! 工具层是 async 的，而 libssh2 / ssh.exe / russh 的入口全是同步的。同步入口
//! 一旦直接跑在 tokio 工作线程上，`Runtime::block_on` 就会 panic：
//!
//! ```text
//! Cannot start a runtime from within a runtime. This happens because a
//! function (like `block_on`) attempted to block the current thread while the
//! thread is being used to drive asynchronous tasks.
//! ```
//!
//! 这个 panic 会被 agent 任务的 JoinError 吞掉，最终只表现为一句「Agent 任务
//! 异常终止」，栈里没有任何业务帧（线上就发生过：`ssh__list_local_files`）。
//!
//! 这里做两层清扫：
//! 1. 所有注册进来的 SSH 工具都在多线程运行时里跑一遍，只允许返回 `Err`
//!    （连不上 `127.0.0.1:1`），不允许 panic；
//! 2. 同步 SSH 原语在 `spawn_blocking` 线程上可以直接调用——这是终端桥与
//!    Tauri 命令层的既有调用约定。

use super::*;
use crate::AllowPermission;

/// 端口 1 上没有任何服务：连接在毫秒级被拒，测试不需要真实 SSH 服务器，
/// 也不会碰系统 ssh 配置。所有断言只关心「有没有 panic」。
fn refused_config(id: &str) -> SshConnectionConfig {
    SshConnectionConfig {
        id: id.to_string(),
        name: id.to_string(),
        host: "127.0.0.1".to_string(),
        port: 1,
        username: "tester".to_string(),
        private_key_path: None,
        known_hosts_path: None,
        strict_host_key: false,
        default_root: Some("/".to_string()),
        reuse_connection: false,
        connect_timeout_secs: 3,
        operation_timeout_secs: 5,
        keepalive_interval_secs: 30,
    }
}

fn refused_context() -> SshToolContext {
    SshToolContext::new(
        refused_config("ssh-reentry-tools"),
        "/",
        None,
        Arc::new(AtomicBool::new(false)),
        None,
    )
}

/// 按 schema 造一份「形状正确」的输入。
///
/// 目的是让工具过掉参数解析、真正走到选择传输引擎与发起 SSH 调用的那段代码——
/// 只有走到那里才会触发 `block_on`。参数填错时工具会提前返回参数错误，测试就
/// 白跑了，所以这里按字段名给合理的占位值。
fn sample_input(definition: &ToolDefinition) -> Value {
    sample_object(&definition.input_schema, 0)
}

fn sample_object(schema: &Value, depth: usize) -> Value {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return json!({});
    };
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut object = serde_json::Map::new();
    for (name, property) in properties {
        // 顶层把可选字段也填上（如 cwd/command），嵌套层只填必填，避免造出
        // 过大的对象。
        if depth > 0 && !required.iter().any(|required| required == name) {
            continue;
        }
        object.insert(name.clone(), sample_value(name, property, depth + 1));
    }
    Value::Object(object)
}

fn sample_value(name: &str, schema: &Value, depth: usize) -> Value {
    if depth > 3 {
        return Value::Null;
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return first.clone();
    }
    match schema.get("type").and_then(Value::as_str).unwrap_or("") {
        "object" => sample_object(schema, depth),
        "array" => {
            let item = schema.get("items").cloned().unwrap_or(Value::Null);
            Value::Array(vec![sample_value(name, &item, depth + 1)])
        }
        "integer" | "number" => json!(1),
        "boolean" => Value::Bool(true),
        _ => Value::String(sample_string(name)),
    }
}

fn sample_string(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("command") || lower.contains("shell") {
        "true".to_string()
    } else if lower.contains("content") || lower.contains("text") || lower.contains("query") {
        "x".to_string()
    } else if lower.contains("mode") || lower.contains("permission") {
        "644".to_string()
    } else if lower.contains("patch") || lower.contains("diff") {
        "--- a\n+++ b\n".to_string()
    } else {
        // path / dir / root / 其余字符串参数：用工作区根目录，保证能走到
        // 「解析路径 → 选引擎 → 连接」这条链路。
        "/".to_string()
    }
}

/// 全量清扫：任何 SSH 工具在 tokio 工作线程上执行都不得 panic。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_tool_handlers_never_nest_runtimes() {
    let registry = Arc::new({
        let mut registry = ToolRegistry::new();
        register_ssh_tools(&mut registry, refused_context()).expect("register ssh tools");
        registry
    });
    let cases = registry
        .list()
        .iter()
        .map(|definition| (definition.name.clone(), sample_input(definition)))
        .collect::<Vec<_>>();
    assert!(
        cases.len() >= 25,
        "SSH 工具数量异常，清扫没有覆盖全部入口：{}",
        cases.len()
    );

    let mut panics = Vec::new();
    for (name, input) in cases {
        let registry = registry.clone();
        let label = name.clone();
        let task = tokio::spawn(async move {
            registry
                .execute(&label, input, &AllowPermission)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        match task.await {
            // 成功或可读的错误都可以：这段断言只关心「没有 panic」。
            Ok(Ok(())) | Ok(Err(_)) => {}
            Err(error) if error.is_panic() => panics.push(name),
            Err(error) => panic!("SSH 工具测试任务异常：{error}"),
        }
    }
    assert!(
        panics.is_empty(),
        "以下 SSH 工具在 tokio 工作线程上 panic（同步 SSH 入口没有走 spawn_blocking）：{panics:?}"
    );
}

/// 同步 SSH 原语的调用约定：允许在 `spawn_blocking` 线程上直接跑（终端桥、Tauri
/// 命令层都是这么用的），返回可读的连接错误而不是 panic。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn russh_sync_entries_are_callable_from_blocking_threads() {
    let config = refused_config("ssh-reentry-blocking");
    let result = tokio::task::spawn_blocking(move || {
        russh_test_connection(&config, None).map_err(|error| error.kind)
    })
    .await
    .expect("spawn_blocking 线程上的同步 SSH 入口不得 panic");
    assert!(result.is_err(), "端口 1 不应该连上任何 SSH 服务");
}

/// 终端桥（`try_open_russh_terminal` 走的 PTY 入口）同样允许从异步上下文发起：
/// 状态面板、会话重连都可能在 async 里碰它。连不上时必须是可读的 `Err`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn russh_pty_bridge_returns_error_instead_of_panicking() {
    let config = refused_config("ssh-reentry-pty");
    let task = tokio::spawn(async move {
        let callbacks = RusshPtyCallbacks {
            on_chunk: Box::new(|_| {}),
            on_closed: Box::new(|_| {}),
        };
        russh_open_pty(&config, None, 80, 24, None, callbacks).map(|_| ())
    });
    let result = task.await.expect("PTY 入口在 tokio 工作线程上不得 panic");
    let error = result.expect_err("端口 1 不应该连上任何 SSH 服务");
    assert!(
        !error.message.is_empty(),
        "PTY 连接失败必须给出可读的错误信息"
    );
}
