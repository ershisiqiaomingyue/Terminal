use super::super::*;
use crate::tasks::TaskManager;

use super::serialization::emit_agent_event_for_turn;

pub(crate) fn agent_task_ids() -> &'static Mutex<HashMap<(String, String), String>> {
    static IDS: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn agent_task_label(message: &str) -> (&'static str, String) {
    let normalized = message.to_lowercase();
    if message.contains("日志") || normalized.contains("log") || normalized.contains("logcat") {
        ("log-analysis", "正在分析日志".to_string())
    } else {
        (
            "agent",
            format!("正在处理：{}", message.chars().take(24).collect::<String>()),
        )
    }
}

pub(crate) fn begin_agent_task(app: &AppHandle, session_id: &str, turn_id: &str, message: &str) {
    let (kind, summary) = agent_task_label(message);
    let manager = app.state::<TaskManager>().inner().clone();
    let Ok((id, _)) = crate::tasks::create_task(app, &manager, kind, session_id, &summary)
    else {
        return;
    };
    // P5-A：键是 (session, turn) 而不是 session。guard 释放先于外层收尾，
    // 上一轮的 end_agent_task 还没跑到时下一轮就可能 begin——session 键会让
    // 旧 id 被覆盖，旧任务永远等不到自己的 end 而残留 running。
    if let Ok(mut ids) = agent_task_ids().lock() {
        ids.insert((session_id.to_string(), turn_id.to_string()), id);
    }
}

pub(crate) fn touch_agent_task(
    app: &AppHandle,
    session_id: &str,
    turn_id: Option<&str>,
    message: &str,
) {
    let Some(id) = agent_task_ids().lock().ok().and_then(|ids| match turn_id {
        Some(turn_id) => ids
            .get(&(session_id.to_string(), turn_id.to_string()))
            .cloned(),
        // 同一 session 同时最多一个活 turn（AgentRequestGuard 保证）；
        // 没有 turn_id 的旧调用方按会话内唯一活跃条目兜底。
        None => ids
            .iter()
            .find(|((session, _), _)| session == session_id)
            .map(|(_, id)| id.clone()),
    }) else {
        return;
    };
    let _ = crate::tasks::update_task(app, &id, 45, message);
}

pub(crate) fn end_agent_task(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    error: Option<&str>,
) {
    let Some(id) = agent_task_ids()
        .lock()
        .ok()
        .and_then(|mut ids| ids.remove(&(session_id.to_string(), turn_id.to_string())))
    else {
        return;
    };
    let manager = app.state::<TaskManager>().inner().clone();
    let _ = crate::tasks::finish_task(app, &manager, &id, None, error);
}

pub(crate) fn emit_agent_status_for_turn(
    app: &AppHandle,
    session_id: &str,
    status: &str,
    message: impl Into<String>,
    turn_id: Option<&str>,
) {
    let message = message.into();
    if let Some(turn_id) = turn_id {
        if let Err(error) = crate::history::update_agent_run_status(
            app,
            session_id,
            turn_id,
            status,
            Some(&message),
        ) {
            eprintln!("无法持久化 Agent 运行状态: {error}");
        }
    }
    touch_agent_task(app, session_id, turn_id, &message);
    emit_agent_event_for_turn(
        app,
        &AgentEvent::Status {
            session_id: session_id.to_string(),
            status: status.to_string(),
            message: Some(message),
        },
        turn_id,
    );
}
