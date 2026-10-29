//! Tauri compatibility surface for the host-independent log engine.
//!
//! The implementation lives in `agent-core` so the desktop app and future
//! CLI use the same bounded archive scanning, cancellation, and timeline code.
pub use agent_core::{
    index_log_paths, read_log_range, read_log_range_with_cancellation, rebuild_timeline,
    rebuild_timeline_with_cancellation, search_log_paths, search_log_paths_with_cancellation,
};
