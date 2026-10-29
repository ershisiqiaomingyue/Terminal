use super::*;

#[test]
fn ssh_background_does_not_wait_for_shared_operation_lock() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config: SshConnectionConfig = serde_json::from_value(json!({
        "id":"background-isolation", "name":"test", "host":"127.0.0.1",
        "port":port, "username":"test", "connectTimeoutSecs":1
    }))
    .unwrap();
    let lock = connection_operation_lock(&config);
    let guard = lock.lock().unwrap();
    let context = SshToolContext::new(
        config,
        "/workspace",
        None,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .for_background();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = run_blocking(&context, |_, _| Ok(()));
        let _ = tx.send(result);
    });
    // Windows may spend several seconds rejecting the closed loopback port,
    // including the transport's bounded retries. Keep the shared lock held
    // throughout, so this still detects accidentally waiting for that lock.
    let result = rx.recv_timeout(Duration::from_secs(10));
    drop(guard);
    worker.join().unwrap();
    // The dedicated connection fails promptly on our closed loopback port,
    // even while a normal tool owns the shared connection's operation lock.
    assert!(result
        .expect("background must bypass the busy shared connection")
        .is_err());
}

#[test]
fn ssh_host_key_check_defaults_off_and_preserves_explicit_setting() {
    let config = json!({ "id": "default-check", "name": "test", "host": "localhost", "port": 22, "username": "test" });
    let default: SshConnectionConfig = serde_json::from_value(config.clone()).unwrap();
    assert!(!default.strict_host_key);
    let mut explicit = config;
    explicit["strictHostKey"] = json!(true);
    let strict: SshConnectionConfig = serde_json::from_value(explicit).unwrap();
    assert!(strict.strict_host_key);
}

#[test]
fn ssh_status_probe_waits_for_shared_operation_lane() {
    let config: SshConnectionConfig = serde_json::from_value(json!({
        "id": "status-lock-probe",
        "name": "test",
        "host": "127.0.0.1",
        "port": 1,
        "username": "test",
        "connectTimeoutSecs": 1
    }))
    .unwrap();
    let lock = connection_operation_lock(&config);
    let guard = lock.lock().unwrap();
    let (result_tx, result_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = result_tx.send(query_ssh_status(&config, None, "/"));
    });

    assert!(
        result_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "status probe bypassed the shared SSH operation lane"
    );
    drop(guard);
    assert!(result_rx.recv_timeout(Duration::from_secs(15)).is_ok());
    worker.join().unwrap();
}

#[test]
fn ssh_connection_retry_skips_authentication_and_configuration_errors() {
    assert!(super::session::retryable_connection_error(
        "无法连接 SSH 主机 127.0.0.1:22：连接被拒绝"
    ));
    assert!(super::session::retryable_connection_error(
        "SSH 操作失败: transport read"
    ));
    assert!(!super::session::retryable_connection_error(
        "SSH 密码认证失败"
    ));
    assert!(!super::session::retryable_connection_error(
        "SSH 主机密钥与 known_hosts 不匹配"
    ));
}

