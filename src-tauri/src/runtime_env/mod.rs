use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::background_process::command as background_command;
use futures_util::{future::join_all, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;

const MAX_PROCESS_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENVIRONMENT_TOOL_DOWNLOAD_BYTES: u64 = 768 * 1024 * 1024;
const MAX_ENVIRONMENT_TOOL_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const ENVIRONMENT_TOOL_DOWNLOAD_PROGRESS_EVENT: &str = "environment-tool-download-progress";

static ENVIRONMENT_TOOL_CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVE_ENVIRONMENT_TOOL_DOWNLOADS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

mod types;
pub use types::*;
mod environment;
pub use environment::*;
mod node_runtime;
pub(crate) use node_runtime::{inspect_node_runtime, node_npm_cli, validate_node_runtime};
use node_runtime::{
    find_node, node_download_checksum, node_environment_tool_release, node_version_supported,
    validate_node_npm, verify_node_archive,
};
mod python_runtime;
pub(crate) use python_runtime::{find_python, inspect_python_runtime};
use python_runtime::{python_child_directories, python_environment_tool_release};
mod tool_paths;
pub(crate) use tool_paths::environment_tool_command;
mod transport;
use transport::{
    command_output_with_timeout, command_output_with_timeout_limit,
};
