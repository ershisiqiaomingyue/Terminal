//! Resource limits for requests and persisted configuration.

use std::time::Duration;

pub(super) const MAX_LSP_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_LSP_RESULT_CHARS: usize = 48 * 1024;
pub(super) const LSP_TIMEOUT: Duration = Duration::from_secs(45);
/// A resident process is reclaimed after this much idle time. Overridable with
/// `ANDROID_TOOL_LSP_IDLE_SECONDS` for users who keep many servers installed.
pub(super) const LSP_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// How often the background reaper looks for idle sessions.
pub(super) const LSP_REAP_INTERVAL: Duration = Duration::from_secs(15);
/// Consecutive automatic restarts allowed without a successful response in
/// between; the first crash after a healthy answer always gets its retry.
pub(super) const MAX_CONSECUTIVE_RESTARTS: usize = 2;
/// Completion candidates and workspace symbols are capped before they reach the
/// model; `total`/`truncated` tell it what it is not seeing.
pub(super) const MAX_COMPLETION_ITEMS: usize = 50;
pub(super) const MAX_WORKSPACE_SYMBOLS: usize = 50;

/// LSP configuration payload ceiling.
pub(super) const MAX_LSP_CONFIG_BYTES: usize = 256 * 1024;
