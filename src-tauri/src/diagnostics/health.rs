use super::record;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, WebviewWindow};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    pub visible: bool,
    pub view: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub running: Option<bool>,
    pub dom_nodes: Option<u64>,
    pub heap_bytes: Option<u64>,
    pub blank: Option<bool>,
}
struct WindowHealth {
    last_seen: Instant,
    last_recorded: Instant,
    stalled: bool,
    state: Heartbeat,
}
fn should_report_stall(
    visible: bool,
    minimized: bool,
    window_visible: bool,
    already_reported: bool,
    gap: Duration,
) -> bool {
    visible && !minimized && window_visible && !already_reported && gap > Duration::from_secs(45)
}
static WINDOWS: OnceLock<Mutex<HashMap<String, WindowHealth>>> = OnceLock::new();
fn windows() -> &'static Mutex<HashMap<String, WindowHealth>> {
    WINDOWS.get_or_init(Mutex::default)
}

#[tauri::command]
pub async fn report_frontend_heartbeat(window: WebviewWindow, state: Heartbeat) {
    let label = window.label().to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || update_heartbeat(&label, state)).await;
}

fn update_heartbeat(label: &str, mut state: Heartbeat) {
    for field in [&mut state.view, &mut state.thread_id, &mut state.turn_id] {
        if let Some(value) = field {
            *value = value.chars().take(160).collect();
        }
    }
    let now = Instant::now();
    let Ok(mut entries) = windows().lock() else {
        return;
    };
    let entry = entries
        .entry(label.into())
        .or_insert_with(|| WindowHealth {
            last_seen: now,
            last_recorded: now - Duration::from_secs(30),
            stalled: false,
            state: state.clone(),
        });
    let resumed_gap = entry.stalled.then(|| now.duration_since(entry.last_seen).as_millis());
    let save_health = now.duration_since(entry.last_recorded) >= Duration::from_secs(30)
        || entry.state.visible != state.visible
        || state.blank == Some(true);
    if save_health {
        entry.last_recorded = now;
    }
    entry.last_seen = now;
    entry.stalled = false;
    entry.state = state.clone();
    drop(entries);
    if let Some(gap) = resumed_gap {
        record("info", "frontend.heartbeat_resumed", label, json!({"gapMs":gap,"state":state}));
    }
    if save_health {
        record(
            if state.blank == Some(true) {
                "warn"
            } else {
                "info"
            },
            "frontend.health",
            label,
            json!({"state":state,"nativeMemoryBytes":native_memory()}),
        );
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendEvent {
    pub level: String,
    pub event: String,
    pub timestamp: String,
    pub data: Value,
}

#[tauri::command]
pub async fn report_diagnostic_events(
    window: WebviewWindow,
    events: Vec<FrontendEvent>,
) -> Result<(), String> {
    let label = window.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        for event in events.into_iter().take(50) {
            let level = match event.level.as_str() { "error" => "error", "warn" => "warn", _ => "info" };
            super::try_record(level, &format!("frontend.{}", event.event.chars().take(100).collect::<String>()), &label, json!({"capturedAt":event.timestamp.chars().take(40).collect::<String>(),"detail":event.data}))?;
        }
        Ok(())
    }).await.map_err(|error| error.to_string())?
}

pub fn register_window(window: &WebviewWindow) {
    let now = Instant::now();
    if let Ok(mut entries) = windows().lock() {
        entries
            .entry(window.label().into())
            .or_insert_with(|| WindowHealth {
                last_seen: now,
                last_recorded: now,
                stalled: false,
                state: Heartbeat {
                    visible: true,
                    view: None,
                    thread_id: None,
                    turn_id: None,
                    running: None,
                    dom_nodes: None,
                    heap_bytes: None,
                    blank: None,
                },
            });
    }
    record("info", "window.created", window.label(), json!({}));
    let label = window.label().to_string();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Focused(focused) => {
            record("info", "window.focus", &label, json!({"focused":focused}));
            if *focused {
                if let Ok(mut entries) = windows().lock() {
                    if let Some(entry) = entries.get_mut(&label) {
                        entry.state.visible = true;
                        entry.last_seen = Instant::now();
                    }
                }
            }
        }
        tauri::WindowEvent::Destroyed => {
            record("info", "window.destroyed", &label, json!({}));
            if let Ok(mut entries) = windows().lock() {
                entries.remove(&label);
            }
        }
        _ => {}
    });
}

