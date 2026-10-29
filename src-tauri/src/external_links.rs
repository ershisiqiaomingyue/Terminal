//! 外部链接统一出口。
//!
//! 主窗口是 `decorations(false)` 的无边框窗口，WebView 一旦导航到外部页面就
//! 没有标题栏和返回按钮，用户无法退回应用（曾经因此卡死）。所以：
//! - WebView 内部只允许导航到应用自身页面与本地开发服务器；
//! - 其它地址一律交给系统默认浏览器，`window.open` 也直接拒绝新建窗口。

use tauri::Url;

/// 应用自身页面、开发服务器以及 WebView 内部使用的地址。
pub fn is_internal_url(url: &Url) -> bool {
    match url.scheme() {
        // `tauri` 是打包后的自定义协议，`asset`/`ipc` 供 WebView 内部资源与 IPC 使用。
        "tauri" | "asset" | "ipc" | "blob" | "data" | "about" => true,
        // 开发模式与浏览器预览都跑在本地 http 服务上。
        "http" | "https" => matches!(
            url.host_str(),
            Some("tauri.localhost") | Some("localhost") | Some("127.0.0.1") | Some("[::1]")
        ),
        _ => false,
    }
}

/// 可以交给系统默认程序处理的协议，与 opener 插件默认权限保持一致。
pub fn is_openable_external_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto" | "tel")
}

/// 用系统默认浏览器打开外部链接。失败只记录日志，不阻断界面。
pub fn open_in_browser(app: &tauri::AppHandle, url: &Url) {
    if !is_openable_external_url(url) {
        eprintln!("已拦截无法在系统浏览器中打开的地址: {url}");
        return;
    }
    use tauri_plugin_opener::OpenerExt;
    if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
        eprintln!("无法用系统浏览器打开 {url}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        value.parse().expect("测试地址应当可解析")
    }

    #[test]
    fn allows_application_pages() {
        assert!(is_internal_url(&url("tauri://localhost/index.html")));
        assert!(is_internal_url(&url("http://127.0.0.1:1420/")));
        assert!(is_internal_url(&url("http://localhost:1420/?boot=1")));
    }

    #[test]
    fn blocks_external_sites() {
        assert!(!is_internal_url(&url("https://github.com/ershisiqiaomingyue/agent2api")));
        assert!(!is_internal_url(&url("http://example.com/")));
        // 主机名形似的站点同样不能放行。
        assert!(!is_internal_url(&url("https://127.0.0.1.evil.com/")));
        assert!(!is_internal_url(&url("https://localhost.evil.com/")));
    }

    #[test]
    fn only_system_handled_schemes_reach_the_browser() {
        assert!(is_openable_external_url(&url("https://example.com")));
        assert!(is_openable_external_url(&url("mailto:dev@example.com")));
        assert!(!is_openable_external_url(&url("javascript:alert(1)")));
        assert!(!is_openable_external_url(&url("file:///C:/Windows/System32/calc.exe")));
    }
}