#[test]
fn cancelled_ssh_waiter_does_not_wait_for_busy_connection_or_connect() {
    let config: SshConnectionConfig = serde_json::from_value(json!({
        "id": "cancelled-lock-waiter", "name": "test", "host": "127.0.0.1",
        "port": 1, "username": "test"
    }))
    .unwrap();
    let lock = connection_operation_lock(&config);
    let guard = lock.lock().unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let waiter_cancel = cancellation.clone();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let sink = CommandOutputSink::new(
        Arc::new(move |event| {
            if event["progress"]["stage"] == "waiting" {
                let _ = waiting_tx.send(());
            }
        }),
        "test",
        "test",
        "run_local_shell",
    );
    let context = SshToolContext::new(config, "/tmp", None, waiter_cancel, Some(sink));
    let (result_tx, result_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let result = run_blocking(&context, |_session, _context| -> Result<(), String> {
            panic!("cancelled waiter must not execute")
        });
        let _ = result_tx.send(result);
    });
    let waiting = waiting_rx.recv_timeout(Duration::from_secs(2));
    cancellation.store(true, Ordering::Relaxed);
    let result = result_rx.recv_timeout(Duration::from_secs(2));
    drop(guard);
    waiter.join().unwrap();
    assert!(waiting.is_ok(), "waiter never reported contention");
    assert!(matches!(result, Ok(Err(message)) if message.contains("取消")));
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL; drops only isolated loopback SSH transports"]
async fn real_ssh_network_loss_never_replays_the_command_and_recovers() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    for (fallback, pty, mode) in [false, true].into_iter().flat_map(|fallback| {
        [false, true].into_iter().flat_map(move |pty| {
            ["reconnect", "outage", "host-key"]
                .into_iter()
                .map(move |mode| (fallback, pty, mode))
        })
    }) {
        let proxy = Arc::new(ssh_fixture::SshFaultProxy::start(&fixture));
        let config: SshConnectionConfig = serde_json::from_value(proxy.config.clone()).unwrap();
        if fallback {
            remember_openssh_transport(&config);
        }
        test_ssh_connection(&config, None).unwrap();
        assert_eq!(uses_openssh_fallback(&config), fallback);
        let known_hosts_path = PathBuf::from(config.known_hosts_path.as_ref().unwrap());
        let original_known_hosts = std::fs::read(&known_hosts_path).unwrap();
        let listener_known_hosts = known_hosts_path.clone();
        let execution_counter = format!("{}/{}-executions", fixture.remote_root, config.id);
        let captured = Arc::new(Mutex::new(String::new()));
        let listener_text = captured.clone();
        let listener_proxy = proxy.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| {
                if event["phase"] == "chunk" && event["stream"] == "stdout" {
                    let mut text = listener_text.lock().unwrap();
                    text.push_str(event["text"].as_str().unwrap_or_default());
                    if text.contains('\n') {
                        if mode == "host-key" {
                            std::fs::write(&listener_known_hosts, "").unwrap();
                        }
                        listener_proxy.drop_connections(mode == "outage");
                    }
                }
            }),
            "network-test",
            "network-test",
            "run_local_shell",
        );
        let context = SshToolContext::new(
            config.clone(),
            &fixture.remote_root,
            None,
            Arc::new(AtomicBool::new(false)),
            Some(sink),
        );
        let mut registry = ToolRegistry::new();
        register_ssh_tools(&mut registry, context).unwrap();
        let started = Instant::now();
        let result = registry.execute(if pty { "run_interactive_shell" } else { "run_local_shell" },
                json!({"shell":"bash", "cwd":fixture.remote_root, "timeout_ms":15000,
                    "command":format!("printf 'executed\\n' >> {}; export ANDROID_DSH_SSH_CHILD={}; trap '' HUP TERM; sleep 45 & worker=$!; printf '%s\\n' \"$worker\"; wait \"$worker\"", shell_escape(&execution_counter), config.id)}),
                &crate::AllowPermission).await;
        let text = captured.lock().unwrap().clone();
        let pid: u32 = text
            .lines()
            .next()
            .unwrap_or_else(|| panic!("network fixture produced no PID: {result:?}"))
            .trim()
            .parse()
            .unwrap();
        let state = fixture.probe_and_clean_test_process(pid, &config.id);
        let elapsed = started.elapsed();
        let error = match result {
            Err(crate::ToolExecutionError::Handler(error)) => error,
            Err(error) => panic!("unexpected network fixture error: {error}"),
            Ok(value) => {
                assert_eq!(
                    value["success"], false,
                    "transport loss cannot report success"
                );
                format!("transport failure: {}", value["stderr"])
            }
        };
        std::fs::write(&known_hosts_path, original_known_hosts).unwrap();
        proxy.restore();
        let context = SshToolContext::new(
            config.clone(),
            &fixture.remote_root,
            None,
            Arc::new(AtomicBool::new(false)),
            None,
        );
        let counter = execution_counter.clone();
        let recovered = await_remote(spawn_remote(context, move |session, context| {
            let count = execute_remote_control(
                &session,
                &format!("cat {path}; rm -f -- {path}", path = shell_escape(&counter)),
            )?;
            assert_eq!(
                count, "executed",
                "original command must execute exactly once"
            );
            let channel = retry_nonblocking("test channel", Duration::from_secs(5), || {
                session.channel_session()
            })?;
            execute_remote_command(
                channel,
                &session,
                "printf recovered",
                context,
                None,
                Duration::from_secs(5),
                None,
            )
        }))
        .await
        .unwrap();
        assert_eq!(recovered["stdout"], "recovered");
        disconnect_ssh_connection(&config);
        assert!(
            elapsed < Duration::from_secs(25),
            "network-loss cleanup stalled: {mode}, fallback={fallback}, pty={pty}: {error}"
        );
        if mode != "reconnect" {
            assert!(
                error.starts_with(crate::process::PROCESS_CLEANUP_ERROR_PREFIX),
                "{mode}: {error}"
            );
        } else if !pty {
            assert!(
                !error.starts_with(crate::process::PROCESS_CLEANUP_ERROR_PREFIX),
                "{error}"
            );
            assert!(
                state.trim().is_empty() || state.trim().starts_with('Z'),
                "remote child survived reachable cleanup: {state}"
            );
        } else if !state.trim().is_empty() && !state.trim().starts_with('Z') {
            assert!(
                error.starts_with(crate::process::PROCESS_CLEANUP_ERROR_PREFIX),
                "live PTY child must report uncertain cleanup: {error}"
            );
        }
        assert_eq!(
            text.lines().count(),
            1,
            "original command must not be replayed"
        );
    }
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an installed sshd; starts an isolated loopback server"]
async fn real_ssh_cancellation_drains_remote_descendants() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    for fallback in [false, true] {
        let mut config: SshConnectionConfig =
            serde_json::from_value(fixture.config.clone()).unwrap();
        config
            .id
            .push_str(if fallback { "-openssh" } else { "-libssh2" });
        assert!(openssh_private_key_path(&config).is_some());
        assert!(
            !uses_openssh_fallback(&config),
            "PEM keys must remain eligible for libssh2"
        );
        if fallback {
            remember_openssh_transport(&config);
        }
        test_ssh_connection(&config, None).unwrap();
        assert_eq!(
            uses_openssh_fallback(&config),
            fallback,
            "fixture did not select the requested transport"
        );
        for mode in [
            "cancel",
            "timeout",
            "finish",
            "flood-cancel",
            "flood-timeout",
        ] {
            real_ssh_descendant_cleanup(&fixture, &config, mode).await;
        }
        if fallback {
            use sha2::{Digest, Sha256};
            let input = "共享输入，保留换行和标准输入。\n"
                .repeat(20_000)
                .into_bytes();
            let output = run_openssh_command_with_cancellation(
                &config,
                "sha256sum",
                Some(&input),
                Duration::from_secs(10),
                Some(&Arc::new(AtomicBool::new(false))),
                None,
                None,
            )
            .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .next()
                    .unwrap(),
                format!("{:x}", Sha256::digest(&input))
            );
        }
        disconnect_ssh_connection(&config);
    }
}

