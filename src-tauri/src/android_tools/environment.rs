use super::*;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct EnvironmentToolConfig {
    #[serde(default)]
    pub(super) paths: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) proxy_url: Option<String>,
    #[serde(default)]
    pub(super) proxy_enabled: Option<bool>,
    #[serde(default)]
    pub(super) proxy_child_processes: bool,
}

impl EnvironmentToolConfig {
    pub(super) fn network_settings(&self) -> EnvironmentNetworkSettings {
        EnvironmentNetworkSettings {
            proxy_url: self.proxy_url.clone(),
            proxy_enabled: self.proxy_enabled.unwrap_or_else(|| {
                self.proxy_url
                    .as_deref()
                    .is_some_and(|url| !url.trim().is_empty())
            }),
            proxy_child_processes: self.proxy_child_processes,
        }
    }
}

pub(super) struct EnvironmentToolDownloadReservation {
    pub(super) tool_id: String,
}

impl Drop for EnvironmentToolDownloadReservation {
    fn drop(&mut self) {
        if let Ok(mut active) = environment_tool_downloads().lock() {
            active.remove(&self.tool_id);
        }
    }
}

pub(super) fn environment_tool_config_lock() -> &'static Mutex<()> {
    ENVIRONMENT_TOOL_CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn environment_tool_downloads() -> &'static Mutex<HashSet<String>> {
    ACTIVE_ENVIRONMENT_TOOL_DOWNLOADS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(super) fn reserve_environment_tool_download(
    tool_id: &str,
) -> Result<EnvironmentToolDownloadReservation, String> {
    let mut active = environment_tool_downloads()
        .lock()
        .map_err(|_| "工具下载状态不可用。".to_string())?;
    if !active.insert(tool_id.to_string()) {
        return Err("该工具正在下载，请等待当前任务完成。".to_string());
    }
    Ok(EnvironmentToolDownloadReservation {
        tool_id: tool_id.to_string(),
    })
}

pub(super) fn environment_tools_root() -> Result<PathBuf, String> {
    crate::user_storage::directory("tools")
}

pub(super) fn environment_tools_config_path() -> Option<PathBuf> {
    crate::user_storage::root().ok().map(|root| root.join("tools/environment-tools.json"))
}

pub(super) fn read_environment_tool_config() -> EnvironmentToolConfig {
    let Some(path) = environment_tools_config_path() else {
        return EnvironmentToolConfig::default();
    };
    fs::read(path)
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .unwrap_or_default()
}

pub(super) fn save_environment_tool_config(config: &EnvironmentToolConfig) -> Result<(), String> {
    let root = environment_tools_root()?;
    let path = root.join("environment-tools.json");
    let content = serde_json::to_vec_pretty(config)
        .map_err(|error| format!("无法序列化工具配置：{error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存工具配置：{error}"))
}

pub(super) fn update_environment_tool_config(
    update: impl FnOnce(&mut EnvironmentToolConfig),
) -> Result<(), String> {
    let _guard = environment_tool_config_lock()
        .lock()
        .map_err(|_| "工具配置状态不可用。".to_string())?;
    let mut config = read_environment_tool_config();
    update(&mut config);
    save_environment_tool_config(&config)
}

pub(crate) fn restore_environment_network_settings() -> Result<(), String> {
    let settings = read_environment_tool_config().network_settings();
    agent_core::set_http_proxy_settings(agent_core::HttpProxySettings::new(
        settings.proxy_url,
        settings.proxy_enabled,
        settings.proxy_child_processes,
    )?)
}

pub(crate) fn configured_http_client_builder() -> Result<reqwest::ClientBuilder, String> {
    agent_core::http_client_builder()
}

pub(super) fn configured_environment_tool_path(tool_id: &str) -> Option<PathBuf> {
    read_environment_tool_config()
        .paths
        .get(tool_id)
        .map(PathBuf::from)
        .map(|path| normalized_environment_tool_path(&path))
        .filter(|path| path.is_file())
        .or_else(|| crate::bundled_environment::tool_path(tool_id))
        .map(|path| normalized_environment_tool_path(&path))
}

