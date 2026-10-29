use super::temporary_root;
use crate::local_tools::wsl::execute_wsl_process;
use crate::{
    register_local_tools, AllowPermission, CommandOutputSink, LocalToolContext, ToolRegistry,
    WslRuntimeConfig,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::Arc;
#[tokio::test]
async fn project_owned_check_executes_an_arbitrary_tool_and_preserves_permission() {
    let root = temporary_root();
    fs::create_dir_all(&root).unwrap();
    let executable = std::env::current_exe().unwrap();
    let name = format!("project-check-fixture{}", std::env::consts::EXE_SUFFIX);
    fs::copy(executable, root.join(&name)).unwrap();
    fs::write(
        root.join(".agent-checks.json"),
        json!({"checks":{"custom-toolchain":{
            "command":format!("./{name}"),"args":["--list","--format","terse"]
        }}})
        .to_string(),
    )
    .unwrap();
    let mut registry = ToolRegistry::new();
    register_local_tools(
        &mut registry,
        LocalToolContext::new(
            vec![root.to_string_lossy().into_owned()],
            vec![],
            false,
            Arc::new(AtomicBool::new(false)),
            None,
        ),
    )
    .unwrap();
    assert!(
        registry
            .get("run_project_check")
            .unwrap()
            .requires_permission
    );
    let output = registry
        .execute(
            "run_project_check",
            json!({"kind":"custom-toolchain"}),
            &AllowPermission,
        )
        .await
        .unwrap();
    assert_eq!(output["success"], true);
    assert!(output["stdout"]
        .as_str()
        .unwrap()
        .contains("project_owned_check_executes_an_arbitrary_tool"));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and runs a real, test-owned Linux process"]
async fn real_wsl_cancellation_drains_linux_descendants() {
    real_wsl_descendant_cleanup("cancel").await;
    real_wsl_descendant_cleanup("timeout").await;
    real_wsl_descendant_cleanup("finish").await;
}

#[cfg(windows)]
async fn real_wsl_descendant_cleanup(mode: &str) {
    use std::sync::Mutex;
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select a test distro");
    let marker = format!(
        "dsh-cancel-{}",
        temporary_root().file_name().unwrap().to_string_lossy()
    );
    let cancellation = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(Mutex::new(String::new()));
    let listener_captured = captured.clone();
    let listener_cancel = cancellation.clone();
    let cancel_on_output = mode == "cancel";
    let sink = CommandOutputSink::new(
        Arc::new(move |event| {
            if event["stream"] == "stdout" {
                let mut text = listener_captured.lock().unwrap();
                text.push_str(event["text"].as_str().unwrap_or_default());
                if cancel_on_output && text.contains('\n') {
                    listener_cancel.store(true, Ordering::Relaxed);
                }
            }
        }),
        "test",
        "test",
        "run_local_shell",
    );
    let context = LocalToolContext::new(vec!["/tmp".into()], vec![], false, cancellation, None)
        .with_wsl(WslRuntimeConfig {
            distribution: distribution.clone(),
            root: "/tmp".into(),
        });
    let ending = if mode == "finish" {
        "exit 7"
    } else {
        "wait \"$worker\""
    };
    let result = execute_wsl_process(
        &context,
        "env",
        &[
            format!("ANDROID_DSH_WSL_TEST={marker}"),
            "sh".into(),
            "-c".into(),
            format!("trap '' HUP TERM; sleep 45 & worker=$!; printf '%s\\n' \"$worker\"; {ending}"),
        ],
        Path::new("/tmp"),
        if mode == "timeout" { 4_000 } else { 20_000 },
        Some(&sink),
    )
    .await;
    let pid: u32 = captured
        .lock()
        .unwrap()
        .trim()
        .parse()
        .expect("test child pid");
    // Verify the unique inherited marker before inspecting or cleaning up a PID.
    let probe = tokio::process::Command::new("wsl.exe")
            .args(["-d", &distribution, "--exec", "sh", "-c",
                "if test -r /proc/$1/environ && tr '\\0' '\\n' < /proc/$1/environ | grep -Fxq \"ANDROID_DSH_WSL_TEST=$2\"; then ps -o stat= -p \"$1\"; kill -KILL \"$1\"; fi",
                "dsh-test-cleanup", &pid.to_string(), &marker])
            .output().await.expect("WSL test probe");
    assert!(probe.status.success(), "test-owned process probe failed");
    let state = String::from_utf8_lossy(&probe.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "cancelled Linux descendant is still live: {state}"
    );
    match mode {
        "cancel" => assert!(matches!(result, Err(message) if message.contains("取消"))),
        "timeout" => assert!(matches!(result, Err(message) if message.contains("超时"))),
        "finish" => assert_eq!(
            result.ok().expect("command should finish").status.code(),
            Some(7)
        ),
        _ => unreachable!(),
    }
}
