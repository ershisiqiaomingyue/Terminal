use futures_util::StreamExt;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{timeout, Duration};

const MCP_MAX_TIMEOUT_SECS: u64 = 300;
const MCP_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MCP_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MCP_PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub transport: McpTransport,
    pub enabled: bool,
    pub timeout_secs: u64,
}

impl McpServerConfig {
    pub fn stdio(id: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            id: id.into(),
            transport: McpTransport::Stdio {
                command: command.into(),
                args,
                env: BTreeMap::new(),
            },
            enabled: true,
            timeout_secs: 30,
        }
    }

    pub fn http(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            transport: McpTransport::Http {
                url: url.into(),
                headers: BTreeMap::new(),
            },
            enabled: true,
            timeout_secs: 30,
        }
    }

    pub fn validate(&self) -> Result<(), McpConfigError> {
        if self.id.trim().is_empty() {
            return Err(McpConfigError::EmptyId);
        }
        if self.timeout_secs == 0 || self.timeout_secs > MCP_MAX_TIMEOUT_SECS {
            return Err(McpConfigError::InvalidTimeout);
        }
        match &self.transport {
            McpTransport::Stdio { command, .. } if command.trim().is_empty() => {
                Err(McpConfigError::EmptyCommand)
            }
            McpTransport::Http { url, .. }
                if !(url.starts_with("http://") || url.starts_with("https://")) =>
            {
                Err(McpConfigError::InvalidUrl)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum McpConfigError {
    #[error("MCP server id must not be empty")]
    EmptyId,
    #[error("MCP stdio command must not be empty")]
    EmptyCommand,
    #[error("MCP HTTP URL must use http or https")]
    InvalidUrl,
    #[error("MCP timeout must be between 1 and 300 seconds")]
    InvalidTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum McpHealth {
    Disconnected,
    Checking,
    Connected { tool_count: usize },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpCallResult {
    pub content: Vec<Value>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

pub fn mcp_initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "android-tool-agent", "version": "0.1.0" }
        }
    })
}

pub fn mcp_tools_list_request(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} })
}

pub fn mcp_initialized_notification() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} })
}

pub fn mcp_call_request(id: u64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

pub fn parse_mcp_result(value: &Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP JSON-RPC error");
        return Err(truncate_message(message));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "MCP response has no result".to_string())
}

static MCP_HTTP_SESSIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static MCP_STDIO_SESSIONS: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<McpStdioSession>>>>> =
    OnceLock::new();

struct McpStdioSession {
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
}

fn mcp_http_session_key(config: &McpServerConfig) -> Option<String> {
    let McpTransport::Http { url, headers } = &config.transport else {
        return None;
    };
    Some(serde_json::to_string(&(url, headers)).unwrap_or_else(|_| url.clone()))
}

fn mcp_stdio_session_key(config: &McpServerConfig) -> Option<String> {
    matches!(&config.transport, McpTransport::Stdio { .. })
        .then(|| serde_json::to_string(&config.transport).unwrap_or_default())
}

fn forget_mcp_stdio_session(config: &McpServerConfig) {
    let Some(key) = mcp_stdio_session_key(config) else {
        return;
    };
    if let Ok(mut sessions) = MCP_STDIO_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        sessions.remove(&key);
    }
}

fn has_cached_mcp_session(config: &McpServerConfig) -> bool {
    cached_mcp_http_session(config).is_some()
        || mcp_stdio_session_key(config)
            .and_then(|key| {
                MCP_STDIO_SESSIONS
                    .get_or_init(|| Mutex::new(HashMap::new()))
                    .lock()
                    .ok()
                    .map(|sessions| sessions.contains_key(&key))
            })
            .unwrap_or(false)
}

fn cached_mcp_http_session(config: &McpServerConfig) -> Option<String> {
    let key = mcp_http_session_key(config)?;
    MCP_HTTP_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|sessions| sessions.get(&key).cloned())
}

fn remember_mcp_http_session(config: &McpServerConfig, session_id: Option<&str>) {
    let Some(key) = mcp_http_session_key(config) else {
        return;
    };
    let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    if let Ok(mut sessions) = MCP_HTTP_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        sessions.insert(key, session_id.to_string());
    }
}

