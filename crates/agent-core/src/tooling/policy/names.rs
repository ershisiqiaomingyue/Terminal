const ENVIRONMENT_TOOL_PREFIXES: &[(&str, &str)] =
    &[("windows__", "windows"), ("ssh__", "ssh"), ("wsl__", "wsl")];

/// Split an explicitly environment-qualified tool name into its target and
/// canonical handler name. Double underscores make collisions with ordinary
/// MCP or application tool names unlikely.
pub fn scoped_tool_name(tool_name: &str) -> (Option<&str>, &str) {
    for (prefix, target) in ENVIRONMENT_TOOL_PREFIXES {
        if let Some(canonical) = tool_name.strip_prefix(prefix) {
            if !canonical.is_empty() {
                return (Some(*target), canonical);
            }
        }
    }
    (None, tool_name)
}

pub fn canonical_tool_name(tool_name: &str) -> &str {
    scoped_tool_name(tool_name).1
}

pub fn permission_requires_confirmation(mode: &str, requires_permission: bool) -> bool {
    requires_permission && mode != "full"
}

pub fn path_access_tool(tool_name: &str) -> bool {
    let tool_name = canonical_tool_name(tool_name);
    matches!(
        tool_name,
        "get_working_directory"
            | "__agent_path_access_probe"
            | "list_local_files"
            | "get_file_info"
            | "search_local_files"
            | "search_local_code"
            | "get_directory_tree"
            | "read_local_file"
            | "write_local_file"
            | "create_directory"
            | "copy_file"
            | "move_file"
            | "copy_directory"
            | "move_directory"
            | "delete_file"
            | "upload_file"
            | "download_file"
            | "upload_directory"
            | "download_directory"
            | "upload_paths"
            | "download_path"
            | "apply_patch"
            | "lsp_diagnostics"
            | "lsp_symbols"
            | "lsp_definition"
            | "lsp_references"
            | "get_git_status"
            | "get_git_diff"
            | "read_local_log_range"
            | "search_local_logs"
            | "rebuild_log_timeline"
            | "run_project_check"
            | "run_local_command"
            | "run_local_shell"
    )
}
