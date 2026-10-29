use crate::{AllowPermission, ToolRegistry};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

mod command_tests;
mod files;
mod search_tests;

pub(super) fn expected_local_tool_names() -> Vec<&'static str> {
    let mut names = vec![
        "get_current_time",
        "get_working_directory",
        "list_local_files",
        "search_local_files",
        "search_local_code",
        "get_directory_tree",
        "apply_patch",
        "read_local_file",
        "write_local_file",
        "create_directory",
        "copy_file",
        "move_file",
        "delete_file",
        "run_local_command",
        "get_git_status",
        "get_git_diff",
        "run_project_check",
        "run_local_shell",
        crate::PATH_ACCESS_PROBE_TOOL,
    ];
    names.sort_unstable();
    names
}

pub(super) fn temporary_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("android-tool-agent-files-{suffix}"))
}

#[tokio::test]
async fn registered_local_tools_keep_their_complete_public_contract() {
    let mut registry = ToolRegistry::new();
    let context = crate::LocalToolContext::new(
        vec![temporary_root().to_string_lossy().into_owned()],
        vec![],
        false,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        None,
    );
    crate::register_local_tools(&mut registry, context).expect("local tools should register");
    assert_eq!(
        registry
            .list()
            .into_iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        expected_local_tool_names()
    );
    assert!(registry
        .execute("get_current_time", serde_json::json!({}), &AllowPermission)
        .await
        .is_ok());
}
