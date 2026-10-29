pub(in crate::local_tools) use crate::MAX_COMMAND_TIMEOUT_MS;
use crate::{CommandOutputSink, WslRuntimeConfig};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::wsl::WslPathMapper;

pub(super) const MAX_SEARCH_RESULTS: usize = 256;
pub(super) const MAX_SEARCH_FILE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_TREE_DEPTH: usize = 8;
// The whole prefix is injected into the model transcript, so one read must stay
// far below the context window: 512 KB could add roughly 130k tokens to a single
// turn. 64 KB is still a large slice of a source file; read past it with the
// start_line/end_line window, which addresses real file lines and is therefore
// not limited by this bound.
pub(super) const MAX_FILE_READ_BYTES: usize = 64 * 1024;
pub(super) const MAX_FILE_WRITE_BYTES: usize = 256 * 1024;
pub(super) const MAX_SHELL_COMMAND_CHARS: usize = 32 * 1024;

/// Internal runtime-only tool used to request path permission before a model
/// has produced a complete write or command call.
pub const PATH_ACCESS_PROBE_TOOL: &str = "__agent_path_access_probe";

#[derive(Clone)]
pub struct LocalToolContext {
    pub(super) roots: Vec<String>,
    pub(super) excluded_paths: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
    pub(super) output_sink: Option<CommandOutputSink>,
    pub(super) wsl: Option<WslRuntimeConfig>,
    pub(super) command_timeout_limit: u64,
}

impl LocalToolContext {
    pub fn new(
        roots: Vec<String>,
        excluded_paths: Vec<String>,
        allow_external_paths: bool,
        cancellation: Arc<AtomicBool>,
        output_sink: Option<CommandOutputSink>,
    ) -> Self {
        Self {
            roots,
            excluded_paths,
            allow_external_paths,
            cancellation,
            output_sink,
            wsl: None,
            command_timeout_limit: MAX_COMMAND_TIMEOUT_MS,
        }
    }

    /// Route the same structured file tools through a local WSL distribution.
    /// Files are accessed through the WSL UNC share while commands are started
    /// with `wsl.exe`, so the model-facing tool names remain unchanged.
    pub fn with_wsl(mut self, config: WslRuntimeConfig) -> Self {
        let Ok(config) = config.normalized() else {
            return self;
        };
        let mapper = WslPathMapper::new(config.clone());
        self.roots = self
            .roots
            .iter()
            .map(|root| mapper.to_unc_path(root))
            .collect();
        self.excluded_paths = self
            .excluded_paths
            .iter()
            .map(|path| {
                if path.starts_with('/') || path.starts_with('~') {
                    mapper.to_unc_path(path)
                } else {
                    path.clone()
                }
            })
            .collect();
        self.wsl = Some(config);
        self
    }

    pub fn with_output_sink(&self, output_sink: Option<CommandOutputSink>) -> Self {
        Self {
            roots: self.roots.clone(),
            excluded_paths: self.excluded_paths.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink,
            wsl: self.wsl.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_allow_external_paths(&self, allow_external_paths: bool) -> Self {
        Self {
            roots: self.roots.clone(),
            excluded_paths: self.excluded_paths.clone(),
            allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink: self.output_sink.clone(),
            wsl: self.wsl.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub fn with_cancellation(&self, cancellation: Arc<AtomicBool>) -> Self {
        Self {
            roots: self.roots.clone(),
            excluded_paths: self.excluded_paths.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation,
            output_sink: self.output_sink.clone(),
            wsl: self.wsl.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    pub(super) fn for_tool(&self, tool_name: &'static str) -> Self {
        Self {
            roots: self.roots.clone(),
            excluded_paths: self.excluded_paths.clone(),
            allow_external_paths: self.allow_external_paths,
            cancellation: self.cancellation.clone(),
            output_sink: self
                .output_sink
                .as_ref()
                .map(|sink| sink.with_tool(tool_name)),
            wsl: self.wsl.clone(),
            command_timeout_limit: self.command_timeout_limit,
        }
    }

    /// Only a host-owned background job may extend the foreground time limit.
    pub(crate) fn for_background(&self) -> Self {
        let mut context = self.clone();
        context.command_timeout_limit = crate::background::MAX_BACKGROUND_TIMEOUT_MS;
        context
    }

    pub(crate) fn environment_label(&self) -> &'static str {
        if self.wsl.is_some() {
            "wsl"
        } else {
            "local"
        }
    }
}
