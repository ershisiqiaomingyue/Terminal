
#[test]
fn proxy_switches_migrate_legacy_config_and_preserve_disabled_address() {
    let legacy: super::EnvironmentToolConfig = serde_json::from_str(
        r#"{"paths":{"java":"local-java"},"proxy_url":"http://127.0.0.1:7890"}"#,
    )
    .unwrap();
    let settings = legacy.network_settings();
    assert!(settings.proxy_enabled);
    assert!(!settings.proxy_child_processes);
    assert!(
        !super::EnvironmentToolConfig::default()
            .network_settings()
            .proxy_enabled
    );

    let disabled = super::EnvironmentToolConfig {
        proxy_enabled: Some(false),
        proxy_child_processes: true,
        ..legacy
    };
    let persisted = serde_json::to_string(&disabled).unwrap();
    let restored: super::EnvironmentToolConfig = serde_json::from_str(&persisted).unwrap();
    let settings = restored.network_settings();
    assert!(!settings.proxy_enabled);
    assert!(settings.proxy_child_processes);
    assert_eq!(settings.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
    assert_eq!(
        restored.paths.get("java").map(String::as_str),
        Some("local-java")
    );
}

use std::{process::Command, sync::atomic::AtomicBool, time::Duration};

use super::{
    addr2line_fallback, analyze_aosp_build_log, android_service_unavailable_message,
    aosp_apk_installability, aosp_build_heartbeat_progress, apk_device_compatibility,
    benchmark_result_remote_path, bounded_performance_support_output, changed_line_counts,
    checked_adb_pairing_code, checked_adb_wireless_endpoint, checked_aosp_artifact_path,
    checked_remote_path, clean_aosp_lunch_output, compare_android_archives,
    ensure_archive_build_not_cancelled, find_llvm_addr2line, is_png_bytes, manifest_package_name,
    native_command_output, normalize_install_error, normalize_simpleperf_error,
    normalize_uiautomator_error, parse_adb_remote_files, parse_android_instrumentations,
    perfetto_capture_progress, read_bounded_logcat_tail, read_process_stream_tail,
    repair_apktool_resource_package, should_index_embedded_aar_jar, simpleperf_capture_progress,
    summarize_adb_diagnostic, summarize_android_benchmark_json, summarize_hprof_container,
    summarize_method_trace, tombstone_frames, winscope_trace_command_error,
    workspace_change_detail, workspace_structured_changes, ArchiveCompareRequest,
    HeapSharkAnalysis, MAX_AAR_EMBEDDED_JAR_INDEX_BYTES,
};
use std::{
    fs::{self, File},
    io::{BufReader, Cursor, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
#[test]
fn environment_tool_paths_remove_java_incompatible_verbatim_prefixes() {
    for (input, expected) in [
        (
            r"\\?\D:\Android Toolkit\apktool.jar",
            r"D:\Android Toolkit\apktool.jar",
        ),
        (
            r"\\?\UNC\server\tools\bundletool.jar",
            r"\\server\tools\bundletool.jar",
        ),
        (r"D:\Android Toolkit\java.exe", r"D:\Android Toolkit\java.exe"),
    ] {
        assert_eq!(
            super::normalized_environment_tool_path(Path::new(input)),
            Path::new(expected)
        );
    }
}

#[cfg(windows)]
#[test]
fn environment_batch_launcher_preserves_paths_and_arguments_with_spaces() {
    let root = std::env::temp_dir().join(format!(
        "android toolkit launcher \u{5de5}\u{5177} {} {}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let args = [
        "one two",
        "a&b",
        "a|b",
        "<in>out",
        "(paren)^",
        "%PATH%",
        "!PATH!",
        "",
        "C:\\directory with spaces\\",
    ];
    fs::write(
        root.join("arguments.ps1"),
        b"foreach ($value in $args) { '\"' + $value + '\"' }\r\n",
    )
    .unwrap();
    // Tool launchers forward %* to a native program, which decodes Windows argv.
    let script = b"@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0arguments.ps1\" %*\r\n";
    for extension in ["bat", "cmd"] {
        let batch = root.join(format!("tool launcher.{extension}"));
        fs::write(&batch, script).unwrap();
        let mut command = super::environment_tool_command(&batch);
        command.args(args);
        let output = super::command_output_with_timeout(command, Duration::from_secs(10)).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lines = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines, args.map(|arg| format!("\"{arg}\"")));

        for invalid in ["first\nsecond", "first\rsecond"] {
            let error = super::environment_tool_command(&batch)
                .arg(invalid)
                .output()
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn accepts_absolute_device_paths_only() {
    assert!(checked_remote_path("/sdcard/Download").is_ok());
    assert!(checked_remote_path("sdcard/Download").is_err());
    assert!(checked_remote_path("/sdcard\nother").is_err());
}

#[test]
fn validates_wireless_adb_endpoints_and_pairing_codes() {
    assert_eq!(
        checked_adb_wireless_endpoint("192.168.1.9:5555").unwrap(),
        "192.168.1.9:5555"
    );
    assert!(checked_adb_wireless_endpoint("phone.local:37123").is_ok());
    assert!(checked_adb_wireless_endpoint("host; reboot:5555").is_err());
    assert!(checked_adb_wireless_endpoint("host:0").is_err());
    assert_eq!(checked_adb_pairing_code("123456").unwrap(), "123456");
    assert!(checked_adb_pairing_code("12345").is_err());
    assert!(checked_adb_pairing_code("12345x").is_err());
}

#[test]
fn identifies_android_system_service_unavailability() {
    assert!(android_service_unavailable_message("cmd: Can't find service: package").is_some());
    assert!(android_service_unavailable_message(
        "Waited one second for activity (is service started?)"
    )
    .is_some());
    assert!(android_service_unavailable_message("package:com.example.ready").is_none());
}

#[test]
fn summarizes_only_known_dumpsys_metrics_without_inventing_missing_values() {
    let battery = summarize_adb_diagnostic("battery", "Current Battery Service state:\n  AC powered: false\n  level: 87\n  temperature: 315\n  arbitrary: ignored\n");
    assert_eq!(
        battery.metrics.get("AC powered").map(String::as_str),
        Some("false")
    );
    assert_eq!(battery.metrics.get("level").map(String::as_str), Some("87"));
    assert!(!battery.metrics.contains_key("arbitrary"));
    let graphics = summarize_adb_diagnostic(
        "graphics",
        "Janky frames: 4 (2.0%)\n95th percentile: 17ms\n",
    );
    assert_eq!(
        graphics.metrics.get("Janky frames").map(String::as_str),
        Some("4 (2.0%)")
    );
    assert_eq!(
        graphics.metrics.get("95th percentile").map(String::as_str),
        Some("17ms")
    );
    let compositor = summarize_adb_diagnostic("surfaceflinger", "HWC missed frame count: 58\nGPU missed frame count: 0\nvulkan_renderengine: false\nunknown: ignored\n");
    assert_eq!(
        compositor
            .metrics
            .get("HWC missed frame count")
            .map(String::as_str),
        Some("58")
    );
    assert_eq!(
        compositor
            .metrics
            .get("GPU missed frame count")
            .map(String::as_str),
        Some("0")
    );
    assert!(!compositor.metrics.contains_key("unknown"));
    assert!(summarize_adb_diagnostic("network", "uid=1000")
        .metrics
        .is_empty());
}

#[test]
fn parses_toybox_listing_and_sorts_directories_first() {
    let entries = parse_adb_remote_files("/sdcard", "total 8\ndrwxrwx--- 2 media_rw media_rw 4096 2026-09-15 08:20 Photos\n-rw-rw---- 1 media_rw media_rw 1536 2026-09-15 08:21 notes file.txt\n");
    assert_eq!(entries.len(), 2);
    assert!(entries[0].is_directory);
    assert_eq!(entries[0].path, "/sdcard/Photos");
    assert_eq!(entries[1].name, "notes file.txt");
    assert_eq!(entries[1].size, Some(1536));
}

#[test]
fn removes_only_terminal_size_noise_from_lunch_output() {
    let output = clean_aosp_lunch_output("your 131072x1 screen size is bogus. expect trouble\n\nLunch menu .. Here are the common combinations:\n     1. aosp_x86_64-trunk_staging-eng\n".to_string());
    assert!(!output.contains("screen size is bogus"));
    assert!(output.contains("Lunch menu"));
    assert!(output.contains("aosp_x86_64-trunk_staging-eng"));
}

#[test]
fn aosp_build_heartbeat_stays_conservative_and_reports_elapsed_minutes() {
    assert_eq!(aosp_build_heartbeat_progress(0), (18, 1));
    assert_eq!(aosp_build_heartbeat_progress(60).1, 1);
    assert_eq!(aosp_build_heartbeat_progress(61).1, 2);
    assert!(aosp_build_heartbeat_progress(60 * 60).0 > 18);
    assert_eq!(aosp_build_heartbeat_progress(3 * 60 * 60), (92, 180));
    assert_eq!(aosp_build_heartbeat_progress(24 * 60 * 60).0, 92);
}

#[test]
fn aosp_probe_reports_only_detected_metadata_and_preserves_zero_count() {
    let target = super::AospLinuxTargetRequest {
        target_kind: "wsl".into(),
        target_id: "test".into(),
        root: "/tmp".into(),
    };
    let missing = super::parse_aosp_probe(&target, "__AT_AOSP_LINUX__1\n__AT_AOSP_BASH__1\n");
    assert_eq!(missing.android_bp_count, None);
    assert_eq!(missing.git_branch, None);
    assert_eq!(missing.ccache_status, None);
    assert!(missing.output.is_empty());
    let detected = super::parse_aosp_probe(&target, "__AT_AOSP_BP_COUNT__0\n__AT_AOSP_GIT_BRANCH__main\n__AT_AOSP_CCACHE_BEGIN__\nHits: 0 / 0\n__AT_AOSP_CCACHE_END__\n__AT_AOSP_SIGNATURE__Linux test\n__AT_AOSP_SIGNATURE__GNU bash test\n");
    assert_eq!(detected.android_bp_count, Some(0));
    assert_eq!(detected.git_branch.as_deref(), Some("main"));
    assert_eq!(detected.ccache_status.as_deref(), Some("Hits: 0 / 0"));
    assert_eq!(detected.output, "Linux test\nGNU bash test");
}

#[test]
fn aosp_live_output_reads_while_writer_is_open_and_preserves_utf8_offsets() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("aosp-live-output-{}.log", std::process::id()));
    let mut writer = fs::File::create(&path).unwrap();
    writer.write_all(&vec![b'x'; 65535]).unwrap();
    writer.write_all("构建\n".as_bytes()).unwrap();
    let first = super::read_aosp_output_chunk(&path, 0).unwrap();
    assert_eq!(first.text.len(), 65535);
    assert_eq!(first.next_offset, 65535);
    let second = super::read_aosp_output_chunk(&path, first.next_offset).unwrap();
    assert_eq!(second.text, "构建\n");
    writer.write_all("后台继续输出\n".as_bytes()).unwrap();
    let third = super::read_aosp_output_chunk(&path, second.next_offset).unwrap();
    assert_eq!(third.text, "后台继续输出\n");
    assert!(super::read_aosp_output_chunk(&path, third.next_offset)
        .unwrap()
        .text
        .is_empty());
    drop(writer);
    fs::remove_file(path).unwrap();
}

#[test]
fn recognizes_aosp_transport_errors_in_managed_build_logs() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let log = std::env::temp_dir().join(format!("android-tool-aosp-{suffix}.log"));
    fs::write(
        &log,
        "target=wsl:test\ntransport_error=SSH connection timed out\n",
    )
    .unwrap();
    let summary = analyze_aosp_build_log(log.to_string_lossy().to_string()).unwrap();
    assert_eq!(
        summary.errors,
        vec!["transport_error=SSH connection timed out"]
    );
    fs::remove_file(log).unwrap();
}

#[test]
fn explains_common_apk_install_failures_without_hiding_adb_output() {
    let sdk = normalize_install_error("Failure [INSTALL_FAILED_OLDER_SDK: Requires development platform VanillaIceCream but this is a release platform.]");
    assert!(sdk.contains("开发版 Android 平台"));
    assert!(sdk.contains("INSTALL_FAILED_OLDER_SDK"));
    assert!(
        normalize_install_error("Failure [INSTALL_FAILED_VERSION_DOWNGRADE]").contains("版本高于")
    );
    assert!(normalize_install_error("").contains("没有返回错误详情"));
}

#[test]
fn catches_only_proven_apk_device_incompatibilities_before_install() {
    assert!(apk_device_compatibility(Some("35"), Some(34), "REL")
        .unwrap()
        .contains("最低 SDK"));
    assert!(
        apk_device_compatibility(Some("VanillaIceCream"), Some(37), "REL")
            .unwrap()
            .contains("开发版")
    );
    assert!(
        apk_device_compatibility(Some("VanillaIceCream"), Some(34), "VanillaIceCream").is_none()
    );
    assert!(apk_device_compatibility(Some("FutureCodename"), Some(40), "FutureDevice").is_none());
    assert!(apk_device_compatibility(None, Some(34), "REL").is_none());
}

#[test]
fn explains_simpleperf_permission_and_target_failures() {
    let root = normalize_simpleperf_error("System wide profiling needs root privilege.");
    assert!(root.contains("需要 root adbd"));
    let denied = normalize_simpleperf_error(
        "failed to open perf event file for event_type cpu-clock:u: Permission denied",
    );
    assert!(denied.contains("perf_event 权限"));
    assert!(normalize_simpleperf_error("No threads to monitor.").contains("目标线程"));
}

#[test]
fn explains_uiautomator_empty_root_failures() {
    let error =
        normalize_uiautomator_error("ERROR: null root node returned by UiTestAutomationBridge.");
    assert!(error.contains("ANR"));
    assert!(error.contains("解锁"));
    assert!(error.contains("UiTestAutomationBridge"));
    assert_eq!(normalize_uiautomator_error(" "), "设备没有返回错误详情。");
}

#[test]
fn rejects_ignored_winscope_shell_commands_even_with_a_zero_exit_code() {
    let ignored = winscope_trace_command_error(
        true,
        "Shell commands are ignored. Any type of action should be performed through perfetto.",
        "",
    )
    .unwrap();
    assert!(ignored.contains("Perfetto"));
    assert!(winscope_trace_command_error(true, "", "").is_none());
    assert!(winscope_trace_command_error(false, "", "permission denied")
        .unwrap()
        .contains("permission denied"));
}

#[test]
fn summarizes_replaced_and_appended_workspace_lines() {
    assert_eq!(
        changed_line_counts("one\ntwo\nthree", "one\nchanged\nthree\nfour"),
        (2, 1)
    );
    assert_eq!(changed_line_counts("one\ntwo", "one\ntwo"), (0, 0));
}

#[test]
fn compares_zip_entries_by_content_and_preserves_manifest_status() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("android-tool-compare-{suffix}"));
    fs::create_dir_all(&root).unwrap();
    let left = root.join("left.apk");
    let right = root.join("right.apk");
    for (path, entries) in [
        (
            &left,
            [
                ("AndroidManifest.xml", "left"),
                ("same.txt", "same"),
                ("removed.txt", "old"),
            ],
        ),
        (
            &right,
            [
                ("AndroidManifest.xml", "right"),
                ("same.txt", "same"),
                ("added.txt", "new"),
            ],
        ),
    ] {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (name, content) in entries {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }
    let result = compare_android_archives(ArchiveCompareRequest {
        left_path: left.to_string_lossy().to_string(),
        right_path: right.to_string_lossy().to_string(),
    })
    .unwrap();
    assert_eq!(result.added, vec!["added.txt"]);
    assert_eq!(result.removed, vec!["removed.txt"]);
    assert_eq!(result.changed, vec!["AndroidManifest.xml"]);
    assert_eq!(result.unchanged_count, 1);
    assert_eq!(result.manifest_changed, Some(true));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn avoids_loading_an_oversized_embedded_aar_jar_into_memory() {
    assert!(should_index_embedded_aar_jar(
        true,
        "classes.jar",
        MAX_AAR_EMBEDDED_JAR_INDEX_BYTES
    ));
    assert!(!should_index_embedded_aar_jar(
        true,
        "classes.jar",
        MAX_AAR_EMBEDDED_JAR_INDEX_BYTES + 1
    ));
    assert!(!should_index_embedded_aar_jar(false, "classes.jar", 1));
    assert!(!should_index_embedded_aar_jar(true, "libs/classes.jar", 1));
}

#[test]
fn keeps_only_the_latest_requested_logcat_lines() {
    let lines = read_bounded_logcat_tail(BufReader::new(Cursor::new("one\ntwo\nthree\nfour\n")), 2);
    assert_eq!(lines, vec!["three", "four"]);
}

#[test]
fn retains_only_trailing_process_output_when_a_limit_is_reached() {
    let output = read_process_stream_tail(Cursor::new(b"abcdefgh"), 4);
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "[process output truncated; retained trailing diagnostics]\nefgh"
    );
}

#[test]
fn accepts_only_real_png_screenshot_bytes() {
    assert!(is_png_bytes(&[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0
    ]));
    assert!(!is_png_bytes(b"Assertion failed: renderer unavailable"));
    assert!(!is_png_bytes(&[0x89, b'P', b'N']));
}

#[test]
fn simpleperf_progress_uses_elapsed_time_and_stays_in_capture_range() {
    assert_eq!(simpleperf_capture_progress(10, 0), 15);
    assert_eq!(simpleperf_capture_progress(10, 5), 45);
    assert_eq!(simpleperf_capture_progress(10, 10), 75);
    assert_eq!(simpleperf_capture_progress(10, 99), 75);
    assert_eq!(simpleperf_capture_progress(0, 0), 15);
}

#[test]
fn perfetto_progress_uses_elapsed_time_and_stays_in_capture_range() {
    assert_eq!(perfetto_capture_progress(10, 0), 15);
    assert_eq!(perfetto_capture_progress(10, 5), 50);
    assert_eq!(perfetto_capture_progress(10, 10), 85);
    assert_eq!(perfetto_capture_progress(10, 99), 85);
    assert_eq!(perfetto_capture_progress(0, 0), 15);
}

#[test]
fn performance_support_probe_keeps_large_dumpsys_output_bounded() {
    let output = bounded_performance_support_output("x".repeat(8 * 1024), String::new());
    assert!(output.len() < 5 * 1024);
    assert!(output.ends_with("[能力探测输出已截断]"));
}

#[test]
fn summarizes_only_the_art_trace_key_section() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = std::env::temp_dir().join(format!("android-tool-method-{suffix}.trace"));
    let mut bytes = vec![0, 1, 2, 3, 4, 5];
    bytes.extend_from_slice(b"*version\n3\ndata-file-overflow=false\n*threads\n1 main\n2 RenderThread\n*methods\n1 4 dev.example.MainActivity onCreate ()V MainActivity.kt\n2 4 dev.example.MainActivity render ()V MainActivity.kt\n3 4 dev.example.Renderer draw ()V Renderer.kt\n*end\n");
    fs::write(&file, bytes).unwrap();
    let summary = summarize_method_trace(&file).unwrap();
    assert_eq!(summary.version.as_deref(), Some("3"));
    assert_eq!(summary.thread_count, 2);
    assert_eq!(summary.method_count, 3);
    assert_eq!(summary.class_count, 2);
    assert_eq!(summary.data_file_overflow, Some(false));
    fs::remove_file(file).unwrap();
}

#[test]
fn parses_bounded_shark_object_graph_summary() {
    let summary: HeapSharkAnalysis = serde_json::from_str(r#"{"status":"ok","analysisDurationMillis":12,"applicationLeakCount":1,"libraryLeakCount":0,"unreachableObjectCount":2,"applicationLeaks":[{"signature":"abc","description":"Activity instance","retainedBytes":4096,"retainedObjects":3}],"libraryLeaks":[]}"#).unwrap();
    assert_eq!(summary.application_leak_count, 1);
    assert_eq!(summary.application_leaks[0].retained_bytes, 4096);
    assert_eq!(
        summary.application_leaks[0].description,
        "Activity instance"
    );
}

#[test]
fn summarizes_androidx_macrobenchmark_metrics_without_samples() {
    let source = br#"{"context":{"build":{"model":"test-device","type":"userdebug","version":{"sdk":34}}},"benchmarks":[{"name":"scroll","className":"ExampleBenchmark","metrics":{"jankyFrameCount":{"minimum":1,"median":2,"maximum":4,"runs":[1,2,4]}},"sampledMetrics":{"gfxFrameTime95thPercentileMs":{"P50":7,"P90":12,"P95":18,"P99":32,"runs":[[1,2]]}}}]}"#;
    let summary = summarize_android_benchmark_json("fixture.json".to_string(), source).unwrap();
    assert_eq!(summary.device_model.as_deref(), Some("test-device"));
    assert_eq!(summary.sdk, Some(34));
    assert_eq!(summary.benchmarks[0].metrics.len(), 2);
    assert_eq!(summary.benchmarks[0].metrics[0].median, Some(2.0));
    assert_eq!(summary.benchmarks[0].metrics[1].p95, Some(18.0));
}

#[test]
fn parses_only_androidx_reported_benchmark_artifacts() {
    let instruments = parse_android_instrumentations("instrumentation:com.example.benchmark/.Runner (target=com.example.app)\ninstrumentation:bad value\n");
    assert_eq!(instruments.len(), 1);
    assert_eq!(instruments[0].component, "com.example.benchmark/.Runner");
    assert_eq!(benchmark_result_remote_path("INSTRUMENTATION_RESULT: additionalTestOutputFile_com.example.app-benchmarkData.json=/storage/emulated/0/Android/media/com.example.app/com.example.app-benchmarkData.json"), Some("/storage/emulated/0/Android/media/com.example.app/com.example.app-benchmarkData.json".to_string()));
    assert_eq!(
        benchmark_result_remote_path("INSTRUMENTATION_RESULT: stream=ordinary test"),
        None
    );
}

#[test]
fn summarizes_hprof_container_without_loading_the_object_graph() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = std::env::temp_dir().join(format!("android-tool-hprof-{suffix}.hprof"));
    let mut bytes = b"JAVA PROFILE 1.0.3\0".to_vec();
    bytes.extend_from_slice(&4u32.to_be_bytes());
    bytes.extend_from_slice(&1234u64.to_be_bytes());
    bytes.push(0x01);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&3u32.to_be_bytes());
    bytes.extend_from_slice(b"abc");
    bytes.push(0x1c);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&5u32.to_be_bytes());
    bytes.extend_from_slice(&[1, 2, 3, 4, 5]);
    fs::write(&file, bytes).unwrap();
    let summary = summarize_hprof_container(&file).unwrap();
    assert_eq!(summary.format, "JAVA PROFILE 1.0.3");
    assert_eq!(summary.identifier_size, 4);
    assert_eq!(summary.record_count, 2);
    assert_eq!(summary.heap_segment_count, 1);
    assert_eq!(summary.heap_segment_bytes, 5);
    assert_eq!(summary.record_types.get("UTF8 字符串"), Some(&1));
    fs::remove_file(file).unwrap();
}

