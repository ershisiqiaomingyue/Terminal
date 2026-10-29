//! 会话完成后的桌面提示窗。
//!
//! 走的是和工具窗口同一套「自绘子 WebView」路子（见 `crate::tool_windows`），
//! 而不是系统 toast：`windows` crate 没开 `UI_Notifications` feature，加
//! `tauri-plugin-notification` 又要联网拉新包，两者都会把「能不能弹出来」
//! 变成构建期风险。自绘窗口零新依赖，且「首行 xxx 完成 + 两行摘要 + 点击
//! 打开对话」这种排版必须自己控制才做得准。
//!
//! 触发点在 `agent::queue` 的终态收尾处（`completed` 才弹），生命周期很短：
//! 建窗 → 前端渲染好后上报 show（这时才定位到右下角并显示，避免白屏闪一下）
//! → 用户点击或超时后销毁。

use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 未及上报 show 的窗口存活上限：前端加载失败（比如 dev server 没起来）时
/// 不能把一块空白永远钉在屏幕角落。
const ORPHAN_TIMEOUT: Duration = Duration::from_secs(15);
/// 提示窗尺寸 == 卡片尺寸（前端 `#root` 不再留投影余量，卡片铺满整窗）：
/// 372 宽；高 84 = 2 边框 + 24 内距 + 18 标题 + 6 间隔 + 34 两行摘要。
const NOTICE_WIDTH: f64 = 372.0;
const NOTICE_HEIGHT: f64 = 84.0;
/// 距工作区（已排除任务栏）右下角的留白。
const MARGIN: i32 = 16;
/// 摘要最多这么多字符，前端再按两行裁一次。
const SUMMARY_LIMIT: usize = 120;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionNoticeInfo {
    label: String,
    session_id: String,
    /// 会话标题（取不到时前端显示「新会话」）。
    title: String,
    /// 回复正文压成一行，不含换行。
    summary: String,
}

struct Entry {
    info: CompletionNoticeInfo,
    shown: bool,
}

#[derive(Default)]
pub struct CompletionNoticeState {
    entries: Mutex<HashMap<String, Entry>>,
}

/// 把回复正文压成一行摘要：去掉行首的 Markdown 记号、折叠空白、截断。
fn summarize(reply: &str) -> String {
    let flattened = reply
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '-', '*', '+', ' '])
                .trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let collapsed = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SUMMARY_LIMIT {
        return collapsed;
    }
    let mut clipped = collapsed.chars().take(SUMMARY_LIMIT).collect::<String>();
    clipped.push('…');
    clipped
}

fn session_title(app: &AppHandle, session_id: &str) -> String {
    crate::history::get_history_session(app.clone(), session_id.to_string())
        .ok()
        .flatten()
        .map(|record| record.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "新会话".to_string())
}