fn forget_mcp_http_session(config: &McpServerConfig) {
    let Some(key) = mcp_http_session_key(config) else {
        return;
    };
    if let Ok(mut sessions) = MCP_HTTP_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        sessions.remove(&key);
    }
}

pub fn mcp_clear_session(config: &McpServerConfig) {
    // HTTP sessions are kept only for the lifetime of the desktop process.
    // Explicit disconnect must release the cached protocol identity.
    forget_mcp_http_session(config);
    if let Some(key) = mcp_stdio_session_key(config) {
        if let Ok(mut sessions) = MCP_STDIO_SESSIONS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
        {
            sessions.remove(&key);
        }
    }
}

pub async fn mcp_list_tools(config: &McpServerConfig) -> Result<Vec<McpTool>, String> {
    config.validate().map_err(|error| error.to_string())?;
    let cached_session = cached_mcp_http_session(config);
    let requests = if has_cached_mcp_session(config) {
        vec![mcp_tools_list_request(1)]
    } else {
        vec![
            mcp_initialize_request(1),
            mcp_initialized_notification(),
            mcp_tools_list_request(2),
        ]
    };
    let result = match exchange(config, requests, cached_session).await {
        Ok(result) => result,
        Err(error) if has_cached_mcp_session(config) => {
            forget_mcp_http_session(config);
            forget_mcp_stdio_session(config);
            exchange(
                config,
                vec![
                    mcp_initialize_request(1),
                    mcp_initialized_notification(),
                    mcp_tools_list_request(2),
                ],
                None,
            )
            .await
            .map_err(|retry| format!("{error}; 重新初始化失败：{retry}"))?
        }
        Err(error) => return Err(error),
    };
    let result = parse_mcp_result(result.last().unwrap_or(&Value::Null))?;
    serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| json!([])))
        .map_err(|error| format!("MCP tools/list response is invalid: {error}"))
}

pub async fn mcp_call_tool(
    config: &McpServerConfig,
    name: &str,
    arguments: Value,
) -> Result<McpCallResult, String> {
    config.validate().map_err(|error| error.to_string())?;
    let cached_session = cached_mcp_http_session(config);
    let requests = if has_cached_mcp_session(config) {
        vec![mcp_call_request(1, name, arguments.clone())]
    } else {
        vec![
            mcp_initialize_request(1),
            mcp_initialized_notification(),
            mcp_call_request(2, name, arguments.clone()),
        ]
    };
    let result = match exchange(config, requests, cached_session).await {
        Ok(result) => result,
        Err(error) if has_cached_mcp_session(config) => {
            forget_mcp_http_session(config);
            forget_mcp_stdio_session(config);
            exchange(
                config,
                vec![
                    mcp_initialize_request(1),
                    mcp_initialized_notification(),
                    mcp_call_request(2, name, arguments),
                ],
                None,
            )
            .await
            .map_err(|retry| format!("{error}; 重新初始化失败：{retry}"))?
        }
        Err(error) => return Err(error),
    };
    serde_json::from_value(parse_mcp_result(result.last().unwrap_or(&Value::Null))?)
        .map_err(|error| format!("MCP tools/call response is invalid: {error}"))
}

async fn exchange(
    config: &McpServerConfig,
    requests: Vec<Value>,
    session_id: Option<String>,
) -> Result<Vec<Value>, String> {
    match &config.transport {
        McpTransport::Stdio { .. } => stdio_exchange(config, requests).await,
        McpTransport::Http { .. } => http_exchange(config, requests, session_id).await,
    }
}