#[test]
fn only_offers_adb_install_for_standalone_aosp_apks() {
    assert!(aosp_apk_installability("out/target/product/emulator/testcases/Sample.apk", "APK").0);
    assert!(
        aosp_apk_installability(
            "out/target/product/emulator/testcases/ApiDemos/x86_64/ApiDemos.apk",
            "APK"
        )
        .0
    );
    assert!(aosp_apk_installability("out/soong/.intermediates/development/samples/ApiDemos/ApiDemos/android_common/hash/ApiDemos.apk", "Soong APK").0);
    assert!(!aosp_apk_installability("out/soong/.intermediates/development/samples/ApiDemos/ApiDemos/android_common/hash/ApiDemos-unsigned.apk", "Soong APK").0);
    assert!(!aosp_apk_installability("out/soong/.intermediates/development/samples/ApiDemos/ApiDemos/android_common/hash/aapt2/package-res.apk", "Soong APK").0);
    assert!(
        !aosp_apk_installability(
            "out/target/product/emulator/system/priv-app/Settings/Settings.apk",
            "APK"
        )
        .0
    );
    assert!(
        !aosp_apk_installability(
            "out/target/product/emulator/obj/APPS/Sample_intermediates/package.apk",
            "APK"
        )
        .0
    );
    assert!(!aosp_apk_installability("out/target/product/emulator/system.img", "镜像").0);
}

