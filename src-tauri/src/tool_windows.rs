use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const TOOLS: &[&str] = &[
    "code-viewer",
    "adb",
    "apps",
    "shell",
    "logcat",
    "archive",
    "aosp",
    "files",
    "capture",
    "control",
    "simpleperf",
    "method-profiler",
    "heap-dump",
    "benchmark",
    "native-symbolizer",
    "scrcpy",
    "perf",
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolWindowInfo {
    label: String,
    tool_key: String,
    title: String,
    state: Value,
}
struct Entry {
    info: ToolWindowInfo,
    source: String,
    ready: bool,
}
#[derive(Default)]
pub struct ToolWindowState {
    entries: Mutex<HashMap<String, Entry>>,
}
#[derive(Deserialize)]
pub struct Position {
    x: f64,
    y: f64,
}

fn validate(tool_key: &str, state: &Value) -> Result<(), String> {
    if !TOOLS.contains(&tool_key) {
        return Err("只能将主页工具拖出为独立窗口".into());
    }
    if !state.is_object() || state.to_string().len() > 4 * 1024 * 1024 {
        return Err("工具窗口状态无效或超过 4 MiB".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn create_tool_window(
    app: AppHandle,
    window: WebviewWindow,
    windows: State<'_, ToolWindowState>,
    tool_key: String,
    title: String,
    state: Value,
    position: Option<Position>,
) -> Result<String, String> {
    validate(&tool_key, &state)?;
    // Each instance gets a unique label, including two viewers of the same device.
    let label = format!(
        "tool-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let title: String = title.chars().take(100).collect();
    let info = ToolWindowInfo {
        label: label.clone(),
        tool_key,
        title: title.clone(),
        state,
    };
    windows
        .entries
        .lock()
        .map_err(|_| "窗口状态不可用")?
        .insert(
            label.clone(),
            Entry {
                info,
                source: window.label().into(),
                ready: false,
            },
        );
    let created = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("/?toolWindow={label}").into()),
    )
    .title(format!("{title} · Terminal"))
    .inner_size(1200.0, 820.0)
    .min_inner_size(900.0, 620.0)
    .decorations(false)
    .visible(false)
    .center()
    .on_navigation(crate::external_links::is_internal_url)
    .build();
    let child = match created {
        Ok(child) => child,
        Err(error) => {
            windows
                .entries
                .lock()
                .map_err(|_| "窗口状态不可用")?
                .remove(&label);
            return Err(format!("无法创建工具窗口：{error}"));
        }
    };
    crate::webview_health::install(&child);
    if let Some(point) = position.filter(|p| p.x.is_finite() && p.y.is_finite()) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let mut x = point.x - 120.0 * scale;
        let mut y = point.y - 18.0 * scale;
        if let Ok(monitors) = window.available_monitors() {
            if let Some(monitor) = monitors.iter().find(|m| {
                let p = m.position();
                let s = m.size();
                point.x >= p.x as f64
                    && point.x < p.x as f64 + s.width as f64
                    && point.y >= p.y as f64
                    && point.y < p.y as f64 + s.height as f64
            }) {
                let p = monitor.position();
                let s = monitor.size();
                x = x.clamp(
                    p.x as f64,
                    (p.x as f64 + s.width as f64 - 300.0 * scale).max(p.x as f64),
                );
                y = y.clamp(
                    p.y as f64,
                    (p.y as f64 + s.height as f64 - 100.0 * scale).max(p.y as f64),
                );
            }
        }
        let _ = child.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
    let close_app = app.clone();
    let close_label = label.clone();
    child.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Ok(mut entries) = close_app.state::<ToolWindowState>().entries.lock() {
                entries.remove(&close_label);
            }
            crate::android_tools::stop_window_logcat(&close_app, &close_label);
            crate::terminal::close_window_terminals(&close_app, &close_label);
        }
    });
    Ok(label)
}