#[cfg(windows)]
async fn real_ssh_descendant_cleanup(
    fixture: &ssh_fixture::SshFixture,
    config: &SshConnectionConfig,
    mode: &str,
) {
    let cancellation = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(Mutex::new(String::new()));
    let listener_captured = captured.clone();
    let listener_cancel = cancellation.clone();
    let cancel_on_output = matches!(mode, "cancel" | "flood-cancel");
    let flood = mode.starts_with("flood-");
    let output_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener_bytes = output_bytes.clone();
    let sink = CommandOutputSink::new(
        Arc::new(move |event| {
            if event["phase"] == "chunk" {
                let chunk = event["text"].as_str().unwrap_or_default();
                let bytes = listener_bytes.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                let mut text = listener_captured.lock().unwrap();
                if event["stream"] == "stdout" {
                    let remaining = 256usize.saturating_sub(text.len());
                    text.extend(chunk.chars().take(remaining));
                }
                if cancel_on_output && text.contains('\n') && (!flood || bytes >= 64 * 1024) {
                    listener_cancel.store(true, Ordering::Relaxed);
                }
            }
        }),
        "test",
        "test",
        "run_local_shell",
    );
    let context = SshToolContext::new(
        config.clone(),
        &fixture.remote_root,
        None,
        cancellation,
        Some(sink),
    );
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context).unwrap();
    let ending = if mode == "finish" {
        "exit 7"
    } else {
        "wait \"$worker\""
    };
    let noise = if flood {
        "yes 'continuous stdout' & yes 'continuous stderr' >&2 &"
    } else {
        ""
    };
    let started = Instant::now();
    let result = registry.execute("run_local_shell", json!({"shell":"bash", "cwd":fixture.remote_root,
            "command":format!("export ANDROID_DSH_SSH_CHILD={}; trap '' HUP TERM; sleep 45 & worker=$!; printf '%s\\n' \"$worker\"; {noise} {ending}", config.id),
            "timeout_ms":if mode.ends_with("timeout") { 2000 } else { 20000 }}), &crate::AllowPermission).await;
    let pid: u32 = captured
        .lock()
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse()
        .expect("remote test child pid");
    let state = fixture.probe_and_clean_test_process(pid, &config.id);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "SSH stop was starved by output: {mode}"
    );
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "cancelled remote descendant is still live: {state}"
    );
    if flood {
        assert!(
            output_bytes.load(Ordering::Relaxed) >= 64 * 1024,
            "stress fixture did not reach sustained output: {mode}, bytes={}, result={result:?}",
            output_bytes.load(Ordering::Relaxed)
        );
    }
    match mode {
        "cancel" | "flood-cancel" => assert!(
            matches!(result, Err(crate::ToolExecutionError::Handler(message)) if message.contains("取消"))
        ),
        "timeout" | "flood-timeout" => assert!(
            matches!(result, Err(crate::ToolExecutionError::Handler(message)) if message.contains("超时"))
        ),
        "finish" => assert_eq!(result.unwrap()["exitCode"], 7),
        _ => unreachable!(),
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL; tests strict-host-key OpenSSH fallback against Ed25519-only sshd"]
fn real_ssh_ed25519_host_fallback_preserves_key_verification() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start_with_host_key(&distribution, "ed25519");
    let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    assert!(config.strict_host_key);
    test_ssh_connection(&config, None).unwrap();
    assert!(uses_openssh_fallback(&config));
    std::fs::write(config.known_hosts_path.as_ref().unwrap(), "").unwrap();
    let error = test_ssh_connection(&config, None).unwrap_err();
    assert!(
        error.contains("Host key verification failed") || error.contains("strict checking"),
        "{error}"
    );
    disconnect_ssh_connection(&config);
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and installed sshd; verifies real PTY terminal ownership"]
async fn real_ssh_pty_preserves_terminal_and_drains_owned_session() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    for fallback in [false, true] {
        let mut config: SshConnectionConfig =
            serde_json::from_value(fixture.config.clone()).unwrap();
        config.id.push_str(if fallback {
            "-pty-openssh"
        } else {
            "-pty-libssh2"
        });
        if fallback {
            remember_openssh_transport(&config);
        }
        let mut registry = ToolRegistry::new();
        register_ssh_tools(
            &mut registry,
            SshToolContext::new(
                config.clone(),
                &fixture.remote_root,
                None,
                Arc::new(AtomicBool::new(false)),
                None,
            ),
        )
        .unwrap();
        let result = registry.execute("run_interactive_shell", json!({"shell":"bash", "cwd":fixture.remote_root,
            "command":r#"test -t 0 && test -t 1 && stty size </dev/tty && IFS= read -r value && printf 'PTY_INPUT:%s\n' "$value""#,
            "input":"终端输入，中文与引号'\"正常。\n", "timeout_ms":10000}), &crate::AllowPermission).await.unwrap();
        assert_eq!(result["exitCode"], 0, "{result}");
        assert!(result["stdout"].as_str().unwrap().contains("40 120"));
        assert!(result["stdout"]
            .as_str()
            .unwrap()
            .contains("PTY_INPUT:终端输入，中文与引号'\"正常。"));
        for mode in [
            "cancel",
            "timeout",
            "finish",
            "job-cancel",
            "job-timeout",
            "job-finish",
            "flood-cancel",
            "flood-timeout",
        ] {
            real_ssh_pty_cleanup(&fixture, &config, mode).await;
        }
        let result = registry
            .execute(
                "run_interactive_shell",
                json!({"shell":"bash", "cwd":fixture.remote_root,
            "command":r#"head -c 8500000 /dev/zero | tr '\0' x; exit 9"#, "timeout_ms":20000}),
                &crate::AllowPermission,
            )
            .await
            .unwrap();
        assert_eq!(result["exitCode"], 9);
        assert_eq!(result["outputTruncated"], true);
        assert_eq!(
            result["stdout"].as_str().unwrap().len(),
            MAX_REMOTE_COMMAND_OUTPUT_BYTES
        );
        disconnect_ssh_connection(&config);
    }
}

#[cfg(windows)]
async fn real_ssh_pty_cleanup(
    fixture: &ssh_fixture::SshFixture,
    config: &SshConnectionConfig,
    mode: &str,
) {
    let cancellation = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(Mutex::new(String::new()));
    let listener_text = captured.clone();
    let listener_cancel = cancellation.clone();
    let cancel_on_output = mode.ends_with("cancel");
    let flood = mode.starts_with("flood-");
    let output_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener_bytes = output_bytes.clone();
    let sink = CommandOutputSink::new(
        Arc::new(move |event| {
            if event["phase"] == "chunk" {
                let chunk = event["text"].as_str().unwrap_or_default();
                let bytes = listener_bytes.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                let mut text = listener_text.lock().unwrap();
                let remaining = 256usize.saturating_sub(text.len());
                text.extend(chunk.chars().take(remaining));
                if cancel_on_output && text.contains('\n') && (!flood || bytes >= 64 * 1024) {
                    listener_cancel.store(true, Ordering::Relaxed);
                }
            }
        }),
        "pty-test",
        "pty-test",
        "run_interactive_shell",
    );
    let mut registry = ToolRegistry::new();
    register_ssh_tools(
        &mut registry,
        SshToolContext::new(
            config.clone(),
            &fixture.remote_root,
            None,
            cancellation,
            Some(sink),
        ),
    )
    .unwrap();
    let ending = if mode.ends_with("finish") {
        "exit 7"
    } else {
        "wait \"$worker\""
    };
    let noise = if flood {
        "yes 'pty stdout' & yes 'pty stderr' >&2 &"
    } else {
        ""
    };
    let mut command = format!(
        r#"export ANDROID_DSH_SSH_CHILD={}; trap '' HUP TERM; sleep 45 & worker=$!; printf 'owned-pty-pid=%s\n' "$worker"; {noise} {ending}"#,
        config.id
    );
    if mode.starts_with("job-") {
        command = format!("bash --noprofile --norc -m -c {}", shell_escape(&command));
    }
    let started = Instant::now();
    let result = registry
        .execute(
            "run_interactive_shell",
            json!({"shell":"bash", "cwd":fixture.remote_root,
            "command":command, "timeout_ms":if mode.ends_with("timeout") {2000} else {20000}}),
            &crate::AllowPermission,
        )
        .await;
    let text = captured.lock().unwrap().clone();
    let pid = text
        .lines()
        .find_map(|line| {
            line.strip_prefix("owned-pty-pid=")
                .and_then(|pid| pid.trim().parse::<u32>().ok())
        })
        .unwrap_or_else(|| panic!("missing PTY pid: {mode}: {text:?}, {result:?}"));
    let state = fixture.probe_and_clean_test_process(pid, &config.id);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "PTY descendant is still live: {mode}: {state}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "PTY stop was starved: {mode}"
    );
    if flood {
        assert!(output_bytes.load(Ordering::Relaxed) >= 64 * 1024);
    }
    if mode.ends_with("finish") {
        assert_eq!(result.unwrap()["exitCode"], 7);
    } else if mode.ends_with("cancel") {
        assert!(
            matches!(result, Err(crate::ToolExecutionError::Handler(message)) if message.contains("取消"))
        );
    } else {
        assert!(
            matches!(result, Err(crate::ToolExecutionError::Handler(message)) if message.contains("超时"))
        );
    }
}

#[test]
fn remote_search_command_bounds_work_and_never_follows_links() {
    let command = remote_search_command("/root/it's here", "needle'; touch /tmp/nope", true, 20);
    assert!(command.contains("timeout --signal=TERM --kill-after=2s 20s"));
    assert!(command.contains(".repo"));
    assert!(command.contains("out"));
    assert!(command.contains("--max-filesize 2M"));
    assert!(command.contains("257"));
    assert!(!command.contains("--iglob"));
    assert!(!command.contains("-iname"));
    assert!(!command.contains("--follow"));
    assert!(!command.contains("find -L"));
    assert!(!command.contains("grep -r"));
}

#[test]
fn remote_search_reports_exact_cap_without_false_truncation() {
    let rows = (0..MAX_REMOTE_SEARCH_RESULTS)
        .map(|index| format!("/root/{index}\n"))
        .collect::<String>();
    let exact = remote_search_result(
        "/root",
        "x",
        &format!("{rows}{REMOTE_SEARCH_STATUS_PREFIX}0\n"),
        "test",
        false,
    );
    assert_eq!(exact["truncated"], false);
    assert_eq!(exact["partial"], false);
    let extra = remote_search_result(
        "/root",
        "x",
        &format!("{rows}/root/extra\n{REMOTE_SEARCH_STATUS_PREFIX}0\n"),
        "test",
        false,
    );
    assert_eq!(
        extra["results"].as_array().unwrap().len(),
        MAX_REMOTE_SEARCH_RESULTS
    );
    assert_eq!(extra["truncated"], true);
    assert_eq!(extra["partial"], true);
}

#[test]
fn remote_search_preserves_partial_timeout_results_and_spaces() {
    let result = remote_search_result(
        "/root",
        "x",
        &format!("/root/a file \n/root/a file \n{REMOTE_SEARCH_STATUS_PREFIX}124\n"),
        "test",
        false,
    );
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["path"], "/root/a file ");
    assert_eq!(result["timedOut"], true);
    assert_eq!(result["partial"], true);
    assert_eq!(result["truncated"], false);
    assert!(result["nextStep"].as_str().is_some());
    let missing_marker = remote_search_result("/root", "x", "/root/x\n", "test", false);
    assert_eq!(missing_marker["partial"], true);
}

