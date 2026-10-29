use super::*;

#[test]
fn transfer_path_check_uses_exit_status_and_keeps_scope_failures_blocked() {
    let mut result = SshCommandResult {
        exit_code: 0,
        stdout: "/workspace/source".into(),
        stderr: "Warning: Permanently added host to the list of known hosts.\n".into(),
        output_truncated: false,
    };
    assert_eq!(
        checked_remote_path_result(result.clone()).unwrap(),
        "/workspace/source"
    );
    result.exit_code = 1;
    result.stderr = "传输路径越出 SSH 工作目录".into();
    assert!(checked_remote_path_result(result.clone())
        .unwrap_err()
        .contains("越出"));
    result.exit_code = 0;
    result.output_truncated = true;
    assert!(checked_remote_path_result(result.clone()).is_err());
    result.output_truncated = false;
    result.stdout = "invalid".into();
    assert!(checked_remote_path_result(result).is_err());
}

fn test_context() -> SshToolContext {
    let config = serde_json::from_value(json!({
        "id": "archive-tests", "name": "test", "host": "127.0.0.1",
        "port": 1, "username": "tester", "strictHostKey": false,
    }))
    .unwrap();
    SshToolContext::new(
        config,
        "/workspace",
        None,
        Arc::new(AtomicBool::new(false)),
        None,
    )
}

#[test]
fn archive_sources_accept_exact_selections_and_reject_unselected_paths() {
    let temp = TransferDirectory::create(&std::env::temp_dir()).unwrap();
    let file = temp.0.join("file.txt");
    let directory = temp.0.join("source");
    fs::write(&file, "data").unwrap();
    fs::create_dir(&directory).unwrap();
    let paths = vec![
        file.to_str().unwrap().to_string(),
        directory.to_str().unwrap().to_string(),
    ];
    let context = test_context().with_local_transfer_roots(paths.clone());
    assert_eq!(
        upload_sources(&context, &json!({ "local_paths": paths })).unwrap(),
        paths
    );
    assert!(upload_sources(&context, &json!({ "local_paths": [temp.0] })).is_err());
    assert!(upload_sources(&context, &json!({ "local_paths": ["attachment://image"] })).is_err());
    assert!(upload_sources(&context, &json!({ "local_paths": [] })).is_err());
    assert!(upload_sources(&context, &json!({ "local_paths": [1] })).is_err());
    assert!(upload_sources(
        &context,
        &json!({ "local_paths": vec![paths[0].clone(); 129] })
    )
    .is_err());
}

