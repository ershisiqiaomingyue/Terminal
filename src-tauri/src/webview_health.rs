use tauri::{AppHandle, Manager};

fn record(_: &AppHandle, label: &str, detail: &str) {
    crate::diagnostics::record(
        if detail.contains("ProcessFailed") || detail.contains("failed") {
            "error"
        } else {
            "info"
        },
        "webview.health",
        label,
        serde_json::json!({"message":detail}),
    );
}

#[tauri::command]
pub fn report_frontend_error(window: tauri::WebviewWindow, category: String, message: String) {
    record(
        window.app_handle(),
        window.label(),
        &format!("frontend {category}: {message}"),
    );
}

#[derive(Default)]
struct RecoveryBudget {
    attempts: std::collections::VecDeque<std::time::Instant>,
}

impl RecoveryBudget {
    fn allow(&mut self, now: std::time::Instant) -> bool {
        self.attempts
            .retain(|attempt| now.duration_since(*attempt).as_secs() < 60);
        if self.attempts.len() >= 3 {
            return false;
        }
        self.attempts.push_back(now);
        true
    }
}

#[cfg(windows)]
pub fn install(window: &tauri::WebviewWindow) {
    crate::diagnostics::health::register_window(window);
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    use webview2_com::{Microsoft::Web::WebView2::Win32::*, ProcessFailedEventHandler};
    use windows_core::Interface;

    let app = window.app_handle().clone();
    let label = window.label().to_string();
    let callback_app = app.clone();
    let callback_label = label.clone();
    let result = window.with_webview(move |platform| {
        let app = callback_app;
        let label = callback_label;
        let setup = || -> windows_core::Result<()> {
            let view = unsafe { platform.controller().CoreWebView2()? };
            let mut version = windows_core::PWSTR::null();
            let mut browser_pid = 0;
            let runtime_version = unsafe {
                let value = if platform.environment().BrowserVersionString(&mut version).is_ok() { version.to_string().ok() } else { None };
                if !version.is_null() { windows::Win32::System::Com::CoTaskMemFree(Some(version.0.cast())); }
                let _ = view.BrowserProcessId(&mut browser_pid);
                value
            };
            crate::diagnostics::record("info", "webview.runtime", &label, serde_json::json!({"version":runtime_version,"browserPid":browser_pid}));
            let mut token = 0;
            let mut budget = RecoveryBudget::default();
            let dialog_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let event_app = app.clone();
            let event_label = label.clone();
            let handler = ProcessFailedEventHandler::create(Box::new(move |view, args| {
                let app = &event_app;
                let label = &event_label;
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(-1);
                unsafe { args.ProcessFailedKind(&mut kind)?; }
                let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON(-1);
                let mut exit_code = 0;
                if let Ok(details) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
                    unsafe { let _ = details.Reason(&mut reason); let _ = details.ExitCode(&mut exit_code); }
                }
                crate::diagnostics::record("error", "webview.process_failed", label, serde_json::json!({"kind":kind.0,"reason":reason.0,"exitCode":exit_code}));
                if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
                    if budget.allow(std::time::Instant::now()) {
                        if let Some(view) = view {
                            let result = unsafe { view.Reload() };
                            record(&app, &label, &format!("renderer reload: {result:?}"));
                        }
                    } else {
                        app.dialog().message("页面连续发生异常，已停止自动重载。请重新打开应用；诊断记录已保存。")
                            .title("页面异常").kind(MessageDialogKind::Error).show(|_| {});
                    }
                } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE {
                    // Do not silently discard a draft for a slow render. The dialog is native,
                    // so it remains usable even when the page's JavaScript thread is stuck.
                    let retry_app = app.clone();
                    let retry_label = label.clone();
                    if dialog_open.swap(true, std::sync::atomic::Ordering::SeqCst) { return Ok(()); }
                    let dismiss_dialog = dialog_open.clone();
                    app.dialog().message("页面长时间未响应。重新加载界面后会重新连接后台任务。是否重新加载？")
                        .title("页面未响应").buttons(MessageDialogButtons::OkCancelCustom("重新加载".into(), "继续等待".into()))
                        .show(move |reload| {
                            dismiss_dialog.store(false, std::sync::atomic::Ordering::SeqCst);
                            if reload {
                                if let Some(window) = retry_app.get_webview_window(&retry_label) {
                                    let result = window.reload();
                                    record(&retry_app, &retry_label, &format!("unresponsive reload: {result:?}"));
                                }
                            }
                        });
                } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
                    app.dialog().message("WebView 浏览器进程已退出，界面无法继续显示。请重新打开应用；诊断记录已保存。")
                        .title("界面进程异常").kind(MessageDialogKind::Error).show(|_| {});
                }
                // WebView2 restarts a failed GPU/utility process itself. Record it,
                // without unnecessarily reloading a healthy renderer or cancelling work.
                Ok(())
            }));
            unsafe { view.add_ProcessFailed(&handler, &mut token)?; }
            crate::diagnostics::record("info", "webview.health_installed", &label, serde_json::json!({}));
            Ok(())
        };
        if let Err(error) = setup() { record(&app, &label, &format!("ProcessFailed hook failed: {error}")); }
    });
    if let Err(error) = result {
        record(&app, &label, &format!("health setup failed: {error}"));
    }
}

#[cfg(not(windows))]
pub fn install(window: &tauri::WebviewWindow) {
    crate::diagnostics::health::register_window(window);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_cannot_loop_and_resets_after_quiet_period() {
        let mut budget = RecoveryBudget::default();
        let now = std::time::Instant::now();
        for _ in 0..3 {
            assert!(budget.allow(now));
        }
        assert!(!budget.allow(now));
        assert!(budget.allow(now + std::time::Duration::from_secs(60)));
    }
}
