//! Language-server operations, server catalog and matching.

use super::limits::MAX_LSP_CONFIG_BYTES;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspOperation {
    Diagnostics,
    Symbols,
    Definition,
    References,
    Hover,
    Completion,
    Rename,
    Format,
    DocumentSymbols,
    WorkspaceSymbols,
}

impl LspOperation {
    pub(super) fn method(self) -> &'static str {
        match self {
            Self::Diagnostics => "textDocument/diagnostic",
            Self::Symbols | Self::DocumentSymbols => "textDocument/documentSymbol",
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Hover => "textDocument/hover",
            Self::Completion => "textDocument/completion",
            Self::Rename => "textDocument/rename",
            Self::Format => "textDocument/formatting",
            Self::WorkspaceSymbols => "workspace/symbol",
        }
    }
}

pub(super) fn check_cancelled(cancellation: &AtomicBool) -> Result<(), String> {
    if cancellation.load(Ordering::Relaxed) {
        Err("请求已取消".to_string())
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ServerConfig {
    pub(super) id: String,
    pub(super) command: String,
    #[serde(default)]
    pub(super) args: Vec<String>,
    #[serde(default)]
    pub(super) languages: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) filenames: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) env: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) disabled: bool,
    #[serde(default)]
    pub(super) initialization_options: Value,
    #[serde(default)]
    pub(super) settings: Value,
}
#[derive(Debug, Deserialize)]
pub(super) struct LspConfig {
    pub(super) servers: Vec<ServerConfig>,
}
pub(super) fn configured_server(
    config_path: &Path,
    path: &Path,
) -> Result<(ServerConfig, String), String> {
    let mut bytes = Vec::new();
    use std::io::Read;
    std::fs::File::open(config_path)
        .and_then(|file| {
            file.take(MAX_LSP_CONFIG_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| {
            format!(
                "无法读取 LSP 配置 {}：{e}。可用源码检索继续定位。",
                config_path.display()
            )
        })?;
    if bytes.len() > MAX_LSP_CONFIG_BYTES {
        return Err("LSP 配置过大".into());
    }
    let config: LspConfig =
        serde_json::from_slice(&bytes).map_err(|e| format!("LSP 配置无效：{e}"))?;
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    config
        .servers
        .into_iter()
        .find_map(|server| {
            if server.disabled {
                return None;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let language = server
                .filenames
                .get(name)
                .or_else(|| server.languages.get(&ext))?
                .clone();
            Some((server, language))
        })
        .ok_or_else(|| {
            format!(
                "LSP 未配置 .{ext} 的语言服务器；请编辑 {}，或使用源码检索。",
                config_path.display()
            )
        })
}