#[test]
fn remote_search_reports_typed_directories_and_scanner_errors() {
    let result = remote_search_result("/root", "needle", &format!("d\t/root/needle\nf\t/root/needle/test.java\n{REMOTE_SEARCH_ERROR_PREFIX}find:1\n{REMOTE_SEARCH_STATUS_PREFIX}0\n"), "test", false);
    assert_eq!(result["results"][0]["kind"], "directory");
    assert_eq!(result["results"][1]["kind"], "file");
    assert_eq!(result["scanErrors"][0], "find:1");
    assert_eq!(result["partial"], true);
    assert_eq!(result["timedOut"], false);
    assert_eq!(result["results"].as_array().unwrap().len(), 2);
    assert!(crate::owned_process::DRAIN.contains("kill -KILL"));
}

#[test]
fn remote_directory_entries_use_posix_parent_not_host_join() {
    #[cfg(target_os = "windows")]
    assert_eq!(
        remote_directory_entry_path("/linux/tree", Path::new("/linux/tree\\entry")),
        "/linux/tree/entry"
    );
    assert_eq!(
        remote_directory_entry_path("/linux/tree", Path::new("entry with spaces")),
        "/linux/tree/entry with spaces"
    );
    assert_eq!(
        remote_directory_entry_path("/", Path::new("entry")),
        "/entry"
    );
}

