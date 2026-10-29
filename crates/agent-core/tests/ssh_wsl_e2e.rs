use agent_core::{
    list_ssh_directories, query_ssh_host_key, query_ssh_status, register_ssh_tools,
    test_ssh_connection, AllowPermission, SshConnectionConfig, SshToolContext, ToolRegistry,
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn env_required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing {name} for the WSL SSH test"))
}

async fn call(registry: &ToolRegistry, name: &str, input: Value) -> Value {
    registry
        .execute(name, input, &AllowPermission)
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {error}"))
}

async fn call_error(registry: &ToolRegistry, name: &str, input: Value) -> String {
    registry
        .execute(name, input, &AllowPermission)
        .await
        .expect_err(&format!("{name} should fail"))
        .to_string()
}

#[test]
#[ignore = "requires a configured local WSL SSH server and explicit test environment variables"]
fn wsl_ssh_file_and_directory_round_trip() {
    let key_path = env_required("ANDROID_TOOL_SSH_E2E_KEY");
    let username = env_required("ANDROID_TOOL_SSH_E2E_USER");
    let remote_root = format!(
        "{}-run-{}",
        env_required("ANDROID_TOOL_SSH_E2E_ROOT").trim_end_matches('/'),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    );
    let host = std::env::var("ANDROID_TOOL_SSH_E2E_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("ANDROID_TOOL_SSH_E2E_PORT")
        .unwrap_or_else(|_| "22".into())
        .parse::<u16>()
        .expect("ANDROID_TOOL_SSH_E2E_PORT must be a valid port");
    let known_hosts_path = std::env::var("ANDROID_TOOL_SSH_E2E_KNOWN_HOSTS").ok();

    let local_root = std::env::temp_dir().join(format!(
        "android-tool-ssh-e2e-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    ));
    let downloads = local_root.join("downloads");
    fs::create_dir_all(&downloads).expect("create local transfer directories");
    let local_binary = local_root.join("payload.bin");
    let payload = (0u8..=255).collect::<Vec<_>>();
    fs::write(&local_binary, &payload).expect("create local binary payload");
    let local_directory = local_root.join("tree-source");
    fs::create_dir_all(local_directory.join("nested")).expect("create local directory payload");
    fs::write(local_directory.join("one.txt"), "directory upload").expect("write directory file");
    fs::write(
        local_directory.join("nested").join("two.txt"),
        "nested upload",
    )
    .expect("write nested directory file");

    let config = SshConnectionConfig {
        id: "wsl-e2e".to_string(),
        name: "WSL E2E".to_string(),
        host,
        port,
        username,
        private_key_path: Some(key_path),
        known_hosts_path,
        strict_host_key: false,
        default_root: None,
        reuse_connection: true,
        connect_timeout_secs: 10,
        operation_timeout_secs: 120,
        keepalive_interval_secs: 30,
    };
    test_ssh_connection(&config, None).expect("SSH command test should succeed");
    let host_key = query_ssh_host_key(&config).expect("SSH host key should be readable");
    assert!(host_key.fingerprint.starts_with("SHA256:"));
    let status = query_ssh_status(&config, None, "/tmp").expect("SSH status should succeed");
    assert!(status.hostname.is_some(), "status should include hostname");
    assert!(status.platform.is_some(), "status should include platform");
    assert!(
        status.cpu_count.is_some(),
        "status should include CPU count"
    );
    assert!(
        status.disk_total_bytes.is_some(),
        "status should include disk capacity"
    );
    let home_status = query_ssh_status(&config, None, "~").expect("home status should succeed");
    assert!(
        home_status.checked_path.starts_with("/home/"),
        "symbolic SSH root should resolve to the login home"
    );
    let context = SshToolContext::new(
        config.clone(),
        remote_root.clone(),
        None,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .with_local_transfer_roots(vec![local_root.to_string_lossy().into_owned()]);
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context).expect("register SSH tools");

    let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
    runtime.block_on(async {
        call(
            &registry,
            "create_directory",
            json!({ "path": remote_root }),
        )
        .await;
        call(
            &registry,
            "create_directory",
            json!({ "path": format!("{remote_root}/source") }),
        )
        .await;
        let directories = list_ssh_directories(&config, None, &remote_root)
            .expect("remote directory picker should list directories");
        assert!(directories.iter().any(|entry| entry.name == "source"));
        call(
            &registry,
            "write_local_file",
            json!({
                "path": format!("{remote_root}/source/readme.txt"),
                "content": "wsl ssh round trip"
            }),
        )
        .await;

        let listed = call(
            &registry,
            "list_local_files",
            json!({ "path": format!("{remote_root}/source") }),
        )
        .await;
        assert!(listed["entries"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["name"] == "readme.txt")));
        let searched = call(
            &registry,
            "search_local_files",
            json!({ "path": remote_root, "query": "readme" }),
        )
        .await;
        assert!(searched["results"]
            .as_array()
            .is_some_and(|results| !results.is_empty()));

        let info = call(
            &registry,
            "get_file_info",
            json!({ "path": format!("{remote_root}/source") }),
        )
        .await;
        assert_eq!(info["kind"], "directory");

        call(
            &registry,
            "copy_directory",
            json!({
                "source": format!("{remote_root}/source"),
                "destination": format!("{remote_root}/copy")
            }),
        )
        .await;
        call(
            &registry,
            "move_directory",
            json!({
                "source": format!("{remote_root}/copy"),
                "destination": format!("{remote_root}/moved")
            }),
        )
        .await;

        call(
            &registry,
            "upload_file",
            json!({
                "local_path": local_binary.to_string_lossy(),
                "remote_path": format!("{remote_root}/payload.bin")
            }),
        )
        .await;
        call(
            &registry,
            "download_file",
            json!({
                "remote_path": format!("{remote_root}/payload.bin"),
                "local_path": downloads.to_string_lossy()
            }),
        )
        .await;
        assert_eq!(
            fs::read(downloads.join("payload.bin")).expect("read downloaded payload"),
            payload
        );
        call(
            &registry,
            "upload_directory",
            json!({
                "local_path": local_directory.to_string_lossy(),
                "remote_path": format!("{remote_root}/tree-copy")
            }),
        )
        .await;
        call(
            &registry,
            "set_file_permissions",
            json!({
                "path": format!("{remote_root}/tree-copy/one.txt"),
                "mode": "640"
            }),
        )
        .await;
        let permission_info = call(
            &registry,
            "get_file_info",
            json!({ "path": format!("{remote_root}/tree-copy/one.txt") }),
        )
        .await;
        assert_eq!(permission_info["permissions"].as_u64(), Some(0o640));
        let interactive = call(
            &registry,
            "run_interactive_shell",
            json!({ "shell": "sh", "command": "printf interactive-ok" }),
        )
        .await;
        assert_eq!(interactive["stdout"], "interactive-ok");
        let shell = call(
            &registry,
            "run_local_shell",
            json!({ "shell": "sh", "command": "printf shell-ok" }),
        )
        .await;
        assert_eq!(shell["stdout"], "shell-ok");

        // A symlink inside the selected workspace must never become an
        // implicit read/copy/download path for the SFTP implementation.
        let escape_link = format!("{remote_root}/escape-link");
        call(
            &registry,
            "run_local_command",
            json!({
                "command": "ln",
                "args": ["-s", "/etc/passwd", &escape_link]
            }),
        )
        .await;
        let search = call(
            &registry,
            "search_local_files",
            json!({ "path": remote_root, "query": "escape-link" }),
        )
        .await;
        assert!(search["results"]
            .as_array()
            .is_some_and(|items| items.is_empty()));
        let copy_error = call_error(
            &registry,
            "copy_file",
            json!({
                "source": &escape_link,
                "destination": format!("{remote_root}/escape-copy")
            }),
        )
        .await;
        assert!(copy_error.contains("普通文件") || copy_error.contains("符号链接"));
        let download_error = call_error(
            &registry,
            "download_file",
            json!({
                "remote_path": &escape_link,
                "local_path": downloads.to_string_lossy()
            }),
        )
        .await;
        assert!(download_error.contains("普通文件") || download_error.contains("符号链接"));
        call(
            &registry,
            "download_directory",
            json!({
                "remote_path": format!("{remote_root}/tree-copy"),
                "local_path": downloads.to_string_lossy()
            }),
        )
        .await;
        assert_eq!(
            fs::read_to_string(downloads.join("tree-copy").join("nested").join("two.txt"))
                .expect("read downloaded nested file"),
            "nested upload"
        );

        for path in [
            format!("{remote_root}/source/readme.txt"),
            format!("{remote_root}/moved/readme.txt"),
            format!("{remote_root}/payload.bin"),
            format!("{remote_root}/tree-copy/one.txt"),
            format!("{remote_root}/tree-copy/nested/two.txt"),
            escape_link,
        ] {
            call(&registry, "delete_file", json!({ "path": path })).await;
        }
        call(
            &registry,
            "run_local_command",
            json!({
                "command": "rmdir",
                "args": [
                    format!("{remote_root}/source"),
                    format!("{remote_root}/moved"),
                    format!("{remote_root}/tree-copy/nested"),
                    format!("{remote_root}/tree-copy"),
                    remote_root
                ]
            }),
        )
        .await;
    });

    fs::remove_dir_all(PathBuf::from(local_root)).expect("remove local transfer directory");
}

#[test]
#[ignore = "requires an unencrypted Ed25519 OpenSSH key and a configured local WSL SSH server"]
fn openssh_ed25519_fallback_covers_connection_status_and_basic_tools() {
    let key_path = env_required("ANDROID_TOOL_SSH_E2E_KEY");
    let username = env_required("ANDROID_TOOL_SSH_E2E_USER");
    let host = std::env::var("ANDROID_TOOL_SSH_E2E_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("ANDROID_TOOL_SSH_E2E_PORT")
        .unwrap_or_else(|_| "2222".into())
        .parse::<u16>()
        .expect("ANDROID_TOOL_SSH_E2E_PORT must be a valid port");
    let root_base = std::env::var("ANDROID_TOOL_SSH_E2E_ROOT")
        .unwrap_or_else(|_| "/tmp/android-tool-ssh-ed25519-e2e".into());
    let root = format!(
        "{}-run-{}",
        root_base.trim_end_matches('/'),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    );
    let local_root = std::env::temp_dir().join(format!(
        "android-tool-ssh-openssh-transfer-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    ));
    let local_source = local_root.join("tree-source");
    let local_downloads = local_root.join("downloads");
    fs::create_dir_all(local_source.join("nested")).expect("create fallback upload directory");
    fs::create_dir_all(&local_downloads).expect("create fallback download directory");
    fs::write(local_source.join("one.txt"), "upload-fallback-one")
        .expect("write fallback upload file");
    fs::write(
        local_source.join("nested").join("two.txt"),
        "upload-fallback-two",
    )
    .expect("write fallback nested upload file");
    let config = SshConnectionConfig {
        id: "openssh-ed25519-e2e".to_string(),
        name: "OpenSSH Ed25519 E2E".to_string(),
        host,
        port,
        username,
        private_key_path: Some(key_path),
        known_hosts_path: None,
        strict_host_key: false,
        default_root: Some(root.clone()),
        reuse_connection: true,
        connect_timeout_secs: 10,
        operation_timeout_secs: 120,
        keepalive_interval_secs: 30,
    };
    test_ssh_connection(&config, None).expect("OpenSSH fallback connection should succeed");
    let status =
        query_ssh_status(&config, None, "/tmp").expect("OpenSSH fallback status should succeed");
    assert!(status.hostname.is_some());
    let directories = list_ssh_directories(&config, None, "/tmp")
        .expect("OpenSSH fallback directory listing should succeed");
    assert!(directories.iter().all(|entry| !entry.name.is_empty()));

    let cancellation = Arc::new(AtomicBool::new(false));
    let context = SshToolContext::new(config.clone(), root.clone(), None, cancellation, None)
        .with_local_transfer_roots(vec![local_root.to_string_lossy().into_owned()]);
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context).expect("register fallback tools");
    let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
    runtime.block_on(async {
        call(&registry, "create_directory", json!({ "path": root })).await;
        call(
            &registry,
            "write_local_file",
            json!({ "path": format!("{root}/fallback.txt"), "content": "fallback-ok" }),
        )
        .await;
        let read = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/fallback.txt") }),
        )
        .await;
        assert_eq!(read["content"], "fallback-ok");
        call(
            &registry,
            "create_directory",
            json!({ "path": format!("{root}/source") }),
        )
        .await;
        call(
            &registry,
            "write_local_file",
            json!({
                "path": format!("{root}/source/directory.txt"),
                "content": "directory-fallback-ok"
            }),
        )
        .await;
        call(
            &registry,
            "copy_directory",
            json!({
                "source": format!("{root}/source"),
                "destination": format!("{root}/copy")
            }),
        )
        .await;
        let copied = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/copy/directory.txt") }),
        )
        .await;
        assert_eq!(copied["content"], "directory-fallback-ok");
        call(
            &registry,
            "move_directory",
            json!({
                "source": format!("{root}/copy"),
                "destination": format!("{root}/moved")
            }),
        )
        .await;
        let moved = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/moved/directory.txt") }),
        )
        .await;
        assert_eq!(moved["content"], "directory-fallback-ok");
        call(
            &registry,
            "apply_patch",
            json!({
                "path": format!("{root}/moved/directory.txt"),
                "old_text": "directory-fallback-ok",
                "new_text": "directory-fallback-patched"
            }),
        )
        .await;
        let patched = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/moved/directory.txt") }),
        )
        .await;
        assert_eq!(patched["content"], "directory-fallback-patched");
        call(
            &registry,
            "upload_directory",
            json!({
                "local_path": local_source.to_string_lossy(),
                "remote_path": format!("{root}/uploaded")
            }),
        )
        .await;
        let uploaded = call(
            &registry,
            "read_local_file",
            json!({ "path": format!("{root}/uploaded/nested/two.txt") }),
        )
        .await;
        assert_eq!(uploaded["content"], "upload-fallback-two");
        call(
            &registry,
            "download_directory",
            json!({
                "remote_path": format!("{root}/uploaded"),
                "local_path": local_downloads.to_string_lossy()
            }),
        )
        .await;
        assert_eq!(
            fs::read_to_string(local_downloads.join("uploaded").join("nested").join("two.txt"))
                .expect("read fallback downloaded file"),
            "upload-fallback-two"
        );
        call(
            &registry,
            "write_local_file",
            json!({
                "path": format!("{root}/gradlew"),
                "content": "#!/bin/sh\nprintf project-check"
            }),
        )
        .await;
        let project_check = call(
            &registry,
            "run_project_check",
            json!({ "kind": "check" }),
        )
        .await;
        assert_eq!(project_check["detected"], "gradlew");
        assert!(project_check["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("project-check")));
        let command = call(
            &registry,
            "run_local_command",
            json!({ "command": "printf", "args": ["%s", "command-ok"] }),
        )
        .await;
        assert_eq!(command["stdout"], "command-ok");
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/fallback.txt") }),
        )
        .await;
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/source/directory.txt") }),
        )
        .await;
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/moved/directory.txt") }),
        )
        .await;
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/uploaded/one.txt") }),
        )
        .await;
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/uploaded/nested/two.txt") }),
        )
        .await;
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/gradlew") }),
        )
        .await;
        call(
            &registry,
            "run_local_command",
            json!({ "command": "rmdir", "args": [format!("{root}/source"), format!("{root}/moved"), format!("{root}/uploaded/nested"), format!("{root}/uploaded")] }),
        )
        .await;

        call(
            &registry,
            "run_local_command",
            json!({
                "command": "ln",
                "args": ["-s", "/etc/passwd", format!("{root}/escape-link")]
            }),
        )
        .await;
        let escaped = registry
            .execute(
                "read_local_file",
                json!({ "path": format!("{root}/escape-link") }),
                &AllowPermission,
            )
            .await
            .expect_err("OpenSSH fallback must reject a symlink escape");
        assert!(escaped.to_string().contains("安全边界"));
        call(
            &registry,
            "delete_file",
            json!({ "path": format!("{root}/escape-link") }),
        )
        .await;
        call(
            &registry,
            "run_local_command",
            json!({ "command": "rmdir", "args": [root] }),
        )
        .await;

        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel_context = SshToolContext::new(
            config,
            "/tmp",
            None,
            cancellation.clone(),
            None,
        );
        let mut cancel_registry = ToolRegistry::new();
        register_ssh_tools(&mut cancel_registry, cancel_context)
            .expect("register fallback cancellation tools");
        let cancel_registry = Arc::new(cancel_registry);
        let cancel_task_registry = Arc::clone(&cancel_registry);
        let cancel_task = tokio::spawn(async move {
            cancel_task_registry
                .execute(
                    "run_local_command",
                    json!({ "command": "sleep", "args": ["30"] }),
                    &AllowPermission,
                )
                .await
        });
        std::thread::sleep(Duration::from_millis(250));
        cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
        let cancelled = cancel_task
            .await
            .expect("cancellation task should join")
            .expect_err("long OpenSSH command should be cancelled");
        assert!(cancelled.to_string().contains("请求已取消"));
    });
    fs::remove_dir_all(local_root).expect("remove fallback transfer directory");
}