#[tauri::command]
pub fn get_tool_window(
    window: WebviewWindow,
    windows: State<'_, ToolWindowState>,
) -> Result<ToolWindowInfo, String> {
    windows
        .entries
        .lock()
        .map_err(|_| "窗口状态不可用")?
        .get(window.label())
        .map(|entry| entry.info.clone())
        .ok_or_else(|| "独立工具窗口不存在".into())
}

#[tauri::command]
pub fn ready_tool_window(
    app: AppHandle,
    window: WebviewWindow,
    windows: State<'_, ToolWindowState>,
) -> Result<(), String> {
    let mut entries = windows.entries.lock().map_err(|_| "窗口状态不可用")?;
    let entry = entries
        .get_mut(window.label())
        .ok_or("独立工具窗口不存在")?;
    if entry.ready {
        return Ok(());
    }
    // Window operations can synchronously trigger lifecycle callbacks. Do them
    // without holding the registry lock used by the destroyed callback.
    let source = entry.source.clone();
    let snapshot = entry.info.state.clone();
    drop(entries);
    window.show().map_err(|e| e.to_string())?;
    let mut entries = windows.entries.lock().map_err(|_| "窗口状态不可用")?;
    let entry = entries
        .get_mut(window.label())
        .ok_or("独立工具窗口不存在")?;
    if entry.ready {
        return Ok(());
    }
    crate::android_tools::transfer_window_logcat(
        &app,
        &entry.source,
        window.label(),
        &entry.info.state,
    )?;
    if let Err(error) =
        crate::terminal::transfer_window_terminals(&app, &source, window.label(), &snapshot)
    {
        let _ =
            crate::android_tools::transfer_window_logcat(&app, window.label(), &source, &snapshot);
        let _ = crate::terminal::transfer_window_terminals(&app, window.label(), &source, &snapshot);
        return Err(error);
    }
    if let Err(error) = app.emit_to(
        &source,
        "tool-window:ready",
        serde_json::json!({"label":window.label()}),
    ) {
        let _ =
            crate::android_tools::transfer_window_logcat(&app, window.label(), &source, &snapshot);
        let _ =
            crate::terminal::transfer_window_terminals(&app, window.label(), &source, &snapshot);
        return Err(error.to_string());
    }
    entry.ready = true;
    drop(entries);
    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
pub fn abort_tool_window(
    app: AppHandle,
    window: WebviewWindow,
    windows: State<'_, ToolWindowState>,
    label: String,
) -> Result<bool, String> {
    let mut entries = windows.entries.lock().map_err(|_| "窗口状态不可用")?;
    let Some(entry) = entries
        .get(&label)
        .filter(|entry| entry.source == window.label())
    else {
        return Ok(false);
    };
    if entry.ready {
        return Ok(true);
    }
    entries.remove(&label);
    drop(entries);
    if let Some(child) = app.get_webview_window(&label) {
        child.destroy().map_err(|e| e.to_string())?;
    }
    Ok(false)
}

#[tauri::command]
pub fn focus_main_window(app: AppHandle, settings_section: Option<String>) -> Result<(), String> {
    let main = app.get_webview_window("main").ok_or("主窗口已关闭")?;
    if let Some(section) = settings_section {
        if [
            "general",
            "agent",
            "ssh",
            "wsl",
            "skills",
            "plugins",
            "project-automation",
            "about",
        ]
        .contains(&section.as_str())
        {
            app.emit_to(
                "main",
                "tool-window:open-settings",
                serde_json::json!({"section": section}),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    main.show().map_err(|e| e.to_string())?;
    main.unminimize().map_err(|e| e.to_string())?;
    main.set_focus().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn independent_windows_accept_tools_only_and_bound_transfer_state() {
        assert!(super::validate("logcat", &serde_json::json!({})).is_ok());
        for key in ["home", "chat", "../chat", "unknown"] {
            assert!(super::validate(key, &serde_json::json!({})).is_err());
        }
        assert!(super::validate("aosp", &serde_json::json!([])).is_err());
        assert!(super::validate(
            "logcat",
            &serde_json::json!({"data":"x".repeat(4*1024*1024)})
        )
        .is_err());
    }
}
