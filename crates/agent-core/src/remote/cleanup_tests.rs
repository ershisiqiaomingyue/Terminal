use super::command_transport::{
    execute_remote_command_inner, execute_remote_pty_command_inner, finalize_remote_process,
    recover_remote_cleanup, remote_cleanup_command, SshCleanup,
};
use super::*;

#[test]
fn ssh_status_symbolic_roots_do_not_relock_the_connection_lane() {
    for root in ["", "~", "~/workspace", "$HOME/workspace"] {
        let config: SshConnectionConfig = serde_json::from_value(json!({
            "id": format!("status-symbolic-root-{root}"),
            "name": "test", "host": "", "port": 22, "username": "test"
        }))
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(query_ssh_status(&config, None, root));
        });
        let result = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("symbolic root resolution deadlocked inside the SSH status lane");
        assert!(result.unwrap_err().contains("SSH 主机和用户名不能为空"));
        worker.join().unwrap();
    }
}

#[test]
fn ssh_cleanup_rejects_unowned_paths() {
    for root in [
        "/tmp",
        "/",
        "/tmp/android-tool-process.123456789012/../other",
    ] {
        assert!(remote_cleanup_command(root, SshCleanup::Drain).is_err());
        assert!(remote_cleanup_command(root, SshCleanup::Remove).is_err());
    }
}

#[test]
fn ssh_turn_preparation_waits_for_shared_operation_lane() {
    for prepare_transport in [true, false] {
        let config: SshConnectionConfig = serde_json::from_value(json!({
            "id":format!("turn-preparation-lock-{prepare_transport}"),
            "name":"test", "host":"", "port":22, "username":"test"
        }))
        .unwrap();
        let lock = connection_operation_lock(&config);
        let guard = lock.lock().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = if prepare_transport {
                prepare_ssh_transport(&config, None)
            } else {
                resolve_ssh_root(&config, None, Some("/")).map(|_| ())
            };
            tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let blocked = rx.recv_timeout(Duration::from_millis(100)).is_err();
        drop(guard);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        worker.join().unwrap();
        assert!(blocked, "turn preparation bypassed the shared SSH lane");
    }
}

#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated loopback sshd"]
fn real_ssh_cleanup_recovers_lost_acknowledgements_without_replaying_commands() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    for pty in [false, true] {
        let session = connect_fresh(&config, None).unwrap();
        session.set_blocking(false);
        let context = SshToolContext::new(
            config.clone(),
            "/",
            None,
            Arc::new(AtomicBool::new(false)),
            None,
        );
        let root = execute_remote_control(&session, crate::owned_process::PREPARE).unwrap();
        let counter = format!("{}/executions-{pty}", fixture.remote_root);
        let command = format!(
            "printf 'once\\n' >> {}; printf completed",
            shell_escape(&counter)
        );
        let mut channel = retry_nonblocking("test channel", Duration::from_secs(5), || {
            session.channel_session()
        })
        .unwrap();
        let output = if pty {
            execute_remote_pty_command_inner(
                &mut channel,
                &session,
                &command,
                None,
                &context,
                None,
                Duration::from_secs(10),
                &root,
            )
        } else {
            execute_remote_command_inner(
                &mut channel,
                &session,
                &crate::owned_process::shell_command(&root, "sh", &["-c".into(), command]),
                &context,
                None,
                Duration::from_secs(10),
            )
        }
        .unwrap();
        assert_eq!(output["exitCode"], 0);
        let _ = channel.close();
        let drain = remote_cleanup_command(
            &root,
            if pty {
                SshCleanup::DrainPty
            } else {
                SshCleanup::Drain
            },
        )
        .unwrap();
        execute_remote_control(&session, &drain).unwrap();
        // The server completed the operation, but the client lost its reply.
        recover_remote_cleanup(
            &context,
            &drain,
            Err("SSH 操作失败: [Session(-1)] Failed waiting for channel success".into()),
        )
        .unwrap();
        if pty {
            // Also cover disconnect after killing the leader but before the
            // durable drained marker could be written.
            execute_remote_control(
                &session,
                &format!("rm -f -- {}/drained", shell_escape(&root)),
            )
            .unwrap();
            recover_remote_cleanup(
                &context,
                &drain,
                Err("reply lost before drain marker".into()),
            )
            .unwrap();
        }
        let remove = remote_cleanup_command(&root, SshCleanup::Remove).unwrap();
        execute_remote_control(&session, &remove).unwrap();
        recover_remote_cleanup(&context, &remove, Err("remove reply lost".into())).unwrap();
        let count = execute_remote_control(
            &session,
            &format!(
                "cat {}; rm -f -- {}",
                shell_escape(&counter),
                shell_escape(&counter)
            ),
        )
        .unwrap();
        assert_eq!(count, "once", "original command must execute exactly once");
    }
}