async fn http_exchange(
    config: &McpServerConfig,
    requests: Vec<Value>,
    mut session_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let McpTransport::Http { url, headers } = &config.transport else {
        unreachable!();
    };
    let timeout_duration = Duration::from_secs(config.timeout_secs);
    let client = crate::network::http_client_builder()?
        .timeout(timeout_duration)
        .build()
        .map_err(|error| format!("MCP HTTP client failed: {error}"))?;
    let mut results = Vec::new();
    for request in requests {
        let has_id = request.get("id").is_some();
        let mut builder = client
            .post(url)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(&request);
        if let Some(session_id) = session_id.as_deref() {
            builder = builder.header("mcp-session-id", session_id);
        }
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        let response_result = timeout(timeout_duration, async move {
            let response = builder
                .send()
                .await
                .map_err(|error| format!("MCP HTTP request failed: {error}"))?;
            let status = response.status();
            let response_session_id = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            if !status.is_success() {
                let detail = match read_bounded_body(response).await {
                    Ok(body) if !body.is_empty() => {
                        format!(": {}", truncate_message(&String::from_utf8_lossy(&body)))
                    }
                    Ok(_) => String::new(),
                    Err(error) => format!(" ({error})"),
                };
                return Err(format!("MCP HTTP returned {}{}", status.as_u16(), detail));
            }
            let message = if has_id {
                Some(read_http_message(response).await?)
            } else {
                ensure_content_length(&response)?;
                None
            };
            Ok::<_, String>((response_session_id, message))
        })
        .await
        .map_err(|_| "MCP HTTP request timed out".to_string())??;
        if response_result.0.is_some() {
            session_id = response_result.0;
        }
        if let Some(message) = response_result.1 {
            results.push(message);
        }
    }
    remember_mcp_http_session(config, session_id.as_deref());
    Ok(results)
}

async fn stdio_exchange(
    config: &McpServerConfig,
    requests: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let McpTransport::Stdio { command, args, env } = &config.transport else {
        unreachable!();
    };
    if let Some(key) = mcp_stdio_session_key(config) {
        let cached = MCP_STDIO_SESSIONS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "MCP stdio session cache unavailable".to_string())?
            .get(&key)
            .cloned();
        if let Some(cached) = cached {
            let mut session = cached.lock().await;
            let result = stdio_session_exchange(&mut session, config, requests).await;
            if result.is_err() {
                terminate_stdio_child(&mut session.child).await;
                drop(session);
            }
            return result;
        }
    }
    let mut command_builder = Command::new(command);
    crate::network::apply_child_environment(&mut command_builder);
    command_builder
        .args(args)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command_builder.creation_flags(CREATE_NEW_PROCESS_GROUP | 0x0800_0000);
    }
    let mut child = command_builder
        .spawn()
        .map_err(|error| format!("MCP stdio process failed to start: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "MCP stdio stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "MCP stdio stdout unavailable".to_string())?;
    let mut session = McpStdioSession {
        child,
        stdin,
        reader: BufReader::new(stdout),
    };
    let result = stdio_session_exchange(&mut session, config, requests).await;
    if result.is_err() {
        terminate_stdio_child(&mut session.child).await;
        return result;
    }
    if let Some(key) = mcp_stdio_session_key(config) {
        MCP_STDIO_SESSIONS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "MCP stdio session cache unavailable".to_string())?
            .insert(key, Arc::new(AsyncMutex::new(session)));
    }
    result
}

async fn stdio_session_exchange(
    session: &mut McpStdioSession,
    config: &McpServerConfig,
    requests: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let request_timeout = Duration::from_secs(config.timeout_secs);
    let mut results = Vec::new();
    for request in requests {
        let mut request_bytes = serde_json::to_vec(&request)
            .map_err(|error| format!("MCP request is invalid: {error}"))?;
        request_bytes.push(b'\n');
        if request_bytes.len() > MCP_MAX_LINE_BYTES {
            return Err("MCP stdio request exceeds the 8 MiB line limit".to_string());
        }
        timeout(request_timeout, async {
            session
                .stdin
                .write_all(&request_bytes)
                .await
                .map_err(|error| format!("MCP stdio write failed: {error}"))?;
            session
                .stdin
                .flush()
                .await
                .map_err(|error| format!("MCP stdio flush failed: {error}"))
        })
        .await
        .map_err(|_| "MCP stdio request timed out while writing".to_string())??;
        let Some(expected_id) = request.get("id") else {
            continue;
        };
        let response = timeout(
            request_timeout,
            read_stdio_response(&mut session.reader, expected_id),
        )
        .await
        .map_err(|_| "MCP stdio request timed out while reading".to_string())??;
        results.push(response);
    }
    Ok(results)
}