#[test]
#[ignore = "requires a local POSIX shell; on Windows uses WSL, never a user SSH connection"]
fn remote_search_posix_fixture_covers_scoping_cap_and_timeout() {
    let dir = "/tmp/android-tool-remote-search-fixture";
    let normal = remote_search_command(dir, "needle", false, 20);
    let code = remote_search_command(dir, "needle", true, 20);
    let timeout = remote_search_command(dir, "unmatched", false, 1);
    let mut script = format!(
        r#"
set -eu
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/tree/.repo" "$fixture/tree/out" "$fixture/tree/vendor" "$fixture/tree/needle-directory" "$fixture/bin"
printf needle > "$fixture/tree/vendor/test.java"
printf needle > "$fixture/tree/vendor/blob.bin"
printf needle > "$fixture/tree/.repo/excluded.java"
printf needle > "$fixture/tree/out/excluded.java"
printf needle > "$fixture/outside.java"
ln -s "$fixture/outside.java" "$fixture/tree/link.java"
ln -s "$fixture/tree" "$fixture/tree/loop"
normal={normal}
code={code}
timed={timeout}
normal=$(printf '%s' "$normal" | sed "s|{dir}|$fixture/tree|g")
code=$(printf '%s' "$code" | sed "s|{dir}|$fixture/tree|g")
timed=$(printf '%s' "$timed" | sed "s|{dir}|$fixture/tree|g")
echo NORMAL
sh -c "$normal"
echo CODE
sh -c "$code"
echo CAP
i=0; while [ "$i" -lt 300 ]; do printf needle > "$fixture/tree/vendor/needle$i.java"; i=$((i + 1)); done
sh -c "$normal"
echo ERROR
printf '#!/bin/sh\nexit 2\n' > "$fixture/bin/find"
chmod +x "$fixture/bin/find"
PATH="$fixture/bin:$PATH" sh -c "$normal"
echo TIMEOUT
printf '#!/bin/sh\n(sleep 3; touch "%s/leaked") &\nsleep 5\n' "$fixture" > "$fixture/bin/find"
chmod +x "$fixture/bin/find"
PATH="$fixture/bin:$PATH" sh -c "$timed"
sleep 3
test ! -e "$fixture/leaked"
"#,
        normal = shell_escape(&normal),
        code = shell_escape(&code),
        timeout = shell_escape(&timeout)
    );
    if let Ok(bin) = std::env::var("ANDROID_TOOL_TEST_LINUX_RG_BIN") {
        script = format!("PATH={}:$PATH\n{script}", shell_escape(&bin));
    }
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", "Ubuntu-22.04", "--exec", "sh"]);
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = Command::new("sh");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixture shell");
    child
        .stdin
        .take()
        .expect("fixture stdin")
        .write_all(script.as_bytes())
        .expect("fixture script");
    let output = child.wait_with_output().expect("fixture output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (_, rest) = stdout.split_once("NORMAL\n").unwrap();
    let (normal, rest) = rest.split_once("CODE\n").unwrap();
    let (code, rest) = rest.split_once("CAP\n").unwrap();
    let (cap, rest) = rest.split_once("ERROR\n").unwrap();
    let (error, timed) = rest.split_once("TIMEOUT\n").unwrap();
    let normal = remote_search_result(dir, "needle", normal, "fixture", false);
    let code = remote_search_result(dir, "needle", code, "fixture", false);
    assert_eq!(normal["results"].as_array().unwrap().len(), 3);
    assert_eq!(code["results"].as_array().unwrap().len(), 3);
    assert_eq!(code["results"][0]["kind"], "directory");
    assert!(code["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"].as_str().unwrap().ends_with("test.java")));
    assert!(code["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"].as_str().unwrap().ends_with("blob.bin")));
    assert_eq!(
        remote_search_result(dir, "needle", cap, "fixture", false)["truncated"],
        true
    );
    assert_eq!(
        remote_search_result(dir, "needle", error, "fixture", false)["partial"],
        true
    );
    assert_eq!(
        remote_search_result(dir, "needle", timed, "fixture", false)["timedOut"],
        true
    );
}