/// Directories and variables that every child process should inherit so the
/// bundled toolchain is reachable from the agent and the integrated terminal,
/// not only from the desktop process itself.
pub(crate) fn bundled_child_environment() -> (Vec<PathBuf>, Vec<(String, String)>) {
    let mut directories = Vec::new();
    let mut variables = Vec::new();
    for (tool_id, subdirectory) in [
        ("adb", "adb"),
        ("aapt2", "aapt2"),
        ("jadx", "jadx/bin"),
        ("scrcpy", "scrcpy"),
    ] {
        if let Some(path) = configured_environment_tool_path(tool_id) {
            let root = crate::bundled_environment::root();
            // The user may point a tool at any location; only derive a PATH
            // entry when it actually sits inside the bundled tool tree.
            let directory = match root {
                Some(root) if path.starts_with(&root) => root.join(subdirectory),
                _ => path.parent().map(PathBuf::from).unwrap_or_default(),
            };
            if directory.is_dir() {
                directories.push(directory);
            }
        }
    }
    if let Some(java) = configured_environment_tool_path("java") {
        if let Some(bin) = java.parent() {
            directories.push(bin.to_path_buf());
            if let Some(home) = bin.parent() {
                variables.push(("JAVA_HOME".to_string(), home.to_string_lossy().to_string()));
            }
        }
    }
    if let Some(python) = configured_environment_tool_path("python") {
        directories.extend(python_child_directories(&python));
    }
    if let Some(node) = configured_environment_tool_path("node") {
        if let Some(parent) = node.parent() {
            directories.push(parent.to_path_buf());
        }
    }
    (directories, variables)
}

/// Same as [`bundled_child_environment`] shaped for `portable_pty`'s builder.
pub(crate) fn bundled_child_environment_variables() -> Vec<(String, String)> {
    let (directories, variables) = bundled_child_environment();
    let mut result = variables;
    if !directories.is_empty() {
        let mut entries = directories;
        if let Some(existing) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(entries) {
            result.push(("PATH".to_string(), path.to_string_lossy().to_string()));
        }
    }
    result
}

pub(crate) fn install_bundled_child_environment() -> Result<(), String> {
    let (directories, variables) = bundled_child_environment();
    agent_core::set_child_environment(directories, variables.into_iter().collect())
}

pub(super) fn known_environment_tool(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "adb"
            | "aapt2"
            | "java"
            | "apktool"
            | "jadx"
            | "bundletool"
            | "scrcpy"
            | "gradle"
            | "python"
            | "node"
    )
}

pub(super) fn environment_tool_metadata(
    tool_id: &str,
) -> Option<(&'static str, &'static str, &'static str)> {
    match tool_id {
        "node" => Some((
            "Node.js",
            "运行 WorkBuddy、OpenCode 等模型插件；便携版包含 npm，无需系统安装。",
            "选择 node.exe",
        )),
        "python" => Some((
            "Python 3",
            "运行 Skills 附带的 Python 脚本；便携版包含 pip，无需系统安装。",
            "选择 python.exe 或 python3.exe",
        )),
        "adb" => Some((
            "Android Platform-Tools",
            "ADB 设备连接、日志、安装与文件传输。",
            "选择 adb.exe",
        )),
        "aapt2" => Some((
            "Android Build-Tools (aapt2)",
            "读取 APK 包名、SDK 要求与资源编译信息。",
            "选择 aapt2.exe",
        )),
        "java" => Some((
            "Java 运行时",
            "运行 Apktool、Bundletool 与 Shark 分析辅助程序。",
            "选择 java.exe",
        )),
        "apktool" => Some((
            "Apktool",
            "解码、编辑和重编译 APK 资源与 Manifest。",
            "选择 apktool_*.jar",
        )),
        "jadx" => Some(("JADX", "APK、DEX 与 JAR 源码反编译。", "选择 jadx.bat")),
        "bundletool" => Some((
            "Bundletool",
            "安装和处理 .apks、App Bundle 产物。",
            "选择 bundletool-all-*.jar",
        )),
        "scrcpy" => Some(("Scrcpy", "Android 设备投屏与交互控制。", "选择 scrcpy.exe")),
        "gradle" => Some((
            "Gradle",
            "Android 项目构建工具。优先使用项目目录中的 Gradle Wrapper，应用会在执行时自动识别。",
            "选择 gradle.bat 或 gradle.cmd",
        )),
        _ => None,
    }
}

pub(super) fn environment_tool_path_is_valid(tool_id: &str, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match tool_id {
        "node" => name == "node.exe" || name == "node",
        "python" => matches!(
            name.as_str(),
            "python.exe" | "python3.exe" | "python" | "python3"
        ),
        "adb" => name == "adb.exe" || name == "adb",
        "aapt2" => name == "aapt2.exe" || name == "aapt2",
        "java" => name == "java.exe" || name == "java",
        "apktool" => name.starts_with("apktool_") && name.ends_with(".jar"),
        "jadx" => name == "jadx.bat" || name == "jadx",
        "bundletool" => name.starts_with("bundletool-all-") && name.ends_with(".jar"),
        "scrcpy" => name == "scrcpy.exe" || name == "scrcpy",
        "gradle" => {
            name == "gradle.bat" || name == "gradle.cmd" || name == "gradle.exe" || name == "gradle"
        }
        _ => false,
    }
}

