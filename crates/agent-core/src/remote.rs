//! SSH-backed Agent tools.
//!
//! The model-facing tool names intentionally match the local tools. A host
//! only needs to choose an execution target; the timeline and permission
//! protocol stay unchanged for local and remote workspaces.

#[cfg(all(test, windows))]
#[path = "../tests/support/ssh_fixture.rs"]
mod ssh_fixture;

use crate::{
    is_allowed_agent_path, is_forbidden_destructive_command_line, resolve_selected_path,
    CommandOutputSink, ToolDefinition, ToolHandler, ToolRegistry,
};
use async_trait::async_trait;
use base64::Engine;
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ssh2::{Channel, Session};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const MAX_REMOTE_FILE_BYTES: usize = 64 * 1024;
const MAX_REMOTE_PATCH_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REMOTE_TRANSFER_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REMOTE_DIRECTORY_ENTRIES: usize = 4096;
const MAX_REMOTE_SEARCH_RESULTS: usize = 256;
const REMOTE_SEARCH_BUDGET_SECONDS: u64 = 20;
const REMOTE_SEARCH_STATUS_PREFIX: &str = "\u{1e}android-tool-search-status\t";
const REMOTE_SEARCH_ERROR_PREFIX: &str = "\u{1e}android-tool-search-error\t";
const MAX_REMOTE_SEARCH_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_REMOTE_TREE_ENTRIES: usize = 1024;
const MAX_REMOTE_PROGRESS_EVENTS: usize = 384;
/// SFTP 读写分片。libssh2 每个 `sftp.write` 最多发一个 32 KB 的 SSH 包，
/// 8 KB 分片等于把大文件切成几千个往返，上传/下载因此慢到不可用。
const REMOTE_SFTP_CHUNK_BYTES: usize = 32 * 1024;
/// 单个文件的进度事件最小间隔（跨线程回调 + 前端逐条入时间线）。
const REMOTE_PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
/// Uploads above this size skip the russh `input` buffer and stream straight
/// through the ssh.exe stdin pipe, so a 100 MB file never becomes a 100 MB
/// allocation plus its base64 copy.
const STREAM_VIA_RUSSH_LIMIT: u64 = 8 * 1024 * 1024;
const REMOTE_UPLOAD_STREAM_BYTES: usize = 64 * 1024;
const DEFAULT_REMOTE_COMMAND_SECONDS: u64 = 120;
const MAX_REMOTE_COMMAND_SECONDS: u64 = 30 * 60;
const MAX_REMOTE_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const SSH2_ERROR_EAGAIN: i32 = -37;
const MAX_CACHED_SSH_CONNECTIONS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshConnectionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(rename = "privateKeyPath", default)]
    pub private_key_path: Option<String>,
    #[serde(rename = "knownHostsPath", default)]
    pub known_hosts_path: Option<String>,
    #[serde(rename = "strictHostKey", default = "default_strict_host_key")]
    pub strict_host_key: bool,
    #[serde(rename = "defaultRoot", default)]
    pub default_root: Option<String>,
    #[serde(rename = "reuseConnection", default = "default_reuse_connection")]
    pub reuse_connection: bool,
    #[serde(
        rename = "connectTimeoutSecs",
        default = "default_connect_timeout_secs"
    )]
    pub connect_timeout_secs: u64,
    #[serde(
        rename = "operationTimeoutSecs",
        default = "default_operation_timeout_secs"
    )]
    pub operation_timeout_secs: u64,
    #[serde(
        rename = "keepaliveIntervalSecs",
        default = "default_keepalive_interval_secs"
    )]
    pub keepalive_interval_secs: u64,
}

fn default_strict_host_key() -> bool {
    false
}

fn default_reuse_connection() -> bool {
    true
}

fn default_connect_timeout_secs() -> u64 {
    10
}

fn default_operation_timeout_secs() -> u64 {
    DEFAULT_REMOTE_COMMAND_SECONDS
}