#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL; drops only isolated loopback transports"]
fn real_ssh_cleanup_reconnects_after_transport_loss() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let proxy = ssh_fixture::SshFaultProxy::start(&fixture);
    let config: SshConnectionConfig = serde_json::from_value(proxy.config.clone()).unwrap();
    let session = connect_fresh(&config, None).unwrap();
    session.set_blocking(false);
    let context = SshToolContext::new(config, "/", None, Arc::new(AtomicBool::new(false)), None);
    let root = execute_remote_control(&session, crate::owned_process::PREPARE).unwrap();
    let remove = remote_cleanup_command(&root, SshCleanup::Remove).unwrap();
    assert!(
        execute_remote_control(&session, &remove).is_err(),
        "removal requires an acknowledged drain"
    );
    proxy.drop_connections(false);
    finalize_remote_process(&session, &context, &root, false).unwrap();
    let fresh = connect_fresh(&context.config, None).unwrap();
    fresh.set_blocking(false);
    execute_remote_control(&fresh, &format!("test ! -e {}", shell_escape(&root))).unwrap();
}

#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated loopback sshd"]
fn real_ssh_cleanup_cancellation_stops_live_children() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    for pty in [false, true] {
        let session = connect_fresh(&config, None).unwrap();
        session.set_blocking(false);
        let cancellation = Arc::new(AtomicBool::new(false));
        let captured = Arc::new(Mutex::new(String::new()));
        let text = captured.clone();
        let cancel = cancellation.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| {
                if event["phase"] == "chunk" && event["stream"] == "stdout" {
                    let mut text = text.lock().unwrap();
                    text.push_str(event["text"].as_str().unwrap_or_default());
                    if text.contains('\n') {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            }),
            "test",
            "test",
            "run_local_shell",
        );
        let context =
            SshToolContext::new(config.clone(), "/", None, cancellation, Some(sink.clone()));
        let channel = retry_nonblocking("test channel", Duration::from_secs(5), || {
            session.channel_session()
        })
        .unwrap();
        let command = "sleep 45 & worker=$!; printf '%s\\n' \"$worker\"; wait \"$worker\"";
        let result = if pty {
            execute_remote_pty_command(
                channel,
                &session,
                command,
                None,
                &context,
                Some(&sink),
                Duration::from_secs(10),
            )
        } else {
            execute_remote_command(
                channel,
                &session,
                command,
                &context,
                Some(&sink),
                Duration::from_secs(10),
                None,
            )
        };
        assert!(
            matches!(result, Err(error) if error.contains("取消") && !error.contains("process_cleanup_unconfirmed"))
        );
        let pid: u32 = captured
            .lock()
            .unwrap()
            .lines()
            .next()
            .expect("child PID")
            .trim()
            .parse()
            .unwrap();
        let state =
            execute_remote_control(&session, &format!("ps -o stat= -p {pid} || true")).unwrap();
        assert!(
            state.is_empty() || state.starts_with('Z'),
            "child survived cancellation: {state}"
        );
    }
}