async fn read_stdio_response<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    expected_id: &Value,
) -> Result<Value, String> {
    loop {
        let line = read_bounded_line(reader)
            .await
            .map_err(|error| format!("MCP stdio read failed: {error}"))?
            .ok_or_else(|| "MCP stdio closed before response".to_string())?;
        let value: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("MCP JSON-RPC response is invalid: {error}"))?;
        let Some(response_id) = value.get("id") else {
            continue;
        };
        if response_id == expected_id {
            return Ok(value);
        }
        return Err("MCP stdio response id does not match the request".to_string());
    }
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let length = newline + 1;
            if line.len().saturating_add(length) > MCP_MAX_LINE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "MCP stdio response line exceeds the 8 MiB limit",
                ));
            }
            line.extend_from_slice(&buffer[..length]);
            reader.consume(length);
            return Ok(Some(line));
        }
        if line.len().saturating_add(buffer.len()) > MCP_MAX_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP stdio response line exceeds the 8 MiB limit",
            ));
        }
        let length = buffer.len();
        line.extend_from_slice(buffer);
        reader.consume(length);
    }
}

async fn terminate_stdio_child(child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut taskkill_command = Command::new("taskkill");
        taskkill_command.creation_flags(0x0800_0000);
        taskkill_command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Ok(mut taskkill) = taskkill_command.spawn() {
            if timeout(MCP_PROCESS_CLEANUP_TIMEOUT, taskkill.wait())
                .await
                .is_err()
            {
                let _ = taskkill.start_kill();
                let _ = timeout(MCP_PROCESS_CLEANUP_TIMEOUT, taskkill.wait()).await;
            }
        }
    }
    let _ = child.start_kill();
    let _ = timeout(MCP_PROCESS_CLEANUP_TIMEOUT, child.wait()).await;
}

fn ensure_content_length(response: &Response) -> Result<(), String> {
    if response
        .content_length()
        .is_some_and(|length| length > MCP_MAX_RESPONSE_BYTES as u64)
    {
        return Err("MCP HTTP response exceeds the 8 MiB body limit".to_string());
    }
    Ok(())
}

async fn read_bounded_body(response: Response) -> Result<Vec<u8>, String> {
    ensure_content_length(&response)?;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("MCP HTTP response body read failed: {error}"))?;
        if body.len().saturating_add(chunk.len()) > MCP_MAX_RESPONSE_BYTES {
            return Err("MCP HTTP response exceeds the 8 MiB body limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_http_message(response: Response) -> Result<Value, String> {
    ensure_content_length(&response)?;
    let is_sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_sse {
        return read_sse_message(response).await;
    }
    let body = read_bounded_body(response).await?;
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err("MCP HTTP response body is empty".to_string());
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("MCP HTTP JSON response is invalid: {error}"))
}

async fn read_sse_message(response: Response) -> Result<Value, String> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut event_name = None;
    let mut event_data = Vec::new();
    let mut has_data = false;
    let mut total_bytes = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("MCP SSE response read failed: {error}"))?;
        total_bytes = total_bytes.saturating_add(chunk.len());
        if total_bytes > MCP_MAX_RESPONSE_BYTES {
            return Err("MCP SSE response exceeds the 8 MiB body limit".to_string());
        }
        pending.extend_from_slice(&chunk);
        if pending.len() > MCP_MAX_LINE_BYTES {
            return Err("MCP SSE line exceeds the 8 MiB limit".to_string());
        }
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if apply_sse_line(&line, &mut event_name, &mut event_data, &mut has_data)? {
                if let Some(message) =
                    finish_sse_event(&mut event_name, &mut event_data, &mut has_data)?
                {
                    return Ok(message);
                }
            }
        }
    }
    if !pending.is_empty() {
        if apply_sse_line(&pending, &mut event_name, &mut event_data, &mut has_data)? {
            if let Some(message) =
                finish_sse_event(&mut event_name, &mut event_data, &mut has_data)?
            {
                return Ok(message);
            }
        }
    }
    if let Some(message) = finish_sse_event(&mut event_name, &mut event_data, &mut has_data)? {
        return Ok(message);
    }
    Err("MCP SSE stream ended before a response event".to_string())
}

