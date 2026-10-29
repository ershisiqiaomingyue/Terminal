use super::*;
use std::time::SystemTime;

#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated loopback SSH fixture"]
fn real_ssh_download_credentials_use_stdin_in_both_transports() {
    use sha2::{Digest, Sha256};
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let original: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    for fallback in [false, true] {
        let config = transfer_connection(&original, &format!(":download-stdin-{fallback}"));
        if fallback {
            remember_openssh_transport(&config);
        }
        let secret = "test-only ' $() secret";
        let command = "IFS= read -r supplied; test -n \"$supplied\" || exit 9; printf '%s' \"$supplied\" | sha256sum";
        let access = SshDownloadAccess {
            elevated: true,
            sudo_password: None,
        };
        // Verify the saved SSH secret travels as stdin without ever being
        // interpolated into the command sent to either transport.
        let result = access
            .run(
                &config,
                Some(secret),
                command,
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            result.stdout.split_whitespace().next().unwrap(),
            format!("{:x}", Sha256::digest(secret.as_bytes()))
        );
        assert!(!result.stderr.contains(secret));
        disconnect_ssh_connection(&config);
    }
}

#[test]
#[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated SSH fixture"]
fn real_ssh_compressed_transfers_and_collision_choices() {
    let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
    let fixture = ssh_fixture::SshFixture::start(&distribution);
    let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    let local = std::env::temp_dir().join(format!(
        "android-tool-transfer-test-{}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(local.join("source/nested")).unwrap();
    let data: Vec<u8> = (0..400_000)
        .map(|index| ((index * 17 + index / 256) % 256) as u8)
        .collect();
    fs::write(local.join("source/nested/中文 ' file.bin"), &data).unwrap();
    fs::write(local.join("source/text.txt"), "上传内容\n").unwrap();
    let paths = vec![local.join("source").to_string_lossy().into_owned()];
    let setup = run_ssh_workspace_command(
        &config,
        None,
        &fixture.remote_root,
        "mkdir uploads; truncate -s 1073741825 large.bin",
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(setup.exit_code, 0, "{}", setup.stderr);
    let uploads = format!("{}/uploads", fixture.remote_root);
    // Two lanes, two real transports. The russh engine only has exec channels,
    // so an engine-routed lane is by definition *not* the libssh2 SFTP path:
    // `uses_openssh_fallback` is true for it while `is_openssh_transport_marked`
    // is false. The ssh.exe lane instead records a russh transport failure on
    // its own key, which takes the engine out for that lane and leaves the
    // explicit openssh mark as the only reason it falls back. Asserting the
    // mark, the engine decision and the effective decision together is what
    // pins each lane to the transport it is named after; the transfers below
    // then run for real on both.
    let native = transfer_connection(&config, ":transfer-native-test");
    let openssh = transfer_connection(&config, ":transfer-openssh-test");
    remember_russh_transport_failure(&openssh);
    for (marked, config) in [(false, &native), (true, &openssh)] {
        if marked {
            remember_openssh_transport(config);
        }
        assert_eq!(is_openssh_transport_marked(config), marked);
        assert_eq!(
            russh_engine_active(config),
            !marked,
            "the russh engine must be the routing decision exactly when the lane is unmarked"
        );
        assert_eq!(
            uses_openssh_fallback(config),
            true,
            "every lane runs the transfer through a shell channel, never through SFTP"
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let progress: TransferProgress = Arc::new(|_, _, _| {});
        let info =
            ssh_browser_transfer_info(&config, None, &format!("{}/large.bin", fixture.remote_root))
                .unwrap();
        assert!(info.requires_confirmation);
        assert_eq!(info.bytes, LARGE_DOWNLOAD_BYTES + 1);
        let blocked = download_ssh_browser_path(
            &config,
            None,
            &format!("{}/large.bin", fixture.remote_root),
            &local,
            false,
            cancel.clone(),
            progress.clone(),
        )
        .unwrap_err();
        assert!(blocked.contains("再次确认"));
        let clean = run_ssh_workspace_command(
            &config,
            None,
            &uploads,
            "rm -rf -- source source.1 source.2",
            cancel.clone(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(clean.exit_code, 0);
        assert!(inspect_ssh_browser_upload(&config, None, &paths, &uploads)
            .unwrap()
            .is_empty());
        upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Reject,
            &[],
            cancel.clone(),
            progress.clone(),
        )
        .unwrap();
        let conflicts = inspect_ssh_browser_upload(&config, None, &paths, &uploads).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].name, "source");
        assert!(upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Reject,
            &[],
            cancel.clone(),
            progress.clone()
        )
        .is_err());
        fs::write(local.join("source/text.txt"), "第二版\n").unwrap();
        upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Rename,
            &conflicts,
            cancel.clone(),
            progress.clone(),
        )
        .unwrap();
        upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Rename,
            &conflicts,
            cancel.clone(),
            progress.clone(),
        )
        .unwrap();
        let out = local.join(if marked { "openssh" } else { "native" });
        fs::create_dir(&out).unwrap();
        download_ssh_browser_path(
            &config,
            None,
            &format!("{uploads}/source.1"),
            &out,
            false,
            cancel.clone(),
            progress.clone(),
        )
        .unwrap();
        assert_eq!(
            fs::read(out.join("source.1/nested/中文 ' file.bin")).unwrap(),
            data
        );
        assert_eq!(
            fs::read_to_string(out.join("source.1/text.txt")).unwrap(),
            "第二版\n"
        );
        upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Overwrite,
            &conflicts,
            cancel.clone(),
            progress.clone(),
        )
        .unwrap();
        let result = run_ssh_workspace_command(&config, None, &uploads, "test -d source.2 && test -z \"$(find . -maxdepth 1 -name '.android-tool-transfer.*' -print -quit)\" && cat source/text.txt", cancel.clone(), Duration::from_secs(30)).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "第二版\n");
        // The previously approved identity cannot authorize another writer's replacement.
        assert!(upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Overwrite,
            &conflicts,
            cancel.clone(),
            progress.clone()
        )
        .unwrap_err()
        .contains("重新确认"));
        cancel.store(true, Ordering::Relaxed);
        assert!(upload_ssh_browser_paths(
            &config,
            None,
            &paths,
            &uploads,
            &local.join("payload.tgz"),
            UploadPolicy::Rename,
            &[],
            cancel,
            progress
        )
        .is_err());
        disconnect_ssh_connection(config);
    }
    // Leave no engine/fallback memory behind for later runs of this fixture.
    reset_russh_transport_failure(&openssh);
    invalidate_cached_connection(&native);
    invalidate_cached_connection(&openssh);
    fs::remove_dir_all(local).unwrap();
    let cleanup = run_ssh_workspace_command(
        &config,
        None,
        &fixture.remote_root,
        "test -d uploads && test ! -L uploads && rm -rf -- uploads large.bin",
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(cleanup.exit_code, 0);
}