fn context(root: &str) -> SshToolContext {
    SshToolContext::new(
        SshConnectionConfig {
            id: "test".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "tester".to_string(),
            private_key_path: None,
            known_hosts_path: None,
            strict_host_key: false,
            default_root: None,
            reuse_connection: true,
            connect_timeout_secs: 10,
            operation_timeout_secs: 120,
            keepalive_interval_secs: 30,
        },
        root,
        None,
        Arc::new(AtomicBool::new(false)),
        None,
    )
}

#[test]
fn preserves_full_paths_and_relative_paths() {
    let context = context("/home/build/workspace");
    assert_eq!(
        remote_path(&context, &json!({ "path": "/home/build/workspace/source" })).unwrap(),
        "/home/build/workspace/source"
    );
    assert_eq!(
        remote_path(&context, &json!({ "path": "source/vendor" })).unwrap(),
        "/home/build/workspace/source/vendor"
    );
}

#[test]
fn leaves_other_absolute_paths_external() {
    let context = context("/home/build/workspace");
    assert!(remote_path(&context, &json!({ "path": "/tmp/source" })).is_err());
}

#[test]
fn does_not_infer_an_unverified_short_path_alias() {
    let context = context("/srv/project");
    assert!(remote_path(&context, &json!({ "path": "/project/source" })).is_err());
}

