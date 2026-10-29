//! Language-server integration organized by protocol and domain.

mod catalog;
mod config;
mod executable;
mod limits;
mod params;
mod protocol;
mod session;

pub use catalog::LspOperation;
pub(crate) use config::{
    __cmd__list_lsp_servers, __cmd__reset_lsp_servers, __cmd__save_lsp_server,
    __tauri_command_name_list_lsp_servers, __tauri_command_name_reset_lsp_servers,
    __tauri_command_name_save_lsp_server, list_lsp_servers, reset_lsp_servers, save_lsp_server,
};
pub(crate) use params::{execute, LspParams};
pub(crate) use protocol::workspace_root;
pub use session::shutdown_sessions_blocking;

#[cfg(test)]
mod tests {
    use crate::lsp::catalog::*;
    use crate::lsp::config::*;
    use crate::lsp::executable::*;
    use crate::lsp::protocol::*;
    use crate::lsp::session::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    include!("tests.rs");
}
