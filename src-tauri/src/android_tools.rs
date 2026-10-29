use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::android_tasks::{self, AndroidTaskManager};
use crate::background_process::command as background_command;
use futures_util::{future::join_all, StreamExt};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;

const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_PREVIEW_BYTES: usize = 512 * 1024;
const MAX_ARCHIVE_SEARCH_RESULTS: usize = 120;
const MAX_ARCHIVE_SEARCH_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARCHIVE_SEARCH_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
// AAR inspection needs an in-memory buffer to inspect the nested classes.jar.
// Keep the package visible but do not turn a large library into a WebView-side
// memory spike merely to expand its class tree.
const MAX_AAR_EMBEDDED_JAR_INDEX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CLASS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LOGCAT_UI_QUEUE_LINES: usize = 4_096;
const MAX_PROCESS_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SCREENSHOT_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_NATIVE_TOMBSTONE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_NATIVE_SYMBOLIZER_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ADDR2LINE_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_AAPT_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_CLASS_DECOMPILED_BYTES: u64 = 2 * 1024 * 1024;
// The editor keeps a complete UTF-8 document in the WebView. Eight MiB covers
// typical XML/configuration files while avoiding large-log UI stalls.
const MAX_REMOTE_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENVIRONMENT_TOOL_DOWNLOAD_BYTES: u64 = 768 * 1024 * 1024;
const MAX_ENVIRONMENT_TOOL_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const ENVIRONMENT_TOOL_DOWNLOAD_PROGRESS_EVENT: &str = "environment-tool-download-progress";

static ENVIRONMENT_TOOL_CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVE_ENVIRONMENT_TOOL_DOWNLOADS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub mod query_commands;
mod types;
pub use types::*;
use types::{LogcatCapture, LogcatEndedEvent, LogcatEvent, ScrcpyCapture, ScreenRecordCapture};
mod environment;
mod node_runtime;
mod python_runtime;
#[cfg(test)]
use environment::EnvironmentToolConfig;
pub use environment::*;
pub(crate) use node_runtime::{inspect_node_runtime, node_npm_cli, validate_node_runtime};
use node_runtime::{
    find_node, node_download_checksum, node_environment_tool_release, node_version_supported,
    validate_node_npm, verify_node_archive,
};
pub(crate) use python_runtime::{find_python, inspect_python_runtime};
use python_runtime::{python_child_directories, python_environment_tool_release};

use environment::{configured_environment_tool_path, normalized_environment_tool_path};
mod tool_paths;
#[cfg(test)]
use tool_paths::apk_device_compatibility;
pub(crate) use tool_paths::*;
use tool_paths::{
    apk_package_name, check_apk_device_compatibility, debug_keystore, environment_tool_command,
    find_aapt2, find_android_build_tool, find_apktool, find_bundletool, find_gradle,
    find_hprof_converter, find_jadx, find_java, find_llvm_addr2line, find_ndk_stack, find_scrcpy,
    find_shark_runtime, find_trace_processor, require_success,
};
mod native_symbolizer;
#[cfg(test)]
use native_symbolizer::addr2line_fallback;
use native_symbolizer::native_command_output;
#[cfg(test)]
use native_symbolizer::tombstone_frames;
pub use native_symbolizer::*;
mod archive_support;
#[cfg(test)]
use archive_support::manifest_package_name;
use archive_support::{
    aar_embedded_entry, archive_history_store, archive_session_root, changed_line_counts,
    collect_editable_workspace_files, collect_jadx_sources, decoded_manifest_preview,
    decompile_class_bytes, decompile_class_entry, now_millis, preview_from_bytes,
    record_archive_opened, repair_apktool_resource_package, upsert_archive_history,
    workspace_change_category, workspace_change_detail, workspace_file_path,
    workspace_structured_changes,
};
mod logcat;
#[cfg(test)]
use logcat::read_bounded_logcat_tail;
use logcat::AndroidPackageKind;
pub use logcat::*;
mod transport;
#[cfg(test)]
use transport::read_process_stream_tail;
use transport::{
    adb_command_output, adb_command_output_with_limit, adb_output,
    android_service_unavailable_message, checked_adb_pairing_code, checked_adb_wireless_endpoint,
    command_output_with_timeout, command_output_with_timeout_limit, CapturedCommandOutput,
};
mod adb;
pub use adb::*;
#[cfg(test)]
use adb::{
    bounded_performance_support_output, checked_remote_path, is_png_bytes, normalize_install_error,
    parse_adb_remote_files,
};
use adb::{checked_package, checked_serial, install_adb_package_with_audit};
mod performance;
pub use performance::*;
#[cfg(test)]
use performance::{
    normalize_simpleperf_error, perfetto_capture_progress, simpleperf_capture_progress,
};
use performance::{normalize_uiautomator_error, winscope_trace_command_error};
mod profiling;
pub use profiling::*;