fn default_keepalive_interval_secs() -> u64 {
    30
}

static SSH_CONNECTION_POOL: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
// A libssh2 Session (and its SFTP channels) is not safe to advance from
// multiple blocking workers at once. Keep the model's parallel tool calls,
// but serialize operations that share one cached SSH transport.
// Keep only weak references here. A connection operation owns the strong Arc;
// once it finishes, the next lookup can discard the key instead of retaining
// every historical host/configuration for the lifetime of the process.
static SSH_OPERATION_LOCKS: OnceLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> = OnceLock::new();
static OPENSSH_TRANSPORTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
// v3 §21.4 S0：同一服务器并发「新建握手」的排队名额注册表。缓存命中不占
// 名额，只有真实 TCP+握手+认证（含回退路径的 ssh.exe 子进程）受限。
static SSH_HANDSHAKE_SLOTS: OnceLock<Mutex<HashMap<String, Weak<SshHandshakeSlot>>>> =
    OnceLock::new();

fn ssh_handshake_slots() -> &'static Mutex<HashMap<String, Weak<SshHandshakeSlot>>> {
    SSH_HANDSHAKE_SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ssh_connection_pool() -> &'static Mutex<HashMap<String, Session>> {
    SSH_CONNECTION_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ssh_operation_locks() -> &'static Mutex<HashMap<String, Weak<Mutex<()>>>> {
    SSH_OPERATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn openssh_transports() -> &'static Mutex<HashSet<String>> {
    OPENSSH_TRANSPORTS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Clone)]
pub struct SshToolContext {
    config: SshConnectionConfig,
    root: String,
    local_transfer_roots: Vec<String>,
    password: Option<String>,
    allow_external_paths: bool,
    cancellation: Arc<AtomicBool>,
    output_sink: Option<CommandOutputSink>,
    command_timeout_limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshDirectoryEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SshRemoteStatus {
    #[serde(rename = "latencyMs")]
    pub latency_ms: u64,
    /// 当前生效的传输引擎（v3 §21.4 S5b：russh / libssh2 / ssh.exe（兼容））。
    #[serde(rename = "engine", default)]
    pub engine: String,
    pub hostname: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "cpuCount")]
    pub cpu_count: Option<u32>,
    #[serde(rename = "cpuPhysicalCores", default)]
    pub cpu_physical_cores: Option<u32>,
    #[serde(rename = "cpuUsagePercent", default)]
    pub cpu_usage_percent: Option<f64>,
    #[serde(rename = "loadAverage")]
    pub load_average: Option<f64>,
    #[serde(rename = "loadAverage5", default)]
    pub load_average_5: Option<f64>,
    #[serde(rename = "loadAverage15", default)]
    pub load_average_15: Option<f64>,
    #[serde(rename = "memoryTotalBytes")]
    pub memory_total_bytes: Option<u64>,
    #[serde(rename = "memoryAvailableBytes")]
    pub memory_available_bytes: Option<u64>,
    #[serde(rename = "diskTotalBytes")]
    pub disk_total_bytes: Option<u64>,
    #[serde(rename = "diskUsedBytes", default)]
    pub disk_used_bytes: Option<u64>,
    #[serde(rename = "diskAvailableBytes")]
    pub disk_available_bytes: Option<u64>,
    #[serde(rename = "diskUsedPercent")]
    pub disk_used_percent: Option<f64>,
    #[serde(rename = "checkedPath")]
    pub checked_path: String,
    /// 每个逻辑核的当前使用率（%，下标即核心号）。探针按 /proc/stat 的 cpuN 差值计算。
    #[serde(rename = "cpuCores", default)]
    pub cpu_cores: Option<Vec<f64>>,
    /// /proc/meminfo MemFree（真正的空闲）。
    #[serde(rename = "memoryFreeBytes", default)]
    pub memory_free_bytes: Option<u64>,
    /// Buffers + Cached + SReclaimable（可回收缓存）。
    #[serde(rename = "memoryCachedBytes", default)]
    pub memory_cached_bytes: Option<u64>,
    /// 网卡累计收发字节（排除 lo）与后端差值算出的实时速率（字节/秒）。
    #[serde(rename = "netRxBytes", default)]
    pub net_rx_bytes: Option<u64>,
    #[serde(rename = "netTxBytes", default)]
    pub net_tx_bytes: Option<u64>,
    #[serde(rename = "netRxRateBps", default)]
    pub net_rx_rate_bps: Option<f64>,
    #[serde(rename = "netTxRateBps", default)]
    pub net_tx_rate_bps: Option<f64>,
    /// 关注的挂载点（/ 与 /data）用量。
    #[serde(rename = "disks", default)]
    pub disks: Vec<RemoteDiskMount>,
    /// CPU 占用前几名进程。
    #[serde(rename = "topProcesses", default)]
    pub top_processes: Vec<RemoteProcessStatus>,
}