#[test]
fn recognizes_transport_failures_that_can_use_openssh_fallback() {
    assert!(should_fallback_to_openssh(
        "Unable to request SFTP subsystem"
    ));
    assert!(should_fallback_to_openssh(
        "Unable to exchange encryption keys"
    ));
    assert!(!should_fallback_to_openssh("SSH authentication failed"));
}

#[test]
fn expands_remote_home_and_environment_paths_for_shell_resolution() {
    assert_eq!(
        remote_shell_path_expression("~/aosp source").unwrap(),
        "$HOME/'aosp source'"
    );
    assert_eq!(
        remote_shell_path_expression("${ANDROID_BUILD_TOP}/system").unwrap(),
        "${ANDROID_BUILD_TOP}/'system'"
    );
    assert_eq!(
        remote_shell_path_expression("/srv/aosp/../vendor").unwrap(),
        "/'srv'/'aosp'/'..'/'vendor'"
    );
    assert!(remote_shell_path_expression("$HOME; rm -rf /").is_err());
}

#[test]
fn rejects_invalid_remote_environment_names() {
    assert!(!is_remote_env_name(""));
    assert!(!is_remote_env_name("1HOME"));
    assert!(!is_remote_env_name("HOME-VAR"));
    assert!(is_remote_env_name("ANDROID_BUILD_TOP"));
}

#[test]
fn matches_names_before_reading_file_contents() {
    assert_eq!(
        search_match_kind("sepolicy", "sepolicy", "/root/system", None),
        Some("name")
    );
    assert_eq!(
        search_match_kind(
            "neverallow",
            "rules.te",
            "/root/system",
            Some("neverallow domain")
        ),
        Some("content")
    );
    assert_eq!(
        search_match_kind("system sepolicy", "sepolicy", "/root/system/sepolicy", None),
        Some("terms")
    );
}

#[test]
fn registers_all_remote_file_and_command_tools() {
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context("/remote/aosp")).unwrap();
    for name in [
        "get_current_time",
        "get_working_directory",
        "get_file_info",
        "list_local_files",
        "search_local_files",
        "search_local_code",
        "read_local_file",
        "write_local_file",
        "create_directory",
        "copy_file",
        "move_file",
        "copy_directory",
        "move_directory",
        "delete_file",
        "upload_file",
        "download_file",
        "upload_paths",
        "download_path",
        "upload_directory",
        "download_directory",
        "set_file_permissions",
        "apply_patch",
        "run_local_command",
        "run_local_shell",
        "run_interactive_shell",
        "run_project_check",
        "get_git_status",
        "get_git_diff",
        "get_directory_tree",
    ] {
        assert!(registry.get(name).is_some(), "missing remote tool: {name}");
    }
}

#[test]
fn remote_file_info_is_read_only_and_requires_a_path() {
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context("/remote/aosp")).unwrap();
    let definition = registry.get("get_file_info").unwrap();
    assert!(definition.read_only);
    assert!(!definition.requires_permission);
    assert!(definition.input_schema["required"]
        .as_array()
        .is_some_and(|required| required.iter().any(|value| value == "path")));
}

