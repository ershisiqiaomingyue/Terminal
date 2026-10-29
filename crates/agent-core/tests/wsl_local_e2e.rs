use agent_core::{
    register_local_tools, AllowPermission, LocalToolContext, ToolRegistry, WslRuntimeConfig,
};
use serde_json::{json, Value};
use std::fs;
use std::sync::{atomic::AtomicBool, Arc};

fn wsl_available() -> bool {
    std::process::Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

async fn call(registry: &ToolRegistry, name: &str, input: Value) -> Value {
    registry
        .execute(name, input, &AllowPermission)
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {error}"))
}

#[test]
fn wsl_reuses_structured_local_tools_when_available() {
    if !wsl_available() {
        return;
    }
    let distribution =
        std::env::var("ANDROID_TOOL_WSL_DISTRO").unwrap_or_else(|_| "Ubuntu-22.04".to_string());
    let root = format!("/tmp/android-tool-wsl-e2e-{}", std::process::id());
    let config = WslRuntimeConfig {
        distribution,
        root: "/tmp".to_string(),
    }
    .normalized()
    .expect("WSL configuration should be valid");
    let context = LocalToolContext::new(
        vec!["/tmp".to_string()],
        Vec::new(),
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .with_wsl(config.clone());
    let mut registry = ToolRegistry::new();
    register_local_tools(&mut registry, context).expect("register WSL tools");
    let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
    runtime.block_on(async {
        call(&registry, "create_directory", json!({ "path": root })).await;
        call(
            &registry,
            "write_local_file",
            json!({ "path": format!("{root}/hello.txt"), "content": "wsl-ready" }),
        )
        .await;
        let read = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/hello.txt") }),
        )
        .await;
        assert_eq!(read["content"], "wsl-ready");
        // Delete through the UNC share right after read, before any wsl.exe
        // process runs: the \\wsl.localhost share on some hosts loses
        // visibility of share-created files once wsl.exe commands have
        // executed (the distro may still hold them), which would make the
        // delete below non-deterministic.
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/hello.txt") }),
        )
        .await;
        let shell = call(
            &registry,
            "run_local_shell",
            json!({ "shell": "bash", "command": "printf shell-ready" }),
        )
        .await;
        assert_eq!(shell["stdout"], "shell-ready");
        let current_time = call(&registry, "get_current_time", json!({})).await;
        assert_eq!(current_time["target"], "wsl");
        assert_eq!(current_time["backend"], "wsl.exe");
        assert!(!current_time["local"]
            .as_str()
            .unwrap_or_default()
            .is_empty());
    });
    let unc_root = format!(
        r"\\wsl.localhost\{}\{}",
        config.distribution,
        root.trim_start_matches('/').replace('/', "\\")
    );
    let _ = fs::remove_dir_all(unc_root);
}