pub fn start_watchdog(app: AppHandle) {
    std::thread::spawn(move || {
        let mut previous = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(10));
            if super::EXITED.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let now = Instant::now();
            let resumed = now.duration_since(previous) > Duration::from_secs(25);
            previous = now;
            for (label, gap, state) in check_windows(windows(), now, resumed, |label| {
                let window = app.get_webview_window(label)?;
                Some((window.is_minimized().unwrap_or(false), window.is_visible().unwrap_or(true)))
            }) {
                record("warn", "frontend.heartbeat_missing", &label, json!({"gapMs":gap,"lastState":state,"nativeMemoryBytes":native_memory()}));
            }
        }
    });
}

fn check_windows(
    registry: &Mutex<HashMap<String, WindowHealth>>,
    now: Instant,
    resumed: bool,
    mut probe: impl FnMut(&str) -> Option<(bool, bool)>,
) -> Vec<(String, u128, Heartbeat)> {
    let labels: Vec<_> = match registry.lock() {
        Ok(entries) => entries.keys().cloned().collect(),
        Err(_) => return Vec::new(),
    };
    let mut stalled = Vec::new();
    for label in labels {
        // Window queries synchronously dispatch to the UI thread. That thread
        // handles focus/heartbeat events using this registry: never hold its
        // lock across a query, or the two threads can wait on each other forever.
        let status = probe(&label);
        let Ok(mut entries) = registry.lock() else { continue };
        let Some((minimized, visible)) = status else {
            entries.remove(&label);
            continue;
        };
        let Some(entry) = entries.get_mut(&label) else { continue };
        if resumed {
            entry.last_seen = now;
            continue;
        }
        // A heartbeat/focus event may have arrived while the query was pending.
        let gap = now.saturating_duration_since(entry.last_seen);
        if should_report_stall(entry.state.visible, minimized, visible, entry.stalled, gap) {
            entry.stalled = true;
            stalled.push((label, gap.as_millis(), entry.state.clone()));
        }
    }
    stalled
}

#[cfg(windows)]
pub fn native_memory() -> Option<usize> {
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size).ok()?;
    }
    Some(counters.WorkingSetSize)
}
#[cfg(not(windows))]
pub fn native_memory() -> Option<usize> {
    None
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0;
        let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == 259;
        let _ = CloseHandle(handle);
        alive
    }
}
#[cfg(not(windows))]
pub fn process_alive(pid: u32) -> bool {
    pid == std::process::id() || std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registry(now: Instant) -> Mutex<HashMap<String, WindowHealth>> {
        Mutex::new(HashMap::from([("main".into(), WindowHealth {
            last_seen: now - Duration::from_secs(50),
            last_recorded: now,
            stalled: false,
            state: Heartbeat { visible: true, view: None, thread_id: None, turn_id: None, running: None, dom_nodes: None, heap_bytes: None, blank: None },
        })]))
    }

    #[test]
    fn window_query_can_process_a_focus_event_without_deadlocking() {
        let now = Instant::now();
        let entries = registry(now);
        let stalled = check_windows(&entries, now, false, |label| {
            // Simulate a UI-thread focus event while the watchdog waits for its
            // window query. The registry must be available to that callback.
            let mut state = entries.try_lock().expect("watchdog must release the registry before dispatching to the UI thread");
            state.get_mut(label).unwrap().last_seen = now + Duration::from_millis(1);
            Some((false, true))
        });
        assert!(stalled.is_empty(), "a fresh focus/heartbeat must prevent a stale hang report");
        assert!(!entries.lock().unwrap()["main"].stalled);
    }

    #[test]
    fn watchdog_records_stall_once_and_cleans_up_closed_windows() {
        let now = Instant::now();
        let entries = registry(now);
        assert_eq!(check_windows(&entries, now, false, |_| Some((false, true))).len(), 1);
        assert!(check_windows(&entries, now, false, |_| Some((false, true))).is_empty());
        check_windows(&entries, now, false, |_| None);
        assert!(entries.lock().unwrap().is_empty());
        let entries = registry(now);
        assert!(check_windows(&entries, now, true, |_| Some((false, true))).is_empty());
        assert_eq!(entries.lock().unwrap()["main"].last_seen, now);
    }

    #[test]
    fn watchdog_ignores_hidden_minimized_or_previously_reported_windows() {
        let gap = Duration::from_secs(50);
        assert!(should_report_stall(true, false, true, false, gap));
        assert!(!should_report_stall(false, false, true, false, gap));
        assert!(!should_report_stall(true, true, true, false, gap));
        assert!(!should_report_stall(true, false, false, false, gap));
        assert!(!should_report_stall(true, false, true, true, gap));
        assert!(!should_report_stall(
            true,
            false,
            true,
            false,
            Duration::from_secs(30)
        ));
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
    }
}