#[test]
fn accepts_only_listed_soong_sample_apks_for_staging() {
    assert!(checked_aosp_artifact_path("out/soong/.intermediates/development/samples/ApiDemos/ApiDemos/android_common/hash/ApiDemos.apk").is_ok());
    assert!(checked_aosp_artifact_path(
        "out/soong/.intermediates/frameworks/base/core/core/android_common/hash/core.jar.apk"
    )
    .is_err());
    assert!(checked_aosp_artifact_path(
        "out/soong/.intermediates/development/samples/ApiDemos/../ApiDemos.apk"
    )
    .is_err());
}

#[test]
fn describes_xml_attributes_and_smali_methods() {
    let manifest_before =
        r#"<manifest package="dev.example"><application android:allowBackup="true" /></manifest>"#;
    let manifest_after =
        r#"<manifest package="dev.example"><application android:allowBackup="false" /></manifest>"#;
    assert!(
        workspace_change_detail("AndroidManifest.xml", manifest_before, manifest_after)
            .contains("application@android:allowBackup")
    );
    assert!(workspace_change_detail(
        "smali/a.smali",
        ".method public one()V\n.end method",
        ".method public two()V\n.end method"
    )
    .contains("方法 +1 / -1"));
}

#[test]
fn exposes_structured_xml_resource_and_smali_changes() {
    let xml = workspace_structured_changes(
        "res/values/strings.xml",
        "<resources><string name=\"title\">旧标题</string></resources>",
        "<resources><string name=\"title\">新标题</string></resources>",
    );
    assert!(xml.iter().any(|change| change.kind == "资源键"
        && change.key == "string#title"
        && change.before.as_deref() == Some("旧标题")
        && change.after.as_deref() == Some("新标题")));
    let smali = workspace_structured_changes(
        "smali/a.smali",
        ".method public one()V\n.end method",
        ".method public two()V\n.end method",
    );
    assert!(smali.iter().any(|change| change.kind == "smali 方法"
        && change.key.contains("one")
        && change.after.is_none()));
    assert!(smali.iter().any(|change| change.kind == "smali 方法"
        && change.key.contains("two")
        && change.before.is_none()));
}

