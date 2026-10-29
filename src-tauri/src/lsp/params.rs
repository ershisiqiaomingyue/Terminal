//! Operation inputs, execution and bounded results.

use super::catalog::{check_cancelled, configured_server, LspOperation};
use super::executable::find_server_executable;
use super::limits::{
    LSP_TIMEOUT, MAX_COMPLETION_ITEMS, MAX_LSP_MESSAGE_BYTES, MAX_LSP_RESULT_CHARS,
    MAX_WORKSPACE_SYMBOLS,
};
use super::protocol::{file_uri, wait_for_cancellation};
use super::session::session_for;
use super::session::RequestFailure;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::time::timeout;

/// Extra per-operation inputs. `line`/`character` are zero-based with UTF-16
/// character offsets, exactly as the LSP specification defines `position`.
#[derive(Clone, Debug, Default)]
pub struct LspParams {
    pub line: u32,
    pub character: u32,
    pub query: Option<String>,
    pub new_name: Option<String>,
}

impl LspParams {
    #[cfg(test)]
    pub fn at(line: u32, character: u32) -> Self {
        Self {
            line,
            character,
            ..Self::default()
        }
    }
}

pub(super) fn operation_params(
    operation: LspOperation,
    uri: &str,
    params: &LspParams,
) -> Result<Value, String> {
    let mut value = match operation {
        LspOperation::Diagnostics | LspOperation::Symbols | LspOperation::DocumentSymbols => {
            json!({ "textDocument": { "uri": uri } })
        }
        LspOperation::Format => json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 4, "insertSpaces": true }
        }),
        LspOperation::WorkspaceSymbols => {
            json!({ "query": params.query.clone().unwrap_or_default() })
        }
        LspOperation::Definition
        | LspOperation::References
        | LspOperation::Hover
        | LspOperation::Completion
        | LspOperation::Rename => json!({
            "textDocument": { "uri": uri },
            "position": { "line": params.line, "character": params.character }
        }),
    };
    match operation {
        LspOperation::References => value["context"] = json!({ "includeDeclaration": true }),
        LspOperation::Rename => {
            let new_name = params.new_name.as_deref().unwrap_or_default();
            if new_name.trim().is_empty() {
                return Err("lsp_rename 需要非空的 newName".to_string());
            }
            value["newName"] = json!(new_name);
        }
        _ => {}
    }
    Ok(value)
}

/// Shape one operation's raw result for the model. Rename and format return the
/// edits as JSON exactly as the server produced them -- this module never applies
/// them, so no tool here can write a file.
pub(super) fn shape_result(operation: LspOperation, result: Value) -> Value {
    match operation {
        LspOperation::Completion => simplify_completion(&result),
        LspOperation::WorkspaceSymbols => cap_list(&result, MAX_WORKSPACE_SYMBOLS),
        _ => result,
    }
}

pub(super) fn simplify_completion(result: &Value) -> Value {
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| result.as_array())
        .cloned()
        .unwrap_or_default();
    let total = items.len();
    let items = items
        .iter()
        .take(MAX_COMPLETION_ITEMS)
        .map(|item| {
            json!({
                "label": item.get("label").cloned().unwrap_or(Value::Null),
                "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                "detail": item.get("detail").cloned().unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "items": items,
        "total": total,
        "truncated": total > MAX_COMPLETION_ITEMS
    })
}

pub(super) fn cap_list(result: &Value, limit: usize) -> Value {
    let items = result.as_array().cloned().unwrap_or_default();
    let total = items.len();
    let items = items.into_iter().take(limit).collect::<Vec<_>>();
    json!({
        "items": items,
        "total": total,
        "truncated": total > limit
    })
}

pub(super) async fn run_request(
    path: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    operation: LspOperation,
    params: LspParams,
    cancellation: Arc<AtomicBool>,
) -> Result<Value, String> {
    check_cancelled(&cancellation)?;
    let (server, language) = configured_server(&config_path, &path)?;
    let resolved = find_server_executable(&server.command, &path, &workspace)?;
    let uri = file_uri(&path)?;
    let root_uri = file_uri(&workspace)?;
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_LSP_MESSAGE_BYTES as u64 {
        return Err("LSP 源文件超过 2 MiB 限制".into());
    }
    let session = session_for(&server, &resolved, &workspace, &root_uri).await;
    check_cancelled(&cancellation)?;
    // Open once, then only sync real changes: the process is resident, so an
    // unchanged file must not be re-opened (or re-sent) on every request.
    session.sync_document(&uri, &language, &path).await?;
    check_cancelled(&cancellation)?;
    if matches!(operation, LspOperation::Diagnostics) {
        let generation = session.ensure_started().await?;
        let capabilities = generation.capabilities();
        // Older servers publish diagnostics rather than implementing pull
        // diagnostics; there is no request to send, only a publish to wait for.
        if capabilities
            .get("diagnosticProvider")
            .map_or(true, Value::is_null)
        {
            let diagnostics = match session.wait_for_diagnostics(&generation, &uri).await {
                Ok(value) => value,
                Err(RequestFailure::Gone(note)) => {
                    // A restart replays `didOpen`, so the new process publishes
                    // diagnostics for this document again.
                    session.restart(&note).await?;
                    let generation = session.ensure_started().await?;
                    session
                        .wait_for_diagnostics(&generation, &uri)
                        .await
                        .map_err(RequestFailure::message)?
                }
                Err(failure) => return Err(failure.message()),
            };
            return bounded_result(
                &server.id,
                &language,
                &workspace,
                json!({"items": diagnostics}),
            );
        }
    }
    let params = operation_params(operation, &uri, &params)?;
    let result = session.request(operation.method(), params).await?;
    bounded_result(
        &server.id,
        &language,
        &workspace,
        shape_result(operation, result),
    )
}

pub(super) fn bounded_result(
    server: &str,
    language: &str,
    workspace: &Path,
    result: Value,
) -> Result<Value, String> {
    let result_text = serde_json::to_string(&result).map_err(|error| error.to_string())?;
    if result_text.chars().count() > MAX_LSP_RESULT_CHARS {
        return Ok(
            json!({"server":server,"language":language,"workspace":workspace,"truncated":true,
            "preview":result_text.chars().take(MAX_LSP_RESULT_CHARS).collect::<String>()}),
        );
    }
    Ok(json!({"server":server,"language":language,"workspace":workspace,"result":result}))
}

pub async fn execute(
    path: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    operation: LspOperation,
    params: LspParams,
    cancellation: Arc<AtomicBool>,
) -> Result<Value, String> {
    let result = tokio::select! {
        result = timeout(LSP_TIMEOUT, run_request(path, workspace, config_path, operation, params, cancellation.clone())) => {
            result.map_err(|_| format!("LSP 服务器无响应（已等待 {} 秒）", LSP_TIMEOUT.as_secs()))?
        }
        _ = wait_for_cancellation(cancellation) => Err("请求已取消".to_string()),
    };
    result
}
