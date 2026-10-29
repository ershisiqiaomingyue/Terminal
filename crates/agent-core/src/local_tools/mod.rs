//! Standard local tools shared by the desktop runtime and future CLI hosts.
//!
//! This module deliberately contains no Tauri types. A host only supplies the
//! selected roots, cancellation flag, and optional live-output sink; the tool
//! definitions and their filesystem/process behavior stay in the core crate.

mod commands;
mod context;
mod files;
mod patches;
mod registry;
mod search;
mod utilities;
mod wsl;

#[cfg(test)]
mod tests;

pub use context::{LocalToolContext, PATH_ACCESS_PROBE_TOOL};
pub use registry::register_local_tools;
