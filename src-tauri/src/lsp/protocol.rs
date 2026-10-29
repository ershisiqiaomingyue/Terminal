//! JSON-RPC framing, paths and cancellation.

use super::limits::MAX_LSP_MESSAGE_BYTES;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{sleep, Duration};
use url::Url;

pub fn workspace_root(path: &Path, roots: &[String]) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .filter(|root| root.is_dir() && path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .ok_or_else(|| "LSP 文件必须位于用户明确选择的工作区内".into())
}

pub(super) fn file_uri(path: &Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("无法定位 LSP 文件 {}：{error}", path.display()))?;
    Url::from_file_path(canonical)
        .map(|value| value.to_string())
        .map_err(|_| format!("无法转换 LSP 文件路径：{}", path.display()))
}

pub(super) async fn wait_for_cancellation(cancellation: Arc<AtomicBool>) {
    while !cancellation.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(30)).await;
    }
}

pub(super) async fn write_json<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| format!("LSP 请求序列化失败：{error}"))?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(|error| format!("LSP 请求头写入失败：{error}"))?;
    writer
        .write_all(&body)
        .await
        .map_err(|error| format!("LSP 请求体写入失败：{error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("LSP 请求刷新失败：{error}"))
}

pub(super) async fn read_json<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value, String> {
    let mut headers = Vec::new();
    loop {
        let byte = reader
            .read_u8()
            .await
            .map_err(|error| format!("LSP 响应头读取失败：{error}"))?;
        headers.push(byte);
        if headers.len() > 64 * 1024 {
            return Err("LSP 响应头过大".to_string());
        }
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_text = String::from_utf8_lossy(&headers);
    let length = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or_else(|| "LSP 响应缺少 Content-Length".to_string())?;
    if length == 0 || length > MAX_LSP_MESSAGE_BYTES {
        return Err("LSP 响应体过大或为空".to_string());
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| format!("LSP 响应体读取失败：{error}"))?;
    serde_json::from_slice(&body).map_err(|error| format!("LSP 响应 JSON 无效：{error}"))
}

pub(super) fn client_response(value: &Value, settings: &Value, root_uri: &str) -> Option<Value> {
    let method = value.get("method")?.as_str()?;
    let id = value.get("id")?;
    Some(match method {
        "workspace/configuration" => {
            let items = value["params"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            json!({"jsonrpc":"2.0","id":id,"result":items.iter().map(|item| {
                item["section"].as_str().filter(|s| !s.is_empty()).map(|section| {
                    section.split('.').fold(settings, |value, key| &value[key]).clone()
                }).unwrap_or_else(|| settings.clone())
            }).collect::<Vec<_>>()})
        }
        "workspace/workspaceFolders" => {
            json!({"jsonrpc":"2.0","id":id,"result":[{"uri":root_uri,"name":"workspace"}]})
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => json!({"jsonrpc":"2.0","id":id,"result":null}),
        // In particular, never acknowledge workspace/applyEdit: it would bypass
        // the host's write tools and permission boundary.
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client method not supported"}})
        }
    })
}