/// 把提示窗摆到当前主窗口所在显示器（拿不到就主显示器）工作区的右下角。
fn place_bottom_right(app: &AppHandle, window: &WebviewWindow) {
    let anchor = app
        .get_webview_window("main")
        .and_then(|main| main.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = anchor else {
        return;
    };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let width = (NOTICE_WIDTH * scale).round() as i32;
    let height = (NOTICE_HEIGHT * scale).round() as i32;
    let margin = (MARGIN as f64 * scale).round() as i32;
    let right = area.position.x.saturating_add(area.size.width as i32);
    let bottom = area.position.y.saturating_add(area.size.height as i32);
    let x = (right - width - margin).max(area.position.x);
    let y = (bottom - height - margin).max(area.position.y);
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 弹出「会话完成」提示。`completed` 之外的状态不弹。
pub(crate) fn notify_completion(app: &AppHandle, session_id: &str, reply: Option<&str>) {
    if session_id.trim().is_empty() {
        return;
    }
    let app = app.clone();
    let session_id = session_id.to_string();
    let summary = summarize(reply.unwrap_or_default());
    // 弹窗要建 WebView，不能拖住 agent 回合的收尾线程。
    tauri::async_runtime::spawn(async move {
        let title = session_title(&app, &session_id);
        let label = format!(
            "tool-notice-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let info = CompletionNoticeInfo {
            label: label.clone(),
            session_id,
            title,
            summary,
        };
        // label 刻意用 `tool-` 前缀：capabilities/default.json 的 windows 列表
        // 只覆盖 `main` / `boot` / `tool-*`，换个前缀就得再维护一份 capability。
        if let Ok(mut entries) = app.state::<CompletionNoticeState>().entries.lock() {
            entries.insert(label.clone(), Entry { info, shown: false });
        }
        let created = WebviewWindowBuilder::new(
            &app,
            &label,
            WebviewUrl::App(format!("/?notice=1&label={label}").into()),
        )
        .title("会话完成")
        .inner_size(NOTICE_WIDTH, NOTICE_HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        // 透明分两层：窗口层 + WebView 层。只写 `transparent(true)` 的话窗口是透明的，
        // 但 WebView2 自己那层默认底色仍是白的，于是卡片周围 14px 投影留白会显示成
        // 一圈白边（看起来像「窗口没占满」）。Windows 上 alpha 必须正好是 0 才生效。
        .transparent(true)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .on_navigation(crate::external_links::is_internal_url)
        .build();
        let window = match created {
            Ok(window) => window,
            Err(error) => {
                if let Ok(mut entries) = app.state::<CompletionNoticeState>().entries.lock() {
                    entries.remove(&label);
                }
                eprintln!("无法创建会话完成提示窗: {error}");
                return;
            }
        };
        // 前端渲染完会回调 show_completion_notice；等不到就说明页面没起来。
        let close_app = app.clone();
        let close_label = label.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Ok(mut entries) = close_app.state::<CompletionNoticeState>().entries.lock() {
                    entries.remove(&close_label);
                }
            }
        });
        let watchdog_app = app.clone();
        let watchdog_label = label.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(ORPHAN_TIMEOUT).await;
            let shown = watchdog_app
                .state::<CompletionNoticeState>()
                .entries
                .lock()
                .ok()
                .and_then(|entries| entries.get(&watchdog_label).map(|entry| entry.shown));
            if shown == Some(false) {
                if let Some(window) = watchdog_app.get_webview_window(&watchdog_label) {
                    let _ = window.destroy();
                }
            }
        });
        // 提示音交给主窗口播：这个提示窗是刚新建的、没有任何用户手势，
        // WebView2 的自动播放策略会把它的 AudioContext 直接挂起，声音根本出不来。
        // 主窗口点过「发送」，手势早就有了，所以那边一定响得了。
        let _ = app.emit_to("main", "completion-notice:chime", ());
    });
}

#[tauri::command]
pub fn get_completion_notice(
    window: WebviewWindow,
    notices: State<'_, CompletionNoticeState>,
) -> Result<CompletionNoticeInfo, String> {
    notices
        .entries
        .lock()
        .map_err(|_| "提示窗状态不可用")?
        .get(window.label())
        .map(|entry| entry.info.clone())
        .ok_or_else(|| "会话完成提示不存在".into())
}

/// 前端首帧渲染完成后调用：把窗口摆到右下角再显示，避免白底闪一下。
#[tauri::command]
pub fn show_completion_notice(
    app: AppHandle,
    window: WebviewWindow,
    notices: State<'_, CompletionNoticeState>,
) -> Result<(), String> {
    if let Ok(mut entries) = notices.entries.lock() {
        if let Some(entry) = entries.get_mut(window.label()) {
            entry.shown = true;
        }
    }
    place_bottom_right(&app, &window);
    window.show().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dismiss_completion_notice(window: WebviewWindow) -> Result<(), String> {
    window.destroy().map_err(|error| error.to_string())
}

/// 点击提示：先拉起主窗口并切到会话，再销毁自己。
#[tauri::command]
pub fn open_completion_notice(
    app: AppHandle,
    window: WebviewWindow,
    notices: State<'_, CompletionNoticeState>,
) -> Result<(), String> {
    let session_id = notices.entries.lock().ok().and_then(|entries| {
        entries
            .get(window.label())
            .map(|entry| entry.info.session_id.clone())
    });
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
    if let Some(session_id) = session_id {
        // 主窗口收到后自己切到对话页并打开该会话（见 src/App.tsx）。
        let _ = app.emit_to(
            "main",
            "completion-notice:open",
            serde_json::json!({ "sessionId": session_id }),
        );
    }
    window.destroy().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn summary_is_a_single_clipped_line() {
        let reply = "## 结论\n\n- 第一点\n- 第二点\n\n  细节  在这里  ";
        assert_eq!(super::summarize(reply), "结论 第一点 第二点 细节 在这里");
        let long = "字".repeat(super::SUMMARY_LIMIT + 20);
        let clipped = super::summarize(&long);
        assert_eq!(clipped.chars().count(), super::SUMMARY_LIMIT + 1);
        assert!(clipped.ends_with('…'));
        assert_eq!(super::summarize("   "), "");
    }
}