pub(super) fn local_environment_tool_path(tool_id: &str) -> Option<PathBuf> {
    match tool_id {
        "node" => find_node(),
        "python" => find_python(),
        "adb" => find_adb().ok(),
        "aapt2" => find_aapt2(),
        "java" => find_java(),
        "apktool" => find_apktool(),
        "jadx" => find_jadx(),
        "bundletool" => find_bundletool(),
        "scrcpy" => find_scrcpy(),
        "gradle" => find_gradle(),
        _ => None,
    }
}

pub(super) fn normalized_environment_tool_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(unc_path) = value.strip_prefix("\\\\?\\UNC\\") {
            return PathBuf::from(format!("\\\\{unc_path}"));
        }
        if let Some(normal_path) = value.strip_prefix("\\\\?\\") {
            return PathBuf::from(normal_path);
        }
    }
    path.to_path_buf()
}

pub(super) fn display_environment_tool_path(path: &Path) -> String {
    normalized_environment_tool_path(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn compact_environment_tool_version(value: String) -> Option<String> {
    let line = value.lines().map(str::trim).find(|line| !line.is_empty())?;
    let compact = line.chars().take(160).collect::<String>();
    (!compact.is_empty()).then_some(compact)
}

pub(super) fn decode_environment_tool_output(bytes: &[u8]) -> String {
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_string();
    }
    #[cfg(windows)]
    {
        let (value, _, had_errors) = encoding_rs::GBK.decode(bytes);
        if !had_errors {
            return value.into_owned();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

pub(super) fn environment_tool_version(
    tool_id: &str,
    path: &Path,
) -> Result<Option<String>, String> {
    let command = match tool_id {
        "apktool" | "bundletool" => {
            let java =
                find_java().ok_or_else(|| "未找到 Java，无法读取 JAR 工具版本。".to_string())?;
            let mut command = Command::new(java);
            let version_argument = if tool_id == "bundletool" {
                "version"
            } else {
                "--version"
            };
            command.args(["-jar", path.to_string_lossy().as_ref(), version_argument]);
            command
        }
        "java" => {
            let mut command = Command::new(path);
            command.arg("-version");
            command
        }
        "aapt2" => {
            let mut command = Command::new(path);
            command.arg("version");
            command
        }
        _ => {
            let mut command = environment_tool_command(path);
            command.arg("--version");
            command
        }
    };
    let output = command_output_with_timeout_limit(command, Duration::from_secs(12), 128 * 1024)
        .map_err(|error| format!("读取版本失败：{error}"))?;
    let stdout = decode_environment_tool_output(&output.stdout);
    let stderr = decode_environment_tool_output(&output.stderr);
    let version_output = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let version = compact_environment_tool_version(version_output);
    if !output.status.success() {
        return Err(match version {
            Some(detail) => format!("工具版本命令执行失败：{detail}"),
            None => "工具版本命令执行失败，且没有返回诊断信息。".to_string(),
        });
    }
    if tool_id == "node" {
        if !version.as_deref().is_some_and(node_version_supported) {
            return Err("需要 Node 22.19+ 或 24+。".into());
        }
        validate_node_npm(path)?;
    }
    if tool_id == "python"
        && !version
            .as_deref()
            .is_some_and(|value| value.starts_with("Python 3."))
    {
        return Err("需要可正常运行的 Python 3 解释器。".into());
    }
    Ok(Some(
        version.ok_or_else(|| "工具没有返回版本信息。".to_string())?,
    ))
}

pub(super) fn inspect_local_environment_tool(
    tool_id: &str,
) -> Result<EnvironmentToolStatus, String> {
    let (name, description, selection_hint) =
        environment_tool_metadata(tool_id).ok_or_else(|| "未知环境工具。".to_string())?;
    let path = local_environment_tool_path(tool_id);
    let (local_status, local_message, local_version) = match path.as_deref() {
        Some(path) => match environment_tool_version(tool_id, path) {
            Ok(version) => (
                "ready".to_string(),
                "已检测到本地工具。".to_string(),
                version,
            ),
            Err(error) => ("available".to_string(), error, None),
        },
        None => (
            "missing".to_string(),
            if tool_id == "gradle" {
                "未检测到系统 Gradle；项目中的 gradlew / gradlew.bat 会在执行时自动识别。"
                    .to_string()
            } else {
                "尚未检测到本地工具。".to_string()
            },
            None,
        ),
    };
    Ok(EnvironmentToolStatus {
        id: tool_id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        local_path: path.as_deref().map(display_environment_tool_path),
        local_version,
        local_status,
        local_message,
        remote_version: None,
        remote_message: None,
        supports_download: tool_id != "gradle"
            && (tool_id != "python" || cfg!(all(target_os = "windows", target_arch = "x86_64")))
            && (tool_id != "node" || cfg!(all(target_os = "windows", target_arch = "x86_64"))),
        selection_hint: selection_hint.to_string(),
    })
}

pub(super) const ENVIRONMENT_TOOL_IDS: [&str; 10] = [
    "adb",
    "aapt2",
    "java",
    "apktool",
    "jadx",
    "bundletool",
    "scrcpy",
    "python",
    "node",
    "gradle",
];
pub(super) const REMOTE_ENVIRONMENT_TOOL_IDS: [&str; 9] = [
    "adb",
    "aapt2",
    "java",
    "apktool",
    "jadx",
    "bundletool",
    "scrcpy",
    "python",
    "node",
];
pub(super) const ANDROID_REPOSITORY_XML: &str =
    "https://dl.google.com/android/repository/repository2-1.xml";

#[derive(Debug, Clone, Copy)]
pub(super) enum EnvironmentToolPackageKind {
    File,
    Zip,
}

#[derive(Debug, Clone)]
pub(super) struct EnvironmentToolRemote {
    pub(super) version: String,
    pub(super) url: String,
    pub(super) file_name: String,
    pub(super) package_kind: EnvironmentToolPackageKind,
}

#[derive(Debug, Deserialize)]
pub(super) struct GitHubReleaseAsset {
    pub(super) name: String,
    pub(super) browser_download_url: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct GitHubRelease {
    pub(super) tag_name: String,
    pub(super) assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdoptiumPackage {
    pub(super) link: String,
    pub(super) name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdoptiumBinary {
    pub(super) package: AdoptiumPackage,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdoptiumVersion {
    pub(super) openjdk_version: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdoptiumRelease {
    pub(super) binary: AdoptiumBinary,
    pub(super) version: AdoptiumVersion,
}

pub(super) fn environment_http_client() -> Result<reqwest::Client, String> {
    configured_http_client_builder()?
        .user_agent("Android-Toolkit-Desktop/2.0")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("无法初始化工具网络请求：{error}"))
}

pub(super) fn environment_tool_download_client() -> Result<reqwest::Client, String> {
    configured_http_client_builder()?
        .user_agent("Android-Toolkit-Desktop/2.0")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|error| format!("无法初始化工具下载请求：{error}"))
}

#[tauri::command]
pub fn get_environment_network_settings() -> EnvironmentNetworkSettings {
    read_environment_tool_config().network_settings()
}

#[tauri::command]
pub fn save_environment_network_settings(
    request: EnvironmentNetworkSettingsRequest,
) -> Result<EnvironmentNetworkSettings, String> {
    let proxy_url = request
        .proxy_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let proxy_enabled = request.proxy_enabled.unwrap_or(proxy_url.is_some());
    let runtime_settings = agent_core::HttpProxySettings::new(
        proxy_url.clone(),
        proxy_enabled,
        request.proxy_child_processes,
    )?;
    let _guard = environment_tool_config_lock()
        .lock()
        .map_err(|_| "工具配置状态不可用。".to_string())?;
    let mut config = read_environment_tool_config();
    config.proxy_url = proxy_url;
    config.proxy_enabled = Some(proxy_enabled);
    config.proxy_child_processes = request.proxy_child_processes;
    save_environment_tool_config(&config)?;
    agent_core::set_http_proxy_settings(runtime_settings)?;
    Ok(config.network_settings())
}

pub(super) async fn github_environment_tool_release(
    repository: &str,
    asset_matches: impl Fn(&str) -> bool,
) -> Result<EnvironmentToolRemote, String> {
    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let release = environment_http_client()?
        .get(url)
        .send()
        .await
        .map_err(|error| format!("无法查询 GitHub Release：{error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub Release 请求失败：{error}"))?
        .json::<GitHubRelease>()
        .await
        .map_err(|error| format!("无法读取 GitHub Release：{error}"))?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset_matches(&asset.name))
        .ok_or_else(|| format!("远端版本 {} 没有适用于当前平台的安装包。", release.tag_name))?;
    Ok(EnvironmentToolRemote {
        version: release.tag_name,
        url: asset.browser_download_url,
        file_name: asset.name.clone(),
        package_kind: if asset.name.to_ascii_lowercase().ends_with(".zip") {
            EnvironmentToolPackageKind::Zip
        } else {
            EnvironmentToolPackageKind::File
        },
    })
}

pub(super) fn android_repository_package_version(
    content: &str,
    package_path: &str,
) -> Option<String> {
    let package = android_repository_package_block(content, package_path)?;
    let revision_pattern = Regex::new(r#"(?s)<revision>(?P<revision>.*?)</revision>"#).ok()?;
    let revision = revision_pattern
        .captures(package)?
        .name("revision")?
        .as_str();
    let component = |name: &str| {
        Regex::new(&format!(r#"(?s)<{name}>(\d+)</{name}>"#))
            .ok()?
            .captures(revision)?
            .get(1)
            .map(|value| value.as_str().to_string())
    };
    let major = component("major")?;
    let minor = component("minor").unwrap_or_else(|| "0".to_string());
    let micro = component("micro").unwrap_or_else(|| "0".to_string());
    Some(format!("{major}.{minor}.{micro}"))
}

pub(super) fn android_repository_package_block<'a>(
    content: &'a str,
    package_path: &str,
) -> Option<&'a str> {
    let marker = format!(r#"<remotePackage path=\"{package_path}\">"#);
    let start = content.find(&marker)?;
    let remaining = &content[start..];
    let end = remaining.find("</remotePackage>")? + "</remotePackage>".len();
    Some(&remaining[..end])
}

pub(super) fn android_repository_windows_archive_url(package: &str) -> Option<String> {
    let archive_pattern = Regex::new(
        r#"(?s)<archive>.*?<complete>.*?<url>(?P<url>[^<]+)</url>.*?</complete>\s*<host-os>windows</host-os>\s*</archive>"#,
    )
    .ok()?;
    archive_pattern.captures(package)?.name("url").map(|url| {
        format!(
            "https://dl.google.com/android/repository/{}",
            url.as_str().trim()
        )
    })
}

pub(super) async fn android_repository_environment_tool(
    tool_id: &str,
) -> Result<EnvironmentToolRemote, String> {
    let content = environment_http_client()?
        .get(ANDROID_REPOSITORY_XML)
        .send()
        .await
        .map_err(|error| format!("无法查询 Android SDK 仓库：{error}"))?
        .error_for_status()
        .map_err(|error| format!("Android SDK 仓库请求失败：{error}"))?
        .text()
        .await
        .map_err(|error| format!("无法读取 Android SDK 仓库：{error}"))?;
    match tool_id {
        "adb" => {
            let version = android_repository_package_version(&content, "platform-tools")
                .ok_or_else(|| "无法识别远端 Platform-Tools 版本。".to_string())?;
            Ok(EnvironmentToolRemote {
                version,
                url: "https://dl.google.com/android/repository/platform-tools-latest-windows.zip"
                    .to_string(),
                file_name: "platform-tools-latest-windows.zip".to_string(),
                package_kind: EnvironmentToolPackageKind::Zip,
            })
        }
        "aapt2" => {
            let package_pattern = Regex::new(
                r#"(?s)<remotePackage\s+path=\"build-tools;(?P<version>[0-9.]+)\".*?<channelRef ref=\"channel-0\""#,
            )
            .map_err(|error| format!("无法创建 Build-Tools 版本规则：{error}"))?;
            let version = package_pattern
                .captures(&content)
                .and_then(|captures| captures.name("version"))
                .map(|value| value.as_str().to_string())
                .ok_or_else(|| "无法识别远端 Android Build-Tools 版本。".to_string())?;
            let package_path = format!("build-tools;{version}");
            let package = android_repository_package_block(&content, &package_path)
                .ok_or_else(|| "无法读取远端 Android Build-Tools 安装包信息。".to_string())?;
            let url = android_repository_windows_archive_url(package)
                .ok_or_else(|| "远端 Android Build-Tools 没有 Windows 安装包。".to_string())?;
            Ok(EnvironmentToolRemote {
                file_name: url
                    .rsplit('/')
                    .next()
                    .unwrap_or("build-tools-windows.zip")
                    .to_string(),
                url,
                version,
                package_kind: EnvironmentToolPackageKind::Zip,
            })
        }
        _ => Err("未知 Android SDK 工具。".to_string()),
    }
}

pub(super) async fn java_environment_tool_release() -> Result<EnvironmentToolRemote, String> {
    let release = environment_http_client()?
        .get("https://api.adoptium.net/v3/assets/latest/21/hotspot?architecture=x64&image_type=jdk&os=windows&vendor=eclipse")
        .send()
        .await
        .map_err(|error| format!("无法查询 Java 运行时版本：{error}"))?
        .error_for_status()
        .map_err(|error| format!("Java 运行时请求失败：{error}"))?
        .json::<Vec<AdoptiumRelease>>()
        .await
        .map_err(|error| format!("无法读取 Java 运行时版本：{error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "远端没有可用的 Java 21 运行时。".to_string())?;
    Ok(EnvironmentToolRemote {
        version: release.version.openjdk_version,
        url: release.binary.package.link,
        file_name: release.binary.package.name,
        package_kind: EnvironmentToolPackageKind::Zip,
    })
}

pub(super) async fn fetch_environment_tool_remote(
    tool_id: &str,
) -> Result<EnvironmentToolRemote, String> {
    match tool_id {
        "adb" | "aapt2" => android_repository_environment_tool(tool_id).await,
        "java" => java_environment_tool_release().await,
        "python" => python_environment_tool_release().await,
        "node" => node_environment_tool_release().await,
        "apktool" => {
            github_environment_tool_release("iBotPeaches/Apktool", |name| {
                name.starts_with("apktool_") && name.ends_with(".jar")
            })
            .await
        }
        "jadx" => {
            github_environment_tool_release("skylot/jadx", |name| {
                name.starts_with("jadx-") && name.ends_with(".zip") && !name.contains("-gui-")
            })
            .await
        }
        "bundletool" => {
            github_environment_tool_release("google/bundletool", |name| {
                name.starts_with("bundletool-all-") && name.ends_with(".jar")
            })
            .await
        }
        "scrcpy" => {
            github_environment_tool_release("Genymobile/scrcpy", |name| {
                name.starts_with("scrcpy-win64-") && name.ends_with(".zip")
            })
            .await
        }
        _ => Err("未知环境工具。".to_string()),
    }
}

pub(super) fn merge_environment_tool_remote(
    status: &mut EnvironmentToolStatus,
    remote: Result<EnvironmentToolRemote, String>,
) {
    match remote {
        Ok(remote) => {
            status.remote_version = Some(remote.version);
            status.remote_message = Some("已从官方发布源检查。".to_string());
        }
        Err(error) => status.remote_message = Some(error),
    }
}

#[tauri::command]
pub async fn list_environment_tools(
    request: EnvironmentToolQuery,
) -> Result<Vec<EnvironmentToolStatus>, String> {
    let local_tasks = ENVIRONMENT_TOOL_IDS.into_iter().map(|tool_id| {
        tauri::async_runtime::spawn_blocking(move || inspect_local_environment_tool(tool_id))
    });
    let mut statuses = join_all(local_tasks)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("环境工具检测任务失败：{error}"))?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    if request.include_remote {
        let remote_results = join_all(
            REMOTE_ENVIRONMENT_TOOL_IDS
                .into_iter()
                .map(fetch_environment_tool_remote),
        )
        .await;
        for (status, remote) in statuses
            .iter_mut()
            .filter(|status| status.id != "gradle")
            .zip(remote_results)
        {
            merge_environment_tool_remote(status, remote);
        }
    }
    Ok(statuses)
}

#[tauri::command]
pub async fn check_environment_tool(tool_id: String) -> Result<EnvironmentToolStatus, String> {
    tauri::async_runtime::spawn_blocking(move || inspect_local_environment_tool(&tool_id))
        .await
        .map_err(|error| format!("环境工具检测任务失败：{error}"))?
}

#[tauri::command]
pub fn save_environment_tool_path(
    request: EnvironmentToolPathRequest,
) -> Result<EnvironmentToolStatus, String> {
    if !known_environment_tool(&request.tool_id) {
        return Err("未知环境工具。".to_string());
    }
    let path = normalized_environment_tool_path(
        &fs::canonicalize(request.path.trim())
            .map_err(|error| format!("无法读取工具文件：{error}"))?,
    );
    if !path.is_file() || !environment_tool_path_is_valid(&request.tool_id, &path) {
        let hint = environment_tool_metadata(&request.tool_id)
            .map(|metadata| metadata.2)
            .unwrap_or("请选择正确的工具文件");
        return Err(format!("工具文件类型不匹配，请{hint}。"));
    }
    // Do not publish a runtime path until both the executable and its required tools work.
    match request.tool_id.as_str() {
        "node" => validate_node_runtime(&path)?,
        "python" => {
            environment_tool_version("python", &path)?;
        }
        _ => {}
    }
    update_environment_tool_config(|config| {
        config
            .paths
            .insert(request.tool_id.clone(), path.to_string_lossy().to_string());
    })?;
    install_bundled_child_environment()?;
    inspect_local_environment_tool(&request.tool_id)
}

pub(super) fn environment_tool_download_file(root: &Path, tool_id: &str) -> PathBuf {
    root.join(format!(".{tool_id}-download.tmp"))
}

pub(super) fn emit_environment_tool_download_progress(
    app: &AppHandle,
    tool_id: &str,
    phase: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) {
    let _ = app.emit(
        ENVIRONMENT_TOOL_DOWNLOAD_PROGRESS_EVENT,
        EnvironmentToolDownloadProgress {
            tool_id: tool_id.to_string(),
            phase: phase.to_string(),
            downloaded_bytes,
            total_bytes,
        },
    );
}

pub(super) async fn download_environment_tool_file(
    app: &AppHandle,
    root: &Path,
    tool_id: &str,
    remote: &EnvironmentToolRemote,
) -> Result<PathBuf, String> {
    let temporary = environment_tool_download_file(root, tool_id);
    let existing_bytes = tokio::fs::metadata(&temporary)
        .await
        .ok()
        .map(|metadata| metadata.len())
        .filter(|size| *size <= MAX_ENVIRONMENT_TOOL_DOWNLOAD_BYTES)
        .unwrap_or(0);
    let mut request = environment_tool_download_client()?.get(&remote.url);
    if existing_bytes > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing_bytes}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("无法下载 {}：{error}", remote.file_name))?
        .error_for_status()
        .map_err(|error| format!("下载 {} 失败：{error}", remote.file_name))?;
    let resumed = existing_bytes > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let base_bytes = if resumed { existing_bytes } else { 0 };
    let total_bytes = response
        .content_length()
        .map(|remaining| remaining.saturating_add(base_bytes));
    if total_bytes.is_some_and(|size| size > MAX_ENVIRONMENT_TOOL_DOWNLOAD_BYTES) {
        return Err("远端工具包超过 768 MB 下载限制。".to_string());
    }

    let mut output = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&temporary)
            .await
            .map_err(|error| format!("无法继续工具下载文件：{error}"))?
    } else {
        tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| format!("无法创建工具下载文件：{error}"))?
    };
    let mut received = base_bytes;
    let mut last_progress_update = Instant::now() - Duration::from_secs(1);
    emit_environment_tool_download_progress(app, tool_id, "downloading", received, total_bytes);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("下载工具时网络中断：{error}"))?;
        received = received.saturating_add(chunk.len() as u64);
        if received > MAX_ENVIRONMENT_TOOL_DOWNLOAD_BYTES {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err("远端工具包超过 768 MB 下载限制。".to_string());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| format!("无法写入工具下载文件：{error}"))?;
        if last_progress_update.elapsed() >= Duration::from_millis(120)
            || total_bytes.is_some_and(|total| received >= total)
        {
            emit_environment_tool_download_progress(
                app,
                tool_id,
                "downloading",
                received,
                total_bytes,
            );
            last_progress_update = Instant::now();
        }
    }
    output
        .flush()
        .await
        .map_err(|error| format!("无法完成工具下载：{error}"))?;
    emit_environment_tool_download_progress(app, tool_id, "downloading", received, total_bytes);
    Ok(temporary)
}

pub(super) fn safe_environment_tool_name(value: &str) -> String {
    let result = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect::<String>();
    if result.is_empty() {
        "latest".to_string()
    } else {
        result
    }
}

pub(super) fn extract_environment_tool_zip(
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    let file = File::open(source).map_err(|error| format!("无法打开工具压缩包：{error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("无法读取工具压缩包：{error}"))?;
    let mut extracted_bytes = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取工具压缩包条目：{error}"))?;
        let Some(relative) = entry.enclosed_name().map(PathBuf::from) else {
            continue;
        };
        let target = destination.join(relative);
        if !target.starts_with(destination) {
            return Err("工具压缩包包含不安全路径。".to_string());
        }
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|error| format!("无法创建工具目录：{error}"))?;
            continue;
        }
        extracted_bytes = extracted_bytes.saturating_add(entry.size());
        if extracted_bytes > MAX_ENVIRONMENT_TOOL_EXTRACTED_BYTES {
            return Err("工具压缩包解压后超过 2 GB 限制。".to_string());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建工具目录：{error}"))?;
        }
        let mut output =
            File::create(&target).map_err(|error| format!("无法创建工具文件：{error}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("无法解压工具文件：{error}"))?;
    }
    Ok(())
}

pub(super) fn downloaded_environment_tool_file_name(tool_id: &str) -> &'static str {
    match tool_id {
        "node" => {
            if cfg!(windows) {
                "node.exe"
            } else {
                "node"
            }
        }
        "python" => {
            if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }
        }
        "adb" => {
            if cfg!(target_os = "windows") {
                "adb.exe"
            } else {
                "adb"
            }
        }
        "aapt2" => {
            if cfg!(target_os = "windows") {
                "aapt2.exe"
            } else {
                "aapt2"
            }
        }
        "java" => {
            if cfg!(target_os = "windows") {
                "java.exe"
            } else {
                "java"
            }
        }
        "jadx" => {
            if cfg!(target_os = "windows") {
                "jadx.bat"
            } else {
                "jadx"
            }
        }
        "scrcpy" => {
            if cfg!(target_os = "windows") {
                "scrcpy.exe"
            } else {
                "scrcpy"
            }
        }
        _ => "",
    }
}

pub(super) fn find_downloaded_environment_tool(
    root: &Path,
    tool_id: &str,
    depth: usize,
) -> Option<PathBuf> {
    let expected = downloaded_environment_tool_file_name(tool_id);
    if expected.is_empty() || depth > 5 {
        return None;
    }
    let mut entries = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(expected))
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(path) = find_downloaded_environment_tool(&path, tool_id, depth + 1) {
                return Some(path);
            }
        }
    }
    None
}

