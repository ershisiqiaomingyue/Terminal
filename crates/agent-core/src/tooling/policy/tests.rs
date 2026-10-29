use super::*;
use crate::{ToolDefinition, ToolRegistry};
use serde_json::json;
#[test]
fn remote_read_only_tools_can_share_a_parallel_group_without_bypassing_permissions() {
    let mut registry = ToolRegistry::new();
    registry
        .register(
            ToolDefinition::new(
                "ssh__read_local_file",
                "remote read",
                json!({ "type": "object" }),
            )
            .read_only(),
        )
        .expect("remote read definition");
    registry
        .register(
            ToolDefinition::new(
                "ssh__run_local_command",
                "remote command",
                json!({ "type": "object" }),
            )
            .requiring_permission(),
        )
        .expect("remote command definition");

    let roots = vec!["/home/builder/aosp".to_string()];
    assert!(is_parallel_safe_tool_for_target(
        "ssh__read_local_file",
        &json!({ "path": "/home/builder/aosp/README.md" }),
        "full",
        &roots,
        &registry,
        true,
    ));
    assert!(!is_parallel_safe_tool_for_target(
        "ssh__run_local_command",
        &json!({ "command": "git", "args": ["status"] }),
        "full",
        &roots,
        &registry,
        true,
    ));
}

#[test]
fn detects_paths_in_commands_and_declared_fields() {
    assert!(command_contains_path(r"D:\logs\radio.txt"));
    assert!(command_contains_path("%USERPROFILE%/Desktop/a.txt"));
    assert!(command_contains_path("--cwd=$env:USERPROFILE\\Desktop"));
    let paths = path_access_candidates(
        "run_local_command",
        &json!({ "command": "rg error D:\\workspace\\project" }),
        &[r"D:\workspace\project".to_string()],
    );
    assert!(paths
        .iter()
        .any(|path| path.to_string_lossy().contains("workspace")));
    let nested = path_access_candidates(
        "write_local_file",
        &json!({ "operations": [{ "target": "D:\\outside\\result.txt" }] }),
        &[],
    );
    assert!(nested
        .iter()
        .any(|path| path.to_string_lossy().contains("outside")));
}

#[test]
fn classifies_read_only_and_dangerous_commands() {
    assert!(is_read_only_tool(
        "capture_android_logcat",
        &json!({"terms":["ready"]})
    ));
    assert!(is_read_only_tool(
        "windows__capture_android_logcat",
        &json!({"serial":"qa"})
    ));
    assert!(is_read_only_command(
        &json!({ "command": "git", "args": ["status"] })
    ));
    assert!(is_dangerous_command(
        &json!({ "command": "git", "args": ["reset"] })
    ));
    assert!(!is_dangerous_shell(
        &json!({ "shell": "powershell", "command": "" })
    ));
}

#[test]
fn blocks_global_recursive_delete_variants_but_allows_scoped_cleanup() {
    assert!(is_forbidden_destructive_command_line("rm -rf *"));
    assert!(is_forbidden_destructive_command_line(
        "rm -r -f -- /tmp/project/*"
    ));
    assert!(is_forbidden_destructive_command_line(
        "Remove-Item -Recurse -Force *"
    ));
    assert!(is_forbidden_destructive_command_line("find . -delete"));
    assert!(is_forbidden_destructive_command_line(
        "find . -exec rm -rf {} +"
    ));
    assert!(is_forbidden_destructive_command_line("git clean -fdx"));
    assert!(!is_forbidden_destructive_command_line("rm -rf build"));
    assert!(!is_forbidden_destructive_command_line("git clean -n"));
}

#[test]
fn keeps_remote_paths_out_of_desktop_path_resolution() {
    let roots = vec!["/home/builder/aosp".to_string()];
    let inside = json!({ "path": "/home/builder/aosp/system/sepolicy" });
    let outside = json!({ "path": "/tmp/diagnostics.txt" });

    assert!(!path_access_requires_external_execution_for_target(
        "read_local_file",
        &inside,
        &roots,
        true,
    ));
    assert!(path_access_requires_external_execution_for_target(
        "read_local_file",
        &outside,
        &roots,
        true,
    ));
    assert_eq!(
        external_path_scope_key_for_target("read_local_file", &outside, &roots, true),
        Some("remote-external-path:/tmp/diagnostics.txt".to_string())
    );
}

#[test]
fn remote_permission_scope_and_display_use_remote_paths() {
    let roots = vec!["/home/builder/aosp".to_string()];
    let input = json!({
        "path": "/home/builder/aosp/vendor/policy.te",
        "cwd": "/home/builder/aosp"
    });
    let scope = permission_scope_key_for_target("write_local_file", &input, &roots, true);
    let enriched = enrich_permission_input_for_target(
        "write_local_file",
        &input,
        "ask",
        &roots,
        true,
        "ssh:connection-a",
    );

    assert!(scope.contains("/home/builder/aosp/vendor/policy.te"));
    assert_eq!(
        enriched["permission"]["target"],
        "/home/builder/aosp/vendor/policy.te"
    );
    assert_eq!(
        enriched["permission"]["workingDirectory"],
        "/home/builder/aosp"
    );
    assert_eq!(enriched["permission"]["environment"], "ssh:connection-a");
    assert!(enriched["permission"]["scope"]
        .as_str()
        .is_some_and(|scope| scope.starts_with("ssh:connection-a::")));
    assert!(!enriched.to_string().contains("D:\\"));
}

#[test]
fn remote_path_candidates_do_not_infer_a_local_working_directory() {
    let candidates = remote_path_access_candidates(
        "list_local_files",
        &json!({}),
        &["/home/builder/aosp".to_string()],
    );
    assert!(candidates.is_empty());
}

#[test]
fn remote_file_operations_scope_both_source_and_destination() {
    let roots = vec!["/home/builder/aosp".to_string()];
    let input = json!({
        "source": "/home/builder/aosp/system/a.te",
        "destination": "/tmp/a.te"
    });
    assert!(path_access_requires_external_execution_for_target(
        "copy_file",
        &input,
        &roots,
        true,
    ));
    let scope = external_path_scope_key_for_target("copy_file", &input, &roots, true)
        .expect("external destination should have a scope");
    assert!(scope.contains("/tmp/a.te"));
    let enriched = enrich_permission_input_for_target(
        "copy_file",
        &input,
        "ask",
        &roots,
        true,
        "ssh:connection-a",
    );
    assert_eq!(enriched["permission"]["target"], "/tmp/a.te");
}

#[test]
fn file_management_tools_are_mutating_and_not_parallel_safe() {
    let roots = vec!["/home/builder/aosp".to_string()];
    for name in ["create_directory", "copy_file", "move_file", "delete_file"] {
        let input = if name == "copy_file" || name == "move_file" {
            json!({
                "source": "/home/builder/aosp/a",
                "destination": "/home/builder/aosp/b"
            })
        } else {
            json!({ "path": "/home/builder/aosp/a" })
        };
        assert!(!is_read_only_tool(name, &input));
        assert!(path_access_requires_confirmation_for_target(
            "ask", name, &input, &roots, false, true
        ));
    }
}
