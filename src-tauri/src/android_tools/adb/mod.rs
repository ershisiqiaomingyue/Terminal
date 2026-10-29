//! ADB integration organized by device workflow.

mod backup;
mod connection;
mod device;
mod diagnostics;
mod files;
mod package;
mod privilege;
mod query;
mod screen;
mod support;

use super::*;
pub use backup::*;
pub use connection::*;
pub use device::*;
pub use diagnostics::*;
pub use files::*;
pub use package::*;
pub use privilege::*;
pub use query::*;
pub use screen::*;
pub(crate) use support::*;

#[cfg(test)]
mod tests;