pub(super) fn install_downloaded_environment_tool(
    root: &Path,
    tool_id: &str,
    remote: &EnvironmentToolRemote,
    temporary: &Path,
) -> Result<PathBuf, String> {
    match remote.package_kind {
        EnvironmentToolPackageKind::File => {
            let destination = root.join(&remote.file_name);
            if destination.exists() {
                fs::remove_file(&destination)
                    .map_err(|error| format!("无法替换旧工具文件：{error}"))?;
            }
            fs::rename(temporary, &destination)
                .map_err(|error| format!("无法安装工具文件：{error}"))?;
            Ok(destination)
        }
        EnvironmentToolPackageKind::Zip => {
            let destination = root.join(format!(
                "{}-{}",
                tool_id,
                safe_environment_tool_name(&remote.version)
            ));
            if destination.exists() {
                fs::remove_dir_all(&destination)
                    .map_err(|error| format!("无法替换旧工具目录：{error}"))?;
            }
            fs::create_dir_all(&destination)
                .map_err(|error| format!("无法创建工具安装目录：{error}"))?;
            let extraction = extract_environment_tool_zip(temporary, &destination);
            let _ = fs::remove_file(temporary);
            if let Err(error) = extraction {
                let _ = fs::remove_dir_all(&destination);
                return Err(error);
            }
            find_downloaded_environment_tool(&destination, tool_id, 0)
                .ok_or_else(|| "工具包已下载，但没有找到可执行文件。".to_string())
        }
    }
}

