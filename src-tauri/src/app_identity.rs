//! 应用 identifier 的唯一出处。
//!
//! identifier 决定用户能直接看到的目录名：`%APPDATA%\com.terminal.desktop`、
//! `%LOCALAPPDATA%\com.terminal.desktop\logs`。设置页的「打开日志目录」、
//! Agent 的数据根目录都从这里取运行时值，避免再散落硬编码字符串。
//!
//! 真正的值来自 `tauri.conf.json` / `tauri.dev.conf.json`，启动时由 [`set`]
//! 注入（debug 版既可以跑正式配置也可以跑 dev 配置，所以不能用 `cfg!` 猜）。
//! 下面的测试把配置文件和常量钉在一起，改漏一处就会红。

use std::sync::OnceLock;

/// 正式版 identifier，必须与 `tauri.conf.json` 一致。
pub const IDENTIFIER: &str = "com.terminal.desktop";
/// 开发版 identifier，必须与 `tauri.dev.conf.json` 一致。
pub const DEV_IDENTIFIER: &str = "com.terminal.desktop.dev";

static RUNTIME: OnceLock<String> = OnceLock::new();

/// 启动时注入真实 identifier（重复调用只有第一次生效）。
pub fn set(identifier: &str) {
    let trimmed = identifier.trim();
    if !trimmed.is_empty() {
        let _ = RUNTIME.set(trimmed.to_string());
    }
}

/// 当前进程使用的 identifier：没注入过（单元测试、工具进程）时回落到正式版。
pub fn identifier() -> String {
    RUNTIME
        .get()
        .cloned()
        .unwrap_or_else(|| IDENTIFIER.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn config_identifier(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
        let text = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("读不到 {}：{error}", path.display());
        });
        text.lines()
            .find(|line| line.contains("\"identifier\""))
            .and_then(|line| line.split('"').nth(3))
            .unwrap_or_else(|| panic!("{name} 里没有 identifier"))
            .to_string()
    }

    #[test]
    fn config_identifiers_match_the_constants() {
        assert_eq!(config_identifier("tauri.conf.json"), IDENTIFIER);
        assert_eq!(config_identifier("tauri.dev.conf.json"), DEV_IDENTIFIER);
    }

    #[test]
    fn runtime_value_wins_and_falls_back_to_the_release_identifier() {
        // 没注入时回落到正式版，保证工具进程也能定位目录。
        assert_eq!(identifier(), RUNTIME.get().cloned().unwrap_or_else(|| IDENTIFIER.to_string()));
        set("");
        assert_eq!(identifier(), IDENTIFIER);
    }
}