/// 单个挂载点用量（SSH 状态面板：/ 与 /data）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDiskMount {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

/// CPU 占用排名里的一个远端进程。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProcessStatus {
    pub pid: u64,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshHostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    #[serde(rename = "knownHostStatus")]
    pub known_host_status: String,
}

/// Result of a bounded non-interactive command executed on an SSH host.
/// Desktop features use this for structured workflows such as AOSP probing;
/// Agent command execution continues to use the tool registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

impl SshToolContext {
    pub fn new(
        config: SshConnectionConfig,
        root: impl Into<String>,
        password: Option<String>,
        cancellation: Arc<AtomicBool>,
        output_sink: Option<CommandOutputSink>,
    ) -> Self {
        Self {
            config,
            root: normalize_remote_path(&root.into()),
            local_transfer_roots: Vec::new(),
            password,
            allow_external_paths: false,
            cancellation,
            output_sink,
            command_timeout_limit: MAX_REMOTE_COMMAND_SECONDS * 1000,
        }
    }

    fn with_tool(&self, tool_name: impl Into<String>) -> Self {
        Self {
            config: self.config.clone(),
            root: self.root.clone(),
            local_transfer_roots: self.local_transfer_roots.clone(),
            password: self.password.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink: self
                .output_sink
                .as_ref()
                .map(|sink| sink.with_tool(tool_name)),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_output_sink(&self, output_sink: Option<CommandOutputSink>) -> Self {
        Self {
            config: self.config.clone(),
            root: self.root.clone(),
            local_transfer_roots: self.local_transfer_roots.clone(),
            password: self.password.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink,
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_allow_external_paths(&self, allow_external_paths: bool) -> Self {
        Self {
            config: self.config.clone(),
            root: self.root.clone(),
            local_transfer_roots: self.local_transfer_roots.clone(),
            password: self.password.clone(),
            allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink: self.output_sink.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_cancellation(&self, cancellation: Arc<AtomicBool>) -> Self {
        Self {
            config: self.config.clone(),
            root: self.root.clone(),
            local_transfer_roots: self.local_transfer_roots.clone(),
            password: self.password.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation,
            output_sink: self.output_sink.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_local_transfer_roots(&self, roots: Vec<String>) -> Self {
        Self {
            config: self.config.clone(),
            root: self.root.clone(),
            local_transfer_roots: roots,
            password: self.password.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink: self.output_sink.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub(crate) fn for_background(&self) -> Self {
        let mut context = self.clone();
        context.command_timeout_limit = crate::background::MAX_BACKGROUND_TIMEOUT_MS;
        context
    }

    pub fn root(&self) -> &str {
        &self.root
    }
}

mod context_tools;
pub use context_tools::*;

mod connection;
pub use connection::*;
mod status;
pub use status::{enrich_linux_resource_status, parse_wsl_resource_status, LINUX_RESOURCE_PROBE};

mod browser;
pub use browser::*;
mod browser_git;
pub use browser_git::*;
mod workspace_git;
pub use workspace_git::*;
mod browser_write;
pub use browser_write::*;
mod browser_mutations;
pub use browser_mutations::*;
mod browser_transfer;
pub use browser_transfer::*;
mod download_access;
pub use download_access::SshDownloadAccess;
#[cfg(all(test, windows))]
mod browser_transfer_tests;
mod browser_transfer_transport;
use connection::{
    connect_ssh_tcp, default_known_hosts_path, expand_local_config_path,
    run_openssh_context_command, run_openssh_context_command_checked, run_openssh_pty_command,
    stream_file_to_openssh, uses_openssh_fallback,
};
#[cfg(test)]
use connection::{
    openssh_private_key_path, remember_openssh_transport, run_openssh_command_with_cancellation,
    should_fallback_to_openssh,
};
mod paths;
#[cfg(test)]
use paths::is_remote_env_name;
use paths::{
    cancelled, ensure_remote_path_safe, is_symbolic_remote_path, normalize_remote_path,
    remote_directory_entry_path, remote_output_sink_for_input, remote_path,
    remote_path_is_same_or_descendant, remote_shell_path_expression, search_match_kind, ssh_error,
};
mod keys;
use keys::{openssh_private_key_to_pem, write_temporary_private_key};
mod session;
pub use session::*;
use session::{
    await_remote, connect, connect_fresh, connection_pool_key, invalidate_cached_connection,
    read_remote_bytes, retry_io_with_cancel, retry_nonblocking, retry_nonblocking_with_cancel,
    send_session_keepalive, spawn_remote, write_remote_bytes,
};
#[cfg(test)]
use session::{connection_operation_lock, run_blocking};
mod file_support;
use file_support::{
    ensure_openssh_remote_directory, openssh_directory_copy_command, openssh_directory_file_list,
    openssh_directory_listing_command, openssh_directory_move_command,
    parse_openssh_directory_result, remote_relative_path_from_base, remote_text, shell_escape,
};
mod command_transport;
mod russh_runtime;
pub use russh_runtime::*;
#[cfg(all(test, windows))]
mod cleanup_tests;
#[cfg(test)]
mod russh_reentry_tests;
#[cfg(test)]
use command_transport::execute_remote_control;
use command_transport::{
    command_line, execute_remote_command, execute_remote_pty_command, finish_remote_operation,
    finish_remote_sink, remote_command_scripts,
};
mod file_queries;
use file_queries::{RemoteFileInfoTool, RemoteListFilesTool, RemoteWorkingDirectoryTool};
mod search;
#[cfg(test)]
use search::{remote_search_command, remote_search_result};
use search::{RemoteSearchCodeTool, RemoteSearchFilesTool};
mod file_mutations;
use file_mutations::{
    remote_mutation_path, remote_parent_path, remote_sftp_lstat, remote_sftp_lstat_if_exists,
    RemoteCopyDirectoryTool, RemoteCopyFileTool, RemoteCreateDirectoryTool, RemoteDeleteFileTool,
    RemoteMoveDirectoryTool, RemoteMoveFileTool, RemoteReadFileTool, RemoteWriteFileTool,
};
mod archive_tools;
mod transfer;
use archive_tools::{RemoteDownloadPathTool, RemoteUploadPathsTool};
use transfer::{
    download_remote_file_to_local, RemoteDownloadDirectoryTool, RemoteDownloadFileTool,
    RemoteSetFilePermissionsTool, RemoteUploadDirectoryTool, RemoteUploadFileTool,
};
mod patch;
#[cfg(test)]
use patch::apply_remote_patch_text;
use patch::RemoteApplyPatchTool;
mod command_tools;
use command_tools::{
    RemoteCommandTool, RemoteGitTool, RemoteInteractiveShellTool, RemoteProjectCheckTool,
    RemoteShellTool,
};
mod tree;
use tree::RemoteTreeTool;
#[cfg(test)]
mod tests;