#[test]
fn repairs_empty_apktool_resource_package_from_manifest() {
    let package_name = manifest_package_name("<manifest package=\"dev.example.fixture\" xmlns:android=\"http://schemas.android.com/apk/res/android\">").unwrap();
    assert_eq!(package_name, "dev.example.fixture");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("android-tool-apktool-{suffix}"));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("apktool.yml"),
        "resourcesInfo:\n  packageId: 0\n  packageName: \ndoNotCompress:\n- arsc\n",
    )
    .unwrap();
    assert!(
        repair_apktool_resource_package(&root, "<manifest package=\"dev.example.fixture\">")
            .unwrap()
    );
    assert!(fs::read_to_string(root.join("apktool.yml"))
        .unwrap()
        .contains("packageName: dev.example.fixture"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_only_safe_native_frames_from_tombstones() {
    let frames = tombstone_frames("      #00 pc 0000000000002a30  /system/bin/app_process64\n      #01 pc invalid /system/bin/ignored\n      #02 pc 10 /system/../etc/blocked\n");
    assert_eq!(
        frames,
        vec![(
            "0000000000002a30".to_string(),
            "/system/bin/app_process64".to_string()
        )]
    );
}

#[test]
fn native_symbolizer_cancellation_stops_before_spawning_a_command() {
    let cancellation = AtomicBool::new(true);
    let error = native_command_output(
        Command::new("android-tool-command-must-not-start"),
        Some(&cancellation),
        Duration::from_secs(1),
        1024,
    )
    .unwrap_err();
    assert_eq!(error, "请求已取消");
}

#[test]
fn apk_build_cancellation_stops_between_external_steps() {
    let cancellation = AtomicBool::new(true);
    let error =
        ensure_archive_build_not_cancelled(Some(("apk-build-test", &cancellation))).unwrap_err();
    assert_eq!(error, "APK 构建已取消。");
    assert!(ensure_archive_build_not_cancelled(None).is_ok());
}

#[test]
fn resolves_native_fixture_when_local_prerequisites_are_available() {
    if find_llvm_addr2line().is_none() {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("native");
    let tombstone_path = root.join("tombstone-app-process.txt");
    let symbols_path = root.join("symbols");
    if !tombstone_path.is_file() || !symbols_path.is_dir() {
        return;
    }
    let tombstone = fs::read_to_string(tombstone_path).unwrap();
    let resolved = addr2line_fallback(&symbols_path, &tombstone).unwrap();
    assert!(resolved
        .iter()
        .any(|line| line.contains("app_process64+0x0000000000002a30") && line.contains("main")));
}
