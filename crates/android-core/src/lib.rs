//! Android data parsing without desktop framework or command execution dependencies.
//!
//! Hosts supply ADB output or trace bytes; this crate validates and parses them.

pub mod adb;
pub mod trace;