#[test]
fn remote_directory_operations_require_permission_and_two_paths() {
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context("/remote/aosp")).unwrap();
    for name in ["copy_directory", "move_directory"] {
        let definition = registry.get(name).unwrap();
        assert!(definition.requires_permission);
        assert!(definition.input_schema["required"]
            .as_array()
            .is_some_and(|required| {
                required.iter().any(|value| value == "source")
                    && required.iter().any(|value| value == "destination")
            }));
    }
}

#[test]
fn remote_public_tool_contract_matches_local_file_and_command_tools() {
    let mut local = ToolRegistry::new();
    crate::register_local_tools(
        &mut local,
        crate::LocalToolContext::new(
            vec!["D:\\workspace".to_string()],
            Vec::new(),
            false,
            Arc::new(AtomicBool::new(false)),
            None,
        ),
    )
    .unwrap();
    let mut remote = ToolRegistry::new();
    register_ssh_tools(&mut remote, context("/remote/aosp")).unwrap();
    let local_names = local
        .list_public()
        .into_iter()
        .map(|definition| definition.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let remote_names = remote
        .list_public()
        .into_iter()
        .map(|definition| definition.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(local_names.is_subset(&remote_names));
    assert!(remote_names.contains("upload_file"));
    assert!(remote_names.contains("download_file"));
}

#[test]
fn command_line_rejects_malformed_arguments_instead_of_dropping_them() {
    assert_eq!(
        command_line(&json!({
            "command": "printf",
            "args": ["%s", "ok"]
        }))
        .unwrap(),
        "printf '%s' 'ok'"
    );
    assert!(command_line(&json!({
        "command": "printf",
        "args": ["%s", 1]
    }))
    .unwrap_err()
    .contains("args[1]"));
    assert!(command_line(&json!({
        "command": "printf",
        "args": "not-an-array"
    }))
    .unwrap_err()
    .contains("字符串数组"));
}

#[test]
fn remote_shell_definition_exposes_portable_shells() {
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context("/remote/aosp")).unwrap();
    let definition = registry.get("run_local_shell").unwrap();
    let shells = definition
        .input_schema
        .pointer("/properties/shell/enum")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(shells, &vec![json!("bash"), json!("sh"), json!("zsh")]);
}

#[test]
fn remote_git_definitions_expose_scope_parameters() {
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context("/remote/aosp")).unwrap();
    let status = registry.get("get_git_status").unwrap();
    assert!(status.input_schema.pointer("/properties/cwd").is_some());
    let diff = registry.get("get_git_diff").unwrap();
    assert!(diff.input_schema.pointer("/properties/cwd").is_some());
    assert!(diff.input_schema.pointer("/properties/path").is_some());
}

#[test]
fn remote_command_wrapper_keeps_display_command_separate_from_pid_wrapper() {
    let (display, execution) = remote_command_scripts("/remote/aosp source", "printf 'ok'");
    assert_eq!(display, "cd '/remote/aosp source' && printf 'ok'");
    assert_eq!(execution, display);
    assert!(!display.contains("__AT_AGENT_PID__"));
}

#[test]
fn applies_remote_unified_patch_only_when_context_matches() {
    let cancellation = Arc::new(AtomicBool::new(false));
    let (updated, hunks) = apply_remote_patch_text(
        "one\ntwo\n",
        "@@ -1,2 +1,2 @@\n-one\n+ONE\n two\n",
        &cancellation,
    )
    .unwrap();
    assert_eq!(updated, "ONE\ntwo\n");
    assert_eq!(hunks, 1);

    let error = apply_remote_patch_text(
        "one\ntwo\n",
        "@@ -1,2 +1,2 @@\n-wrong\n+ONE\n two\n",
        &cancellation,
    )
    .unwrap_err();
    assert!(error.contains("补丁删除行不匹配"));
}

#[test]
fn cancelled_remote_patch_does_not_apply() {
    let cancellation = Arc::new(AtomicBool::new(true));
    let error =
        apply_remote_patch_text("one\n", "@@ -1 +1 @@\n-one\n+ONE\n", &cancellation).unwrap_err();
    assert_eq!(error, "请求已取消");
}

#[test]
fn upload_refusals_map_to_actionable_messages() {
    // 2/3 are produced by the remote guard in the streamed upload command; a
    // bare exit code would otherwise reach the user as "上传文件失败（退出码 2）".
    assert!(upload_status_error(2, "").contains("已存在"));
    assert!(upload_status_error(3, "").contains("目录不存在"));
    assert_eq!(
        upload_status_error(1, "disk full"),
        "上传文件失败：disk full"
    );
    assert_eq!(upload_status_error(1, ""), "上传文件失败（退出码 1）");
}