#[tokio::test]
async fn archive_tools_require_permission_and_cancel_before_connecting() {
    let temp = TransferDirectory::create(&std::env::temp_dir()).unwrap();
    let context = test_context().with_local_transfer_roots(vec![temp.0.to_str().unwrap().into()]);
    context.cancellation.store(true, Ordering::Relaxed);
    let mut registry = ToolRegistry::new();
    register_ssh_tools(&mut registry, context).unwrap();
    for (name, input) in [
        (
            "upload_paths",
            json!({ "local_paths": [temp.0], "remote_path": "/workspace" }),
        ),
        (
            "download_path",
            json!({ "local_path": temp.0, "remote_path": "/workspace/source" }),
        ),
    ] {
        assert!(registry.get(name).unwrap().requires_permission);
        assert!(registry
            .execute(
                name,
                input.clone(),
                &crate::DenyPermission {
                    reason: "denied".into()
                }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("denied"));
        assert!(registry
            .execute(name, input, &crate::AllowPermission)
            .await
            .unwrap_err()
            .to_string()
            .contains("取消"));
    }
    assert!(upload_policy(&json!({ "conflict_policy": "unknown" })).is_err());
}

#[test]
fn archive_permission_scope_tracks_remote_destination_without_local_path_confusion() {
    let roots = vec!["/workspace".to_string()];
    for name in ["ssh__upload_paths", "ssh__download_path"] {
        let input = json!({
            "local_paths": ["D:\\chosen\\source"], "local_path": "D:\\chosen",
            "remote_path": "/workspace/uploads",
        });
        assert!(crate::path_access_tool(name));
        assert!(!crate::path_access_requires_external_execution_for_target(
            name, &input, &roots, true
        ));
        let enriched = crate::enrich_permission_input_for_target(
            name, &input, "ask", &roots, true, "ssh:test",
        );
        assert_eq!(enriched["permission"]["target"], "/workspace/uploads");
        let external = json!({ "remote_path": "/outside", "local_paths": ["D:\\chosen"] });
        assert!(crate::path_access_requires_external_execution_for_target(
            name, &external, &roots, true
        ));
    }
}

#[test]
fn archive_progress_is_throttled_and_temporary_payload_is_cleaned_after_failure() {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = events.clone();
    let sink = CommandOutputSink::new(
        Arc::new(move |event| captured.lock().unwrap().push(event)),
        "session",
        "turn",
        "upload_paths",
    )
    .with_call_id("upload-call");
    let progress = archive_progress(Some(sink), "/workspace/uploads".into());
    progress("transferring", 10, 100);
    progress("transferring", 20, 100);
    progress("extracting", 0, 200);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["call_id"], "upload-call");
    assert_eq!(events[0]["progress"]["completed"], 10);
    assert!(events[0]["progress"]["message"]
        .as_str()
        .unwrap()
        .contains("10 / 100"));
    assert_eq!(events[1]["progress"]["stage"], "extracting");
    let owner = TransferDirectory::create(&std::env::temp_dir()).unwrap();
    let mut owned_path = PathBuf::new();
    let failed = (|| -> Result<(), String> {
        let temp = TransferDirectory::create(&owner.0)?;
        owned_path = temp.0.clone();
        fs::write(temp.0.join("payload.tar.gz"), "incomplete").unwrap();
        Err("模拟中断".into())
    })();
    assert!(failed.is_err());
    assert!(!owned_path.exists());
    assert!(owner.0.exists());
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated SSH fixture"]
async fn real_ssh_archive_function_calls_roundtrip_and_handle_conflicts() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let base_config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    let temp = TransferDirectory::create(&std::env::temp_dir()).unwrap();
    let source = temp.0.join("source with 中文");
    fs::create_dir_all(source.join("nested/empty")).unwrap();
    let bytes = (0..400_000)
        .map(|index| ((index * 17 + index / 256) % 256) as u8)
        .collect::<Vec<_>>();
    fs::write(source.join("nested/中文 ' file.bin"), &bytes).unwrap();
    fs::write(source.join("empty.txt"), []).unwrap();
    let extra = temp.0.join("extra.txt");
    fs::write(&extra, "first").unwrap();
    let paths = vec![
        source.to_str().unwrap().to_string(),
        extra.to_str().unwrap().to_string(),
    ];
    for fallback in [false, true] {
        let config = transfer_connection(
            &base_config,
            if fallback {
                ":agent-openssh-test"
            } else {
                ":agent-native-test"
            },
        );
        if fallback {
            connection::remember_openssh_transport(&config);
        }
        assert_eq!(uses_openssh_fallback(&config), fallback);
        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let listener_events = events.clone();
        let sink = CommandOutputSink::new(
            Arc::new(move |event| listener_events.lock().unwrap().push(event)),
            "session",
            "turn",
            "upload_paths",
        );
        let cancellation = Arc::new(AtomicBool::new(false));
        let context = SshToolContext::new(
            config.clone(),
            &fixture.remote_root,
            None,
            cancellation.clone(),
            Some(sink),
        )
        .with_local_transfer_roots(vec![temp.0.to_str().unwrap().into()]);
        let setup = run_ssh_workspace_command(&config, None, &fixture.remote_root,
            "mkdir uploads; ln -s /tmp escape; mkdir unsafe; ln -s /etc/passwd unsafe/link; truncate -s 1073741825 large.bin",
            cancellation.clone(), Duration::from_secs(30)).unwrap();
        assert_eq!(setup.exit_code, 0, "{}", setup.stderr);
        let remote = format!("{}/uploads", fixture.remote_root);
        let mut registry = ToolRegistry::new();
        register_ssh_tools(&mut registry, context).unwrap();
        let upload =
            json!({ "local_paths": paths, "remote_path": remote, "__agent_call_id": "bulk-1" });
        let result = registry
            .execute("upload_paths", upload.clone(), &crate::AllowPermission)
            .await
            .unwrap();
        assert_eq!(result["transport"], "archive");
        assert_eq!(result["items"], 2);
        assert!(registry
            .execute("upload_paths", upload.clone(), &crate::AllowPermission)
            .await
            .is_err());
        let download_dir = temp.0.join(if fallback { "openssh" } else { "native" });
        fs::create_dir(&download_dir).unwrap();
        let remote_source = format!("{remote}/source with 中文");
        let download = json!({ "remote_path": remote_source, "local_path": download_dir });
        registry
            .execute("download_path", download.clone(), &crate::AllowPermission)
            .await
            .unwrap();
        assert_eq!(
            fs::read(download_dir.join("source with 中文/nested/中文 ' file.bin")).unwrap(),
            bytes
        );
        assert!(download_dir.join("source with 中文/nested/empty").is_dir());
        assert!(download_dir.join("source with 中文/empty.txt").is_file());
        assert!(registry
            .execute("download_path", download.clone(), &crate::AllowPermission)
            .await
            .is_err());
        let mut renamed = download.clone();
        renamed["conflict_policy"] = json!("rename");
        let result = registry
            .execute("download_path", renamed, &crate::AllowPermission)
            .await
            .unwrap();
        assert!(result["localPath"]
            .as_str()
            .unwrap()
            .ends_with("source with 中文 (1)"));
        let mut renamed = upload.clone();
        renamed["conflict_policy"] = json!("rename");
        registry
            .execute("upload_paths", renamed, &crate::AllowPermission)
            .await
            .unwrap();
        fs::write(&extra, "replacement").unwrap();
        let mut overwritten = upload.clone();
        overwritten["conflict_policy"] = json!("overwrite");
        registry
            .execute("upload_paths", overwritten, &crate::AllowPermission)
            .await
            .unwrap();
        registry
            .execute(
                "download_path",
                json!({ "remote_path": format!("{remote}/extra.txt"), "local_path": download_dir }),
                &crate::AllowPermission,
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(download_dir.join("extra.txt")).unwrap(),
            "replacement"
        );
        for path in [
            format!("{}/escape", fixture.remote_root),
            format!("{}/unsafe", fixture.remote_root),
            "/etc/passwd".into(),
        ] {
            assert!(registry
                .execute(
                    "download_path",
                    json!({ "remote_path": path, "local_path": download_dir }),
                    &crate::AllowPermission
                )
                .await
                .is_err());
        }
        let large = registry.execute("download_path", json!({ "remote_path": format!("{}/large.bin", fixture.remote_root), "local_path": download_dir }), &crate::AllowPermission).await.unwrap_err().to_string();
        assert!(large.contains("再次确认"), "{large}");
        assert!(!download_dir.join("large.bin").exists());
        let upload_events = events.lock().unwrap().clone();
        for stage in [
            "checking",
            "compressing",
            "transferring",
            "extracting",
            "completed",
        ] {
            assert!(
                upload_events.iter().any(
                    |event| event["progress"]["stage"] == stage && event["call_id"] == "bulk-1"
                ),
                "missing {stage}"
            );
        }
        assert!(!fs::read_dir(&download_dir).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("agent-transfer")));
        cancellation.store(true, Ordering::Relaxed);
        assert!(registry
            .execute("upload_paths", upload, &crate::AllowPermission)
            .await
            .unwrap_err()
            .to_string()
            .contains("取消"));
        let cleanup = run_ssh_workspace_command(
            &config,
            None,
            &fixture.remote_root,
            "test -f hostkey && test ! -L uploads && rm -rf -- uploads unsafe escape large.bin",
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(cleanup.exit_code, 0, "{}", cleanup.stderr);
        disconnect_ssh_connection(&config);
    }
}