#[tauri::command]
pub async fn download_environment_tool(
    app: AppHandle,
    request: EnvironmentToolDownloadRequest,
) -> Result<EnvironmentToolStatus, String> {
    if !known_environment_tool(&request.tool_id) {
        return Err("未知环境工具。".to_string());
    }
    let _reservation = reserve_environment_tool_download(&request.tool_id)?;
    let result = download_environment_tool_inner(&app, &request.tool_id).await;
    if result.is_err() {
        emit_environment_tool_download_progress(&app, &request.tool_id, "failed", 0, None);
    }
    result
}

async fn download_environment_tool_inner(
    app: &AppHandle,
    tool_id: &str,
) -> Result<EnvironmentToolStatus, String> {
    emit_environment_tool_download_progress(app, tool_id, "resolving", 0, None);
    let remote = fetch_environment_tool_remote(tool_id).await?;
    let node_checksum = if tool_id == "node" {
        Some(node_download_checksum(&remote).await?)
    } else {
        None
    };
    let root = environment_tools_root()?;
    let temporary = download_environment_tool_file(app, &root, tool_id, &remote).await?;
    emit_environment_tool_download_progress(app, tool_id, "installing", 0, None);
    let install_root = root.clone();
    let install_tool_id = tool_id.to_string();
    let remote_for_install = remote.clone();
    let installed_path = tauri::async_runtime::spawn_blocking(move || {
        if let Some(checksum) = node_checksum {
            verify_node_archive(&temporary, &checksum)?;
        }
        install_downloaded_environment_tool(
            &install_root,
            &install_tool_id,
            &remote_for_install,
            &temporary,
        )
    })
    .await
    .map_err(|error| format!("工具安装任务失败：{error}"))??;
    let request = EnvironmentToolPathRequest {
        tool_id: tool_id.to_string(),
        path: installed_path.to_string_lossy().to_string(),
    };
    let mut status = tauri::async_runtime::spawn_blocking(move || save_environment_tool_path(request))
        .await
        .map_err(|error| format!("环境验证任务失败：{error}"))??;
    if status.local_status != "ready" {
        return Err(format!("环境校验失败：{}", status.local_message));
    }
    status.remote_version = Some(remote.version);
    status.remote_message = Some("已从官方发布源下载安装。".to_string());
    emit_environment_tool_download_progress(app, &status.id, "completed", 0, None);
    Ok(status)
}
