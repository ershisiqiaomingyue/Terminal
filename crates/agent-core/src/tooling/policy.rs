mod command_safety;
mod names;
mod path_parsing;
mod permission_input;
mod remote_paths;
mod scope_keys;
mod security;
#[cfg(test)]
mod tests;

pub use command_safety::*;
pub use names::{
    canonical_tool_name, path_access_tool, permission_requires_confirmation, scoped_tool_name,
};
pub use path_parsing::{
    collect_declared_path_values, command_contains_path, command_path_tokens,
    path_access_candidates, remote_path_access_candidates,
};
pub use permission_input::*;
pub use remote_paths::external_path_scope_key_for_target;
pub use scope_keys::*;
pub use security::*;

// The root remains a stable export surface. Implementation lives in focused
// modules under this directory so command, path, and scope rules stay separate.