fn apply_sse_line(
    line: &[u8],
    event_name: &mut Option<String>,
    event_data: &mut Vec<u8>,
    has_data: &mut bool,
) -> Result<bool, String> {
    if line.is_empty() {
        return Ok(true);
    }
    if line[0] == b':' {
        return Ok(false);
    }
    let line = std::str::from_utf8(line)
        .map_err(|_| "MCP SSE response contains invalid UTF-8".to_string())?;
    if let Some(value) = line.strip_prefix("event:") {
        *event_name = Some(value.trim_start().to_string());
    } else if let Some(value) = line.strip_prefix("data:") {
        if *has_data {
            event_data.push(b'\n');
        }
        let value = value.strip_prefix(' ').unwrap_or(value);
        if event_data.len().saturating_add(value.len()) > MCP_MAX_LINE_BYTES {
            return Err("MCP SSE event exceeds the 8 MiB line limit".to_string());
        }
        event_data.extend_from_slice(value.as_bytes());
        *has_data = true;
    }
    Ok(false)
}

fn finish_sse_event(
    event_name: &mut Option<String>,
    event_data: &mut Vec<u8>,
    has_data: &mut bool,
) -> Result<Option<Value>, String> {
    if !*has_data {
        *event_name = None;
        return Ok(None);
    }
    let name = event_name.take();
    let data = std::mem::take(event_data);
    *has_data = false;
    if name.as_deref() == Some("endpoint") {
        return Err("MCP legacy SSE endpoint transport is not supported".to_string());
    }
    let value = serde_json::from_slice::<Value>(&data).map_err(|error| {
        if name.as_deref() == Some("error") {
            format!(
                "MCP SSE error event: {}",
                truncate_message(&String::from_utf8_lossy(&data))
            )
        } else {
            format!("MCP SSE event is not valid JSON: {error}")
        }
    })?;
    if name.as_deref() == Some("error") {
        return Err(format!(
            "MCP SSE error event: {}",
            sse_error_message(&value)
        ));
    }
    Ok(Some(value))
}

fn sse_error_message(value: &Value) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map_or_else(
            || truncate_message(&value.to_string()),
            |message| truncate_message(message),
        )
}

fn truncate_message(message: &str) -> String {
    message.chars().take(512).collect()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum McpRegistryError {
    #[error("invalid MCP configuration: {0}")]
    InvalidConfig(#[from] McpConfigError),
    #[error("MCP server is already registered: {0}")]
    Duplicate(String),
    #[error("MCP server is not registered: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone)]
struct McpServerState {
    config: McpServerConfig,
    health: McpHealth,
}

#[derive(Debug, Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, McpServerState>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, config: McpServerConfig) -> Result<(), McpRegistryError> {
        config.validate()?;
        if self.servers.contains_key(&config.id) {
            return Err(McpRegistryError::Duplicate(config.id));
        }
        let id = config.id.clone();
        self.servers.insert(
            id,
            McpServerState {
                config,
                health: McpHealth::Disconnected,
            },
        );
        Ok(())
    }

    pub fn health(&self, id: &str) -> Option<&McpHealth> {
        self.servers.get(id).map(|server| &server.health)
    }

    pub fn config(&self, id: &str) -> Option<&McpServerConfig> {
        self.servers.get(id).map(|server| &server.config)
    }

    pub fn mark_checking(&mut self, id: &str) -> Result<(), McpRegistryError> {
        let server = self.server_mut(id)?;
        server.health = McpHealth::Checking;
        Ok(())
    }

    pub fn mark_connected(&mut self, id: &str, tool_count: usize) -> Result<(), McpRegistryError> {
        let server = self.server_mut(id)?;
        server.health = McpHealth::Connected { tool_count };
        Ok(())
    }

    pub fn mark_error(
        &mut self,
        id: &str,
        message: impl Into<String>,
    ) -> Result<(), McpRegistryError> {
        let server = self.server_mut(id)?;
        server.health = McpHealth::Error {
            message: message.into(),
        };
        Ok(())
    }

    fn server_mut(&mut self, id: &str) -> Result<&mut McpServerState, McpRegistryError> {
        self.servers
            .get_mut(id)
            .ok_or_else(|| McpRegistryError::NotFound(id.to_string()))
    }
}
