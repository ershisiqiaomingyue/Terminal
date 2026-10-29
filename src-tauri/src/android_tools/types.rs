use super::*;

#[derive(Default)]
pub struct AndroidToolState {
    pub(super) logcat: Arc<Mutex<HashMap<String, LogcatCapture>>>,
    pub(super) screen_records: Arc<Mutex<HashMap<String, ScreenRecordCapture>>>,
    pub(super) scrcpy: Arc<Mutex<HashMap<String, ScrcpyCapture>>>,
    pub(super) winscope: Arc<Mutex<HashMap<String, String>>>,
}

pub(super) struct LogcatCapture {
    pub(super) owner: String,
    pub(super) output: Arc<Mutex<crate::terminal_output::TerminalOutput>>,
    pub(super) child: Arc<Mutex<Child>>,
    pub(super) session_id: String,
    pub(super) task_id: String,
    pub(super) stop_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
pub(super) struct ScreenRecordCapture {
    pub(super) child: Arc<Mutex<Child>>,
    pub(super) remote: String,
    pub(super) task_id: String,
}

#[derive(Clone)]
pub(super) struct ScrcpyCapture {
    pub(super) child: Arc<Mutex<Child>>,
    pub(super) task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,
    pub product: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
    pub transport_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDeviceInfo {
    pub android_version: String,
    pub sdk: String,
    pub manufacturer: String,
    pub model: String,
    pub build_fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDeviceStatus {
    pub battery_level: String,
    pub battery_status: String,
    pub battery_temperature: String,
    pub storage: String,
    pub abi: String,
    pub security_patch: String,
    pub system_services_ready: bool,
    pub system_services_message: String,
}

/// Small, version-tolerant metrics extracted from selected `dumpsys` reports.
/// The original command output remains available for evidence because Android
/// services vary by release and vendor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDiagnosticSummary {
    pub report: String,
    pub metrics: BTreeMap<String, String>,
    pub raw_size: usize,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidPerformanceCapability {
    pub name: String,
    pub available: bool,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidPerformanceSupportMatrix {
    pub serial: String,
    pub capabilities: Vec<AndroidPerformanceCapability>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbApplication {
    pub package_name: String,
    pub source_path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbPackageDetails {
    pub package_name: String,
    pub version_name: String,
    pub version_code: String,
    pub uid: String,
    pub enabled: String,
    pub source_paths: Vec<String>,
    pub requested_permissions: Vec<String>,
    pub permissions: Vec<AdbPermission>,
    pub enabled_state: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbPermission {
    pub name: String,
    pub granted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbStorageInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbCaptureInfo {
    pub physical_size: Option<String>,
    pub physical_density: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbPackageExportRequest {
    pub serial: String,
    pub package_name: String,
    pub destination: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbPackageInstallRequest {
    pub serial: String,
    pub apk_path: String,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub allow_test: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbGraphicsDiagnosticRequest {
    pub serial: String,
    #[serde(default)]
    pub package_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbWirelessPairRequest {
    pub endpoint: String,
    pub pairing_code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbWirelessConnectRequest {
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbRemoteFile {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbPrivilegeStatus {
    pub is_root: bool,
    pub build_type: String,
    pub debuggable: bool,
    pub secure: bool,
    pub selinux: String,
    pub verified_boot_state: String,
    pub bootloader_locked: Option<bool>,
    pub oem_unlock_supported: Option<bool>,
    pub slot_suffix: String,
    pub can_request_root: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbRemotePathRequest {
    pub serial: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDeleteRequest {
    pub serial: String,
    pub path: String,
    pub is_directory: bool,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbBackupEntry {
    pub id: String,
    pub serial: String,
    pub remote_path: String,
    pub name: String,
    pub is_directory: bool,
    pub created_at: String,
    pub local_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbBackupRestoreRequest {
    pub serial: String,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbTransferRequest {
    pub serial: String,
    pub remote_path: String,
    pub local_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbRemoteText {
    pub content: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbWriteTextRequest {
    pub serial: String,
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub confirm_protected: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbScreenRecordStopRequest {
    pub serial: String,
    pub destination: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbProcess {
    pub pid: String,
    pub user: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbShellResult {
    pub serial: String,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    pub path: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveInspection {
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub size: u64,
    pub sha256: String,
    pub entries: Vec<ArchiveEntry>,
    pub entry_count: usize,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveExtractRequest {
    pub archive_path: String,
    pub destination: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveExtractResult {
    pub destination: String,
    pub files_extracted: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntryPreviewRequest {
    pub archive_path: String,
    pub entry_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntryPreview {
    pub path: String,
    pub size: u64,
    pub kind: String,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSearchRequest {
    pub archive_path: String,
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSearchMatch {
    pub entry_path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub preview: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSearchResult {
    pub matches: Vec<ArchiveSearchMatch>,
    pub scanned_entries: usize,
    pub skipped_entries: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveCompareRequest {
    pub left_path: String,
    pub right_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveCompareResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub unchanged_count: usize,
    pub manifest_changed: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEditSessionRequest {
    pub archive_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEditSession {
    pub id: String,
    pub archive_path: String,
    pub workspace_path: String,
    pub manifest: String,
    pub apktool_version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveBuildRequest {
    pub session_id: String,
    pub manifest: String,
    pub destination: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveBuildInstallRequest {
    pub session_id: String,
    pub serial: String,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveBuildResult {
    pub output_path: String,
    pub verification: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveHistoryRecord {
    pub id: String,
    pub archive_path: String,
    pub file_name: String,
    pub sha256: String,
    pub opened_at: i64,
    pub updated_at: i64,
    pub workspace_path: Option<String>,
    pub apktool_version: Option<String>,
    pub manifest_changed: bool,
    pub output_path: Option<String>,
    pub verification: Option<String>,
    pub installed_serial: Option<String>,
    pub install_result: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveHistoryDeleteRequest {
    pub id: String,
    #[serde(default)]
    pub delete_workspace: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveHistoryResumeRequest {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolQuery {
    #[serde(default)]
    pub include_remote: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolPathRequest {
    pub tool_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolDownloadRequest {
    pub tool_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentNetworkSettingsRequest {
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub proxy_enabled: Option<bool>,
    #[serde(default)]
    pub proxy_child_processes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentNetworkSettings {
    pub proxy_url: Option<String>,
    pub proxy_enabled: bool,
    pub proxy_child_processes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub local_path: Option<String>,
    pub local_version: Option<String>,
    pub local_status: String,
    pub local_message: String,
    pub remote_version: Option<String>,
    pub remote_message: Option<String>,
    pub supports_download: bool,
    pub selection_hint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolDownloadProgress {
    pub tool_id: String,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JadxDecompileRequest {
    pub archive_path: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JadxSourceRequest {
    pub workspace_path: String,
    pub relative_path: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JadxDecompileResult {
    pub workspace_path: String,
    pub version: String,
    pub files: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileRequest {
    pub session_id: String,
    pub relative_path: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWriteRequest {
    pub session_id: String,
    pub relative_path: String,
    pub content: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    pub path: String,
    pub size: u64,
    pub changed: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileDiff {
    pub original: String,
    pub current: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeEntry {
    pub path: String,
    pub category: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub detail: String,
    pub changes: Vec<WorkspaceStructuredChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStructuredChange {
    pub kind: String,
    pub key: String,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeSummary {
    pub changed_files: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub entries: Vec<WorkspaceChangeEntry>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundletoolBuildRequest {
    pub aab_path: String,
    pub destination: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundletoolInstallRequest {
    pub serial: String,
    pub apks_path: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BugreportRequest {
    pub serial: String,
    pub destination: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHealthCaptureRequest {
    pub serial: String,
    pub destination: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfettoCaptureRequest {
    pub serial: String,
    pub destination: String,
    pub duration_seconds: u32,
    pub preset: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfettoSqlRequest {
    pub trace_path: String,
    pub query: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfettoDiagnosis {
    pub trace_path: String,
    pub sections: Vec<PerfettoDiagnosisSection>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfettoDiagnosisSection {
    pub title: String,
    pub query: String,
    pub output: String,
    pub available: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleperfCaptureRequest {
    pub serial: String,
    pub destination: String,
    pub duration_seconds: u32,
    #[serde(default)]
    pub package_name: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleperfReport {
    pub task_id: String,
    pub csv: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodProfileCaptureRequest {
    pub serial: String,
    pub destination: String,
    pub package_name: String,
    pub duration_seconds: u32,
    #[serde(default = "default_method_profile_interval_us")]
    pub sampling_interval_us: u32,
}
pub(super) fn default_method_profile_interval_us() -> u32 {
    1_000
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodTraceSummary {
    pub version: Option<String>,
    pub thread_count: usize,
    pub method_count: usize,
    pub class_count: usize,
    pub data_file_overflow: Option<bool>,
    pub limitation: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodProfileReport {
    pub task_id: String,
    pub trace_path: String,
    pub trace_size: u64,
    pub metadata: String,
    pub summary: MethodTraceSummary,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapDumpRequest {
    pub serial: String,
    pub destination: String,
    pub package_name: String,
    #[serde(default)]
    pub exclude_zygote: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapDumpReport {
    pub task_id: String,
    pub android_hprof_path: String,
    pub standard_hprof_path: String,
    pub android_hprof_size: u64,
    pub standard_hprof_size: u64,
    pub metadata: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapDumpContainerSummary {
    pub format: String,
    pub identifier_size: u32,
    pub timestamp_millis: u64,
    pub record_count: u64,
    pub heap_segment_count: u64,
    pub heap_segment_bytes: u64,
    pub record_types: BTreeMap<String, u64>,
    pub limitation: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapDumpAnalysis {
    pub task_id: String,
    pub standard_hprof_path: String,
    pub standard_hprof_size: u64,
    pub summary: HeapDumpContainerSummary,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapSharkLeak {
    pub signature: String,
    pub description: String,
    pub retained_bytes: i64,
    pub retained_objects: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapSharkAnalysis {
    pub status: String,
    pub analysis_duration_millis: u64,
    pub application_leak_count: usize,
    pub library_leak_count: usize,
    pub unreachable_object_count: usize,
    pub application_leaks: Vec<HeapSharkLeak>,
    pub library_leaks: Vec<HeapSharkLeak>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBenchmarkMetric {
    pub name: String,
    pub median: Option<f64>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBenchmarkCase {
    pub name: String,
    pub class_name: String,
    pub metrics: Vec<AndroidBenchmarkMetric>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBenchmarkSummary {
    pub source_path: String,
    pub device_model: Option<String>,
    pub sdk: Option<u64>,
    pub build_type: Option<String>,
    pub benchmarks: Vec<AndroidBenchmarkCase>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidInstrumentation {
    pub component: String,
    pub target_package: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidBenchmarkRunRequest {
    pub serial: String,
    pub instrumentation: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashDiagnosticsRequest {
    pub serial: String,
    pub destination: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSymbolizeRequest {
    pub tombstone_path: String,
    pub symbols_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSymbolizeResult {
    pub output: String,
    pub ndk_stack_symbol_count: usize,
    pub addr2line_symbol_count: usize,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinscopeCaptureStopRequest {
    pub serial: String,
    pub destination: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinscopeCaptureResult {
    pub path: String,
    pub name: String,
    pub size: u64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitApkInstallRequest {
    pub serial: String,
    pub apk_paths: Vec<String>,
    #[serde(default)]
    pub replace: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiHierarchyDumpRequest {
    pub serial: String,
    pub destination: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrcpyRequest {
    pub serial: String,
    #[serde(default)]
    pub no_audio: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospWorkspaceInfo {
    pub root: String,
    pub envsetup_path: Option<String>,
    pub soong_path: Option<String>,
    pub android_bp_files: Vec<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospBuildLogAnalysis {
    pub path: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospBuildArtifact {
    pub path: String,
    pub size: u64,
    pub kind: String,
    pub installable: bool,
    pub install_note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AospLinuxTargetRequest {
    /// Only configured Linux execution targets are accepted. Windows is the
    /// coordinator and never sources AOSP environment scripts.
    pub target_kind: String,
    pub target_id: String,
    pub root: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AospWorkspaceDiscoveryRequest {
    pub target_kind: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AospLinuxBuildRequest {
    #[serde(flatten)]
    pub target: AospLinuxTargetRequest,
    #[serde(default)]
    pub lunch_target: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub custom_command: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AospArtifactInstallRequest {
    #[serde(flatten)]
    pub target: AospLinuxTargetRequest,
    pub serial: String,
    pub artifact_path: String,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospLinuxProbe {
    pub target_kind: String,
    pub target_id: String,
    pub root: String,
    pub linux: bool,
    pub bash_available: bool,
    pub envsetup_path: Option<String>,
    pub soong_path: Option<String>,
    pub manifest_path: Option<String>,
    pub android_bp_count: Option<u32>,
    pub git_branch: Option<String>,
    pub ccache_status: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LogcatEvent {
    pub(super) sequence: u64,
    pub(super) session_id: String,
    pub(super) serial: String,
    pub(super) lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LogcatEndedEvent {
    pub(super) serial: String,
    pub(super) session_id: String,
    pub(super) reason: String,
    pub(super) recoverable: bool,
    pub(super) dropped_event_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatHistoryRecord {
    pub id: String,
    pub serial: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub line_count: i64,
    pub end_reason: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatHistoryReadRequest {
    pub id: String,
    #[serde(default = "default_logcat_limit")]
    pub limit: usize,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatHistoryDeleteRequest {
    pub id: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatExportRequest {
    pub destination: String,
    pub content: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatImportRequest {
    pub path: String,
    #[serde(default)]
    pub serial: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatBookmarkRequest {
    pub session_id: String,
    pub line: String,
    #[serde(default)]
    pub note: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatBookmark {
    pub id: i64,
    pub session_id: String,
    pub created_at: i64,
    pub line: String,
    pub note: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogcatEvidenceRequest {
    pub session_id: String,
    pub terms: Vec<String>,
    #[serde(default = "default_logcat_limit")]
    pub limit: usize,
}
pub(super) fn default_logcat_limit() -> usize {
    8000
}