mod benchmark;
pub use benchmark::*;

#[cfg(test)]
use benchmark::{benchmark_result_remote_path, parse_android_instrumentations};
mod device_ui;
pub use device_ui::*;
mod aosp;
use aosp::aosp_shell_quote;
pub use aosp::*;
#[cfg(test)]
use aosp::{
    aosp_apk_installability, aosp_build_heartbeat_progress, checked_aosp_artifact_path,
    clean_aosp_lunch_output, parse_aosp_probe, read_aosp_output_chunk,
};
mod archives;
#[cfg(test)]
use archives::should_index_embedded_aar_jar;
pub use archives::*;
use archives::{archive_path, sha256};
mod apk_build;
pub use apk_build::*;

#[cfg(test)]
use apk_build::ensure_archive_build_not_cancelled;
mod archive_workspace;
pub use archive_workspace::*;

#[cfg(test)]
mod tests;

// Tauri command wrappers remain available at the established registration path.
pub(crate) use adb::{
    __cmd__capture_adb_screenshot, __cmd__connect_adb_wireless, __cmd__create_adb_directory,
    __cmd__delete_adb_remote_path, __cmd__disconnect_adb_wireless, __cmd__export_adb_package,
    __cmd__get_adb_diagnostic, __cmd__get_adb_diagnostic_summary,
    __cmd__get_adb_graphics_diagnostic, __cmd__get_android_performance_support_matrix,
    __cmd__install_adb_package, __cmd__pair_adb_wireless, __cmd__pull_adb_file,
    __cmd__push_adb_file, __cmd__remount_adb_system, __cmd__request_adb_root, __cmd__restart_adb,
    __cmd__restart_adb_device, __cmd__restore_adb_backup, __cmd__run_adb_device_action,
    __cmd__run_adb_package_action, __cmd__start_adb_screen_record,
    __cmd__start_device_health_capture, __cmd__stop_adb_screen_record,
    __cmd__write_adb_remote_text, __tauri_command_name_capture_adb_screenshot,
    __tauri_command_name_connect_adb_wireless, __tauri_command_name_create_adb_directory,
    __tauri_command_name_delete_adb_remote_path, __tauri_command_name_disconnect_adb_wireless,
    __tauri_command_name_export_adb_package, __tauri_command_name_get_adb_diagnostic,
    __tauri_command_name_get_adb_diagnostic_summary,
    __tauri_command_name_get_adb_graphics_diagnostic,
    __tauri_command_name_get_android_performance_support_matrix,
    __tauri_command_name_install_adb_package, __tauri_command_name_pair_adb_wireless,
    __tauri_command_name_pull_adb_file, __tauri_command_name_push_adb_file,
    __tauri_command_name_remount_adb_system, __tauri_command_name_request_adb_root,
    __tauri_command_name_restart_adb, __tauri_command_name_restart_adb_device,
    __tauri_command_name_restore_adb_backup, __tauri_command_name_run_adb_device_action,
    __tauri_command_name_run_adb_package_action, __tauri_command_name_start_adb_screen_record,
    __tauri_command_name_start_device_health_capture, __tauri_command_name_stop_adb_screen_record,
    __tauri_command_name_write_adb_remote_text,
};
pub(crate) use aosp::{
    __cmd__discover_aosp_linux_workspaces, __cmd__inspect_aosp_workspace,
    __cmd__list_aosp_build_artifacts, __cmd__list_aosp_directories, __cmd__list_aosp_lunch_targets,
    __cmd__probe_aosp_linux_workspace, __cmd__read_aosp_build_log, __cmd__read_aosp_live_output,
    __cmd__start_aosp_artifact_install, __cmd__start_aosp_linux_build,
    __tauri_command_name_discover_aosp_linux_workspaces,
    __tauri_command_name_inspect_aosp_workspace, __tauri_command_name_list_aosp_build_artifacts,
    __tauri_command_name_list_aosp_directories, __tauri_command_name_list_aosp_lunch_targets,
    __tauri_command_name_probe_aosp_linux_workspace, __tauri_command_name_read_aosp_build_log,
    __tauri_command_name_read_aosp_live_output, __tauri_command_name_start_aosp_artifact_install,
    __tauri_command_name_start_aosp_linux_build,
};
pub(crate) use apk_build::{
    __cmd__begin_apk_edit_session, __cmd__build_signed_apk, __cmd__install_archive_build,
    __cmd__list_archive_history, __cmd__read_archive_build_result, __cmd__start_apk_edit_session,
    __cmd__start_archive_build_install, __cmd__start_signed_apk_build,
    __tauri_command_name_begin_apk_edit_session, __tauri_command_name_build_signed_apk,
    __tauri_command_name_install_archive_build, __tauri_command_name_list_archive_history,
    __tauri_command_name_read_archive_build_result, __tauri_command_name_start_apk_edit_session,
    __tauri_command_name_start_archive_build_install, __tauri_command_name_start_signed_apk_build,
};
pub(crate) use archive_workspace::{
    __cmd__build_apks_from_aab, __cmd__decompile_apk_with_jadx, __cmd__delete_archive_history,
    __cmd__inspect_jadx_workspace, __cmd__install_apks_with_bundletool,
    __cmd__list_apk_workspace_files, __cmd__read_apk_workspace_file,
    __cmd__read_archive_original_manifest, __cmd__read_jadx_source, __cmd__resume_apk_edit_session,
    __cmd__start_apks_from_aab_build, __cmd__start_apks_with_bundletool_install,
    __cmd__start_jadx_decompile, __cmd__summarize_apk_workspace_diff,
    __cmd__write_apk_workspace_file, __tauri_command_name_build_apks_from_aab,
    __tauri_command_name_decompile_apk_with_jadx, __tauri_command_name_delete_archive_history,
    __tauri_command_name_inspect_jadx_workspace, __tauri_command_name_install_apks_with_bundletool,
    __tauri_command_name_list_apk_workspace_files, __tauri_command_name_read_apk_workspace_file,
    __tauri_command_name_read_archive_original_manifest, __tauri_command_name_read_jadx_source,
    __tauri_command_name_resume_apk_edit_session, __tauri_command_name_start_apks_from_aab_build,
    __tauri_command_name_start_apks_with_bundletool_install,
    __tauri_command_name_start_jadx_decompile, __tauri_command_name_summarize_apk_workspace_diff,
    __tauri_command_name_write_apk_workspace_file,
};
pub(crate) use archives::{
    __cmd__compare_android_archives, __cmd__extract_android_archive,
    __cmd__inspect_android_archive, __cmd__preview_android_archive_entry,
    __cmd__read_android_archive_compare_result, __cmd__read_android_archive_inspection,
    __cmd__search_android_archive_entries, __cmd__start_android_archive_compare,
    __cmd__start_android_archive_extract, __cmd__start_android_archive_inspection,
    __tauri_command_name_compare_android_archives, __tauri_command_name_extract_android_archive,
    __tauri_command_name_inspect_android_archive,
    __tauri_command_name_preview_android_archive_entry,
    __tauri_command_name_read_android_archive_compare_result,
    __tauri_command_name_read_android_archive_inspection,
    __tauri_command_name_search_android_archive_entries,
    __tauri_command_name_start_android_archive_compare,
    __tauri_command_name_start_android_archive_extract,
    __tauri_command_name_start_android_archive_inspection,
};
pub(crate) use benchmark::{
    __cmd__import_android_benchmark_result, __cmd__list_android_instrumentations,
    __cmd__read_android_benchmark_result, __cmd__start_android_benchmark_run,
    __cmd__start_crash_diagnostics, __tauri_command_name_import_android_benchmark_result,
    __tauri_command_name_list_android_instrumentations,
    __tauri_command_name_read_android_benchmark_result,
    __tauri_command_name_start_android_benchmark_run, __tauri_command_name_start_crash_diagnostics,
};
pub(crate) use device_ui::{
    __cmd__dump_uiautomator_hierarchy, __cmd__install_split_apks, __cmd__start_scrcpy,
    __cmd__start_split_apks_install, __cmd__start_uiautomator_hierarchy_dump,
    __cmd__start_winscope_capture, __cmd__stop_scrcpy, __cmd__stop_winscope_capture,
    __tauri_command_name_dump_uiautomator_hierarchy, __tauri_command_name_install_split_apks,
    __tauri_command_name_start_scrcpy, __tauri_command_name_start_split_apks_install,
    __tauri_command_name_start_uiautomator_hierarchy_dump,
    __tauri_command_name_start_winscope_capture, __tauri_command_name_stop_scrcpy,
    __tauri_command_name_stop_winscope_capture,
};
pub(crate) use environment::{
    __cmd__check_environment_tool, __cmd__download_environment_tool,
    __cmd__get_environment_network_settings, __cmd__list_environment_tools,
    __cmd__save_environment_network_settings, __cmd__save_environment_tool_path,
    __tauri_command_name_check_environment_tool, __tauri_command_name_download_environment_tool,
    __tauri_command_name_get_environment_network_settings,
    __tauri_command_name_list_environment_tools,
    __tauri_command_name_save_environment_network_settings,
    __tauri_command_name_save_environment_tool_path,
};
pub(crate) use logcat::{
    __cmd__add_logcat_bookmark, __cmd__delete_logcat_history, __cmd__export_logcat_content,
    __cmd__import_logcat_history, __cmd__list_logcat_bookmarks, __cmd__list_logcat_history,
    __cmd__query_logcat_evidence, __cmd__read_logcat_history, __cmd__start_logcat,
    __cmd__stop_logcat, __tauri_command_name_add_logcat_bookmark,
    __tauri_command_name_delete_logcat_history, __tauri_command_name_export_logcat_content,
    __tauri_command_name_import_logcat_history, __tauri_command_name_list_logcat_bookmarks,
    __tauri_command_name_list_logcat_history, __tauri_command_name_query_logcat_evidence,
    __tauri_command_name_read_logcat_history, __tauri_command_name_start_logcat,
    __tauri_command_name_stop_logcat,
};
pub(crate) use native_symbolizer::{
    __cmd__read_native_symbolization_report, __cmd__start_native_tombstone_symbolization,
    __tauri_command_name_read_native_symbolization_report,
    __tauri_command_name_start_native_tombstone_symbolization,
};
pub(crate) use performance::{
    __cmd__diagnose_perfetto_trace, __cmd__read_simpleperf_report, __cmd__run_perfetto_sql,
    __cmd__start_adb_bugreport, __cmd__start_method_profile_capture, __cmd__start_perfetto_capture,
    __cmd__start_simpleperf_capture, __tauri_command_name_diagnose_perfetto_trace,
    __tauri_command_name_read_simpleperf_report, __tauri_command_name_run_perfetto_sql,
    __tauri_command_name_start_adb_bugreport, __tauri_command_name_start_method_profile_capture,
    __tauri_command_name_start_perfetto_capture, __tauri_command_name_start_simpleperf_capture,
};
pub(crate) use profiling::{
    __cmd__analyze_heap_dump, __cmd__read_heap_dump_report, __cmd__read_heap_shark_analysis,
    __cmd__read_method_profile_report, __cmd__start_heap_dump, __cmd__start_heap_shark_analysis,
    __tauri_command_name_analyze_heap_dump, __tauri_command_name_read_heap_dump_report,
    __tauri_command_name_read_heap_shark_analysis, __tauri_command_name_read_method_profile_report,
    __tauri_command_name_start_heap_dump, __tauri_command_name_start_heap_shark_analysis,
};
