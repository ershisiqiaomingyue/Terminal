use super::*;

pub(crate) fn environment_tool_command(program: &Path) -> Command {
    // Rust wraps .bat/.cmd in cmd.exe and escapes batch arguments at spawn time.
    // An explicit cmd.exe wrapper would bypass that batch-specific escaping.
    let mut command = Command::new(normalized_environment_tool_path(program));
    for (name, value) in agent_core::child_proxy_environment() {
        command.env(name, value);
    }
    command
}